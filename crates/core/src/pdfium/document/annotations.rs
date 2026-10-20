//! Anotaciones del usuario: resaltar, subrayar y tachar texto, notas, texto
//! libre y dibujo a mano.
//!
//! Se crean directamente en las páginas del documento abierto y se guardan
//! con el resto de cambios (actualización incremental). PDFium genera la
//! apariencia de las de texto marcado, las notas y los trazos; el texto libre
//! es un sello con el texto dentro (PDFium no dibuja las anotaciones FreeText
//! sin apariencia), marcado en `NM` para reconocerlo y poder editarlo.
//!
//! No se ofrecen en formularios XFA: al guardar solo se conservan sus datos.

use super::super::{from_wide_buf, sys, to_wide};
use super::Document;
use anyhow::{Result, bail};
use std::ffi::{c_int, c_ulong};

/// Prefijo del `NM` de los textos libres (sellos con texto).
const FREE_TEXT_NM: &str = "pdfre-texto-";
/// Margen interior del texto libre, en puntos.
const FREE_TEXT_PAD: f32 = 3.0;
/// Alto de línea del texto libre, en veces el tamaño de letra.
const LINE_HEIGHT: f32 = 1.25;

/// Tipo de anotación que se puede crear, editar o borrar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotKind {
    Highlight,
    Underline,
    StrikeOut,
    /// Nota (icono con un comentario).
    Note,
    /// Texto escrito en la página.
    FreeText,
    /// Dibujo a mano.
    Ink,
}

/// Una anotación de la página.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub page: usize,
    /// Posición en la lista de anotaciones de la página (cambia al borrar
    /// otra anterior).
    pub index: usize,
    pub kind: AnnotKind,
    /// Izquierda, abajo, derecha, arriba (coordenadas de página).
    pub rect: [f32; 4],
    /// El comentario (nota), el texto (texto libre) o el texto marcado.
    pub contents: String,
    pub color: [u8; 3],
    /// Tamaño de letra del texto libre.
    pub font_size: f32,
}

/// Una anotación abierta de PDFium, que se cierra al soltarla.
struct Annot(sys::FPDF_ANNOTATION);

impl Drop for Annot {
    fn drop(&mut self) {
        unsafe { sys::FPDFPage_CloseAnnot(self.0) }
    }
}

impl Annot {
    fn string(&self, key: &std::ffi::CStr) -> String {
        unsafe {
            let n = sys::FPDFAnnot_GetStringValue(self.0, key.as_ptr(), std::ptr::null_mut(), 0) as usize;
            if n <= 2 {
                return String::new();
            }
            let mut buf = vec![0u16; n / 2];
            sys::FPDFAnnot_GetStringValue(self.0, key.as_ptr(), buf.as_mut_ptr(), (buf.len() * 2) as c_ulong);
            from_wide_buf(&buf)
        }
    }

    fn set_string(&self, key: &std::ffi::CStr, value: &str) {
        let w = to_wide(value);
        unsafe { sys::FPDFAnnot_SetStringValue(self.0, key.as_ptr(), w.as_ptr()) };
    }

    fn rect(&self) -> [f32; 4] {
        let mut r = sys::FS_RECTF { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };
        unsafe { sys::FPDFAnnot_GetRect(self.0, &mut r) };
        [r.left.min(r.right), r.bottom.min(r.top), r.left.max(r.right), r.bottom.max(r.top)]
    }

    fn set_rect(&self, [left, bottom, right, top]: [f32; 4]) {
        let r = sys::FS_RECTF { left, top, right, bottom };
        unsafe { sys::FPDFAnnot_SetRect(self.0, &r) };
    }

    fn color(&self) -> Option<[u8; 3]> {
        let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
        let ok = unsafe { sys::FPDFAnnot_GetColor(self.0, sys::FPDFANNOT_COLORTYPE_Color, &mut r, &mut g, &mut b, &mut a) };
        (ok != 0).then_some([r as u8, g as u8, b as u8])
    }

    fn set_color(&self, [r, g, b]: [u8; 3]) {
        unsafe { sys::FPDFAnnot_SetColor(self.0, sys::FPDFANNOT_COLORTYPE_Color, r as u32, g as u32, b as u32, 255) };
    }

    /// Qué es, si es de las que se pueden editar aquí.
    fn kind(&self) -> Option<AnnotKind> {
        match unsafe { sys::FPDFAnnot_GetSubtype(self.0) } as u32 {
            sys::FPDF_ANNOT_HIGHLIGHT => Some(AnnotKind::Highlight),
            sys::FPDF_ANNOT_UNDERLINE | sys::FPDF_ANNOT_SQUIGGLY => Some(AnnotKind::Underline),
            sys::FPDF_ANNOT_STRIKEOUT => Some(AnnotKind::StrikeOut),
            sys::FPDF_ANNOT_TEXT => Some(AnnotKind::Note),
            sys::FPDF_ANNOT_INK => Some(AnnotKind::Ink),
            sys::FPDF_ANNOT_STAMP if self.string(c"NM").starts_with(FREE_TEXT_NM) => Some(AnnotKind::FreeText),
            _ => None,
        }
    }
}

/// Tamaño de letra y color guardados en el `DA` de un texto libre
/// ("/Helv 12 Tf 0 0 1 rg").
fn parse_da(da: &str) -> (Option<f32>, Option<[u8; 3]>) {
    let t: Vec<&str> = da.split_whitespace().collect();
    let size = t.iter().position(|&w| w == "Tf").and_then(|i| t.get(i.wrapping_sub(1))).and_then(|s| s.parse().ok());
    let color = t.iter().position(|&w| w == "rg").filter(|&i| i >= 3).and_then(|i| {
        let c: Option<Vec<f32>> = t[i - 3..i].iter().map(|s| s.parse().ok()).collect();
        c.map(|c| [(c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8])
    });
    (size, color)
}

impl Document {
    /// Se pueden añadir anotaciones (no en formularios XFA).
    pub fn can_annotate(&self) -> bool {
        !self.xfa_loaded
    }

    fn annot(&self, page: usize, index: usize) -> Option<Annot> {
        let p = self.page(page);
        if p.is_null() {
            return None;
        }
        let a = unsafe { sys::FPDFPage_GetAnnot(p, index as c_int) };
        (!a.is_null()).then_some(Annot(a))
    }

    fn new_annot(&mut self, page: usize, subtype: u32) -> Result<Annot> {
        let p = self.page(page);
        if p.is_null() {
            bail!("no existe la página {}", page + 1);
        }
        let a = unsafe { sys::FPDFPage_CreateAnnot(p, subtype as c_int) };
        if a.is_null() {
            bail!("PDFium no pudo crear la anotación");
        }
        let a = Annot(a);
        self.sig_counter += 1;
        let prefix = if subtype == sys::FPDF_ANNOT_STAMP { FREE_TEXT_NM } else { "pdfre-anot-" };
        a.set_string(c"NM", &format!("{prefix}{}-{}", std::process::id(), self.sig_counter));
        unsafe { sys::FPDFAnnot_SetFlags(a.0, sys::FPDF_ANNOT_FLAG_PRINT as c_int) };
        Ok(a)
    }

    /// Las anotaciones de la página que se pueden editar o borrar aquí.
    pub fn annotations(&self, page: usize) -> Vec<Annotation> {
        let p = self.page(page);
        if p.is_null() {
            return Vec::new();
        }
        let n = unsafe { sys::FPDFPage_GetAnnotCount(p) }.max(0) as usize;
        let mut v = Vec::new();
        for index in 0..n {
            let Some(a) = self.annot(page, index) else { continue };
            let Some(kind) = a.kind() else { continue };
            let (size, da_color) = if kind == AnnotKind::FreeText { parse_da(&a.string(c"DA")) } else { (None, None) };
            v.push(Annotation {
                page,
                index,
                kind,
                rect: a.rect(),
                contents: a.string(c"Contents"),
                color: da_color.or_else(|| a.color()).unwrap_or([255, 210, 0]),
                font_size: size.unwrap_or(12.0),
            });
        }
        v
    }

    /// La anotación de encima en el punto (coordenadas de página).
    pub fn annotation_at(&self, page: usize, x: f32, y: f32) -> Option<Annotation> {
        const SLACK: f32 = 2.0;
        self.annotations(page).into_iter().rev().find(|a| {
            let [l, b, r, t] = a.rect;
            x >= l - SLACK && x <= r + SLACK && y >= b - SLACK && y <= t + SLACK
        })
    }

    /// Resalta, subraya o tacha el texto que ocupa `rects` (los de
    /// `text_rects`: izquierda, arriba, derecha, abajo). `text` es el texto
    /// marcado, que otros visores enseñan en la lista de comentarios.
    pub fn add_markup(&mut self, page: usize, kind: AnnotKind, rects: &[[f64; 4]], color: [u8; 3], text: &str) -> Result<()> {
        let subtype = match kind {
            AnnotKind::Highlight => sys::FPDF_ANNOT_HIGHLIGHT,
            AnnotKind::Underline => sys::FPDF_ANNOT_UNDERLINE,
            AnnotKind::StrikeOut => sys::FPDF_ANNOT_STRIKEOUT,
            _ => bail!("no es una marca de texto"),
        };
        if rects.is_empty() {
            bail!("no hay texto seleccionado");
        }
        let a = self.new_annot(page, subtype)?;
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for &[l, t, r, b] in rects {
            let (l, t, r, b) = (l as f32, t as f32, r as f32, b as f32);
            let q = sys::FS_QUADPOINTSF { x1: l, y1: t, x2: r, y2: t, x3: l, y3: b, x4: r, y4: b };
            unsafe { sys::FPDFAnnot_AppendAttachmentPoints(a.0, &q) };
            bounds = [bounds[0].min(l), bounds[1].min(b), bounds[2].max(r), bounds[3].max(t)];
        }
        a.set_rect(bounds);
        a.set_color(color);
        if !text.is_empty() {
            a.set_string(c"Contents", text);
        }
        Ok(())
    }

    /// Pone una nota (un icono con un comentario) con su esquina superior
    /// izquierda en (x, y).
    pub fn add_note(&mut self, page: usize, x: f32, y: f32, text: &str, color: [u8; 3]) -> Result<()> {
        const SIZE: f32 = 20.0;
        let rect = self.clamp_to_page(page, [x, y - SIZE, x + SIZE, y]);
        let a = self.new_annot(page, sys::FPDF_ANNOT_TEXT)?;
        a.set_rect(rect);
        a.set_color(color);
        a.set_string(c"Contents", text);
        a.set_string(c"Name", "Comment");
        Ok(())
    }

    /// Escribe `text` en la página, con su esquina superior izquierda en
    /// (x, y). Cada línea del texto es una línea en la página.
    pub fn add_free_text(&mut self, page: usize, x: f32, y: f32, text: &str, size: f32, color: [u8; 3]) -> Result<()> {
        let text = text.trim_end();
        if text.is_empty() {
            bail!("el texto está vacío");
        }
        let size = size.clamp(4.0, 144.0);
        let doc = self.doc;
        let font = unsafe { sys::FPDFText_LoadStandardFont(doc, c"Helvetica".as_ptr()) };
        if font.is_null() {
            bail!("PDFium no pudo cargar la fuente");
        }
        // Las líneas, medidas antes de colocarlas.
        let mut objs = Vec::new();
        let mut width: f32 = 0.0;
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            unsafe {
                let obj = sys::FPDFPageObj_CreateTextObj(doc, font, size);
                let w = to_wide(line);
                sys::FPDFText_SetText(obj, w.as_ptr());
                sys::FPDFPageObj_SetFillColor(obj, color[0] as u32, color[1] as u32, color[2] as u32, 255);
                let (mut l, mut b, mut r, mut t) = (0.0, 0.0, 0.0, 0.0);
                sys::FPDFPageObj_GetBounds(obj, &mut l, &mut b, &mut r, &mut t);
                width = width.max(r - l);
                objs.push((i, obj));
            }
        }
        unsafe { sys::FPDFFont_Close(font) };
        let lines = text.lines().count() as f32;
        let w = width + 2.0 * FREE_TEXT_PAD;
        let h = lines * size * LINE_HEIGHT + 2.0 * FREE_TEXT_PAD;
        let [left, bottom, right, top] = self.clamp_to_page(page, [x, y - h, x + w, y]);
        let a = self.new_annot(page, sys::FPDF_ANNOT_STAMP)?;
        a.set_rect([left, bottom, right, top]);
        for (i, obj) in objs {
            // Línea base: un poco por encima del borde inferior de la línea.
            let base = top - FREE_TEXT_PAD - (i as f32 + 1.0) * size * LINE_HEIGHT + size * 0.25;
            unsafe {
                sys::FPDFPageObj_Transform(obj, 1.0, 0.0, 0.0, 1.0, (left + FREE_TEXT_PAD) as f64, base as f64);
                sys::FPDFAnnot_AppendObject(a.0, obj);
            }
        }
        let [r, g, b] = color.map(|c| c as f32 / 255.0);
        a.set_string(c"DA", &format!("/Helv {size} Tf {r:.3} {g:.3} {b:.3} rg"));
        a.set_string(c"Contents", text);
        a.set_color(color);
        Ok(())
    }

    /// Dibuja los trazos (puntos en coordenadas de página) con un grosor de
    /// `width` puntos, como una sola anotación.
    pub fn add_ink(&mut self, page: usize, strokes: &[Vec<[f32; 2]>], width: f32, color: [u8; 3]) -> Result<()> {
        let strokes: Vec<&Vec<[f32; 2]>> = strokes.iter().filter(|s| !s.is_empty()).collect();
        if strokes.is_empty() {
            bail!("no hay trazos");
        }
        let a = self.new_annot(page, sys::FPDF_ANNOT_INK)?;
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for s in &strokes {
            // Un toque sin moverse: un punto visible.
            let pts: Vec<sys::FS_POINTF> = if s.len() == 1 {
                vec![sys::FS_POINTF { x: s[0][0], y: s[0][1] }, sys::FS_POINTF { x: s[0][0] + 0.1, y: s[0][1] }]
            } else {
                s.iter().map(|p| sys::FS_POINTF { x: p[0], y: p[1] }).collect()
            };
            for p in &pts {
                bounds = [bounds[0].min(p.x), bounds[1].min(p.y), bounds[2].max(p.x), bounds[3].max(p.y)];
            }
            if unsafe { sys::FPDFAnnot_AddInkStroke(a.0, pts.as_ptr(), pts.len()) } < 0 {
                bail!("PDFium no pudo añadir el trazo");
            }
        }
        let m = width / 2.0 + 1.0;
        a.set_rect([bounds[0] - m, bounds[1] - m, bounds[2] + m, bounds[3] + m]);
        unsafe { sys::FPDFAnnot_SetBorder(a.0, 0.0, 0.0, width) };
        a.set_color(color);
        Ok(())
    }

    /// Cambia el comentario de una nota o el texto de un texto libre.
    pub fn set_annotation_text(&mut self, page: usize, index: usize, text: &str) -> Result<()> {
        let Some(info) = self.annotations(page).into_iter().find(|a| a.index == index) else { bail!("no existe esa anotación") };
        match info.kind {
            AnnotKind::FreeText => {
                // Se vuelve a escribir en el mismo sitio, con la misma letra.
                let [left, _, _, top] = info.rect;
                self.add_free_text(page, left, top, text, info.font_size, info.color)?;
                self.remove_annotation(page, index)
            }
            _ => {
                let Some(a) = self.annot(page, index) else { bail!("no existe esa anotación") };
                a.set_string(c"Contents", text);
                Ok(())
            }
        }
    }

    /// Borra una anotación (y su ventana emergente, si tiene).
    pub fn remove_annotation(&mut self, page: usize, index: usize) -> Result<()> {
        let p = self.page(page);
        let Some(a) = self.annot(page, index) else { bail!("no existe esa anotación") };
        let popup = unsafe { sys::FPDFAnnot_GetLinkedAnnot(a.0, c"Popup".as_ptr()) };
        let popup_index = if popup.is_null() {
            None
        } else {
            let popup = Annot(popup);
            Some(unsafe { sys::FPDFPage_GetAnnotIndex(p, popup.0) })
        };
        drop(a);
        let mut indices = vec![index as c_int];
        indices.extend(popup_index.filter(|&i| i >= 0));
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for i in indices {
            if unsafe { sys::FPDFPage_RemoveAnnot(p, i) } == 0 {
                bail!("PDFium no pudo borrar la anotación");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_da;

    #[test]
    fn leer_da() {
        assert_eq!(parse_da("/Helv 14 Tf 0.000 0.000 1.000 rg"), (Some(14.0), Some([0, 0, 255])));
        assert_eq!(parse_da(""), (None, None));
    }
}
