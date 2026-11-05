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

    /// Su color. PDFium no lo da si la anotación ya tiene apariencia (la
    /// genera al dibujarla y se guarda con ella): entonces se lee de la
    /// apariencia.
    fn color(&self) -> Option<[u8; 3]> {
        let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
        let ok = unsafe { sys::FPDFAnnot_GetColor(self.0, sys::FPDFANNOT_COLORTYPE_Color, &mut r, &mut g, &mut b, &mut a) };
        if ok != 0 {
            return Some([r as u8, g as u8, b as u8]);
        }
        appearance_color(&self.appearance())
    }

    /// El contenido de su apariencia normal.
    fn appearance(&self) -> String {
        unsafe {
            let mode = sys::FPDF_ANNOT_APPEARANCEMODE_NORMAL as sys::FPDF_ANNOT_APPEARANCEMODE;
            let n = sys::FPDFAnnot_GetAP(self.0, mode, std::ptr::null_mut(), 0) as usize;
            if n <= 2 {
                return String::new();
            }
            let mut buf = vec![0u16; n / 2];
            sys::FPDFAnnot_GetAP(self.0, mode, buf.as_mut_ptr(), (buf.len() * 2) as c_ulong);
            from_wide_buf(&buf)
        }
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

/// Dos cajas (izquierda, abajo, derecha, arriba) en la misma línea: se
/// solapan en vertical al menos la mitad de la más baja.
fn same_line(a: &[f32; 4], b: &[f32; 4]) -> bool {
    let overlap = a[3].min(b[3]) - a[1].max(b[1]);
    overlap > 0.5 * (a[3] - a[1]).min(b[3] - b[1])
}

/// Dos líneas de marca que se tocan: en la misma línea y sin hueco entre
/// ellas (o menos de un tercio de su alto).
fn touches(a: &[f32; 4], b: &[f32; 4]) -> bool {
    let gap = (a[0] - b[2]).max(b[0] - a[2]);
    same_line(a, b) && gap < 0.35 * (a[3] - a[1]).min(b[3] - b[1])
}

/// Junta las cajas que se tocan en una por línea (y de arriba abajo).
fn merge_lines(mut rects: Vec<[f32; 4]>) -> Vec<[f32; 4]> {
    rects.retain(|r| r[2] > r[0] && r[3] > r[1]);
    let mut out: Vec<[f32; 4]> = Vec::new();
    loop {
        let mut changed = false;
        for r in std::mem::take(&mut rects) {
            if let Some(o) = out.iter_mut().find(|o| touches(o, &r)) {
                *o = [o[0].min(r[0]), o[1].min(r[1]), o[2].max(r[2]), o[3].max(r[3])];
                changed = true;
            } else {
                out.push(r);
            }
        }
        if !changed {
            break;
        }
        rects = std::mem::take(&mut out);
    }
    out.sort_by(|a, b| b[3].total_cmp(&a[3]).then(a[0].total_cmp(&b[0])));
    out
}

/// Un carácter de la página, para marcarlo.
#[derive(Clone, Copy)]
enum CharBox {
    /// Fin de línea.
    Break,
    /// No ocupa sitio.
    Empty,
    /// Su caja "holgada" (la altura de la fuente; izquierda, abajo, derecha,
    /// arriba) y si es un espacio.
    Char([f32; 4], bool),
}

impl CharBox {
    /// Un espacio, un fin de línea o algo que no ocupa: entre dos trozos
    /// marcados del mismo color no los separa.
    fn is_gap(&self) -> bool {
        !matches!(self, CharBox::Char(_, false))
    }
}

/// Rectángulos (izquierda, abajo, derecha, arriba) de las líneas de los
/// caracteres [start, end), sin los espacios de los extremos (salvo los del
/// final con `keep_end`).
fn line_rects(boxes: &[CharBox], start: usize, end: usize, keep_end: bool) -> Vec<[f32; 4]> {
    let mut lines: Vec<[f32; 4]> = Vec::new();
    // Caracteres de la línea en curso: su caja y si es un espacio.
    let mut cur: Vec<([f32; 4], bool)> = Vec::new();
    let flush = |cur: &mut Vec<([f32; 4], bool)>, lines: &mut Vec<[f32; 4]>, keep_end: bool| {
        let first = cur.iter().position(|c| !c.1);
        let last = if keep_end && first.is_some() { cur.len().checked_sub(1) } else { cur.iter().rposition(|c| !c.1) };
        if let (Some(a), Some(b)) = (first, last) {
            let mut u = cur[a].0;
            for c in &cur[a..=b] {
                u = [u[0].min(c.0[0]), u[1].min(c.0[1]), u[2].max(c.0[2]), u[3].max(c.0[3])];
            }
            lines.push(u);
        }
        cur.clear();
    };
    for c in &boxes[start.min(boxes.len())..end.min(boxes.len())] {
        match *c {
            CharBox::Break => flush(&mut cur, &mut lines, false),
            CharBox::Empty => {}
            CharBox::Char(b, space) => {
                // Otra línea: no se solapa en vertical con la que se lleva.
                if let Some(&(prev, _)) = cur.last()
                    && !same_line(&prev, &b)
                {
                    flush(&mut cur, &mut lines, false);
                }
                cur.push((b, space));
            }
        }
    }
    flush(&mut cur, &mut lines, keep_end);
    lines
}

/// Los caracteres que cubre una marca: los que tienen el centro dentro de
/// alguno de sus rectángulos (con algo de margen en vertical, por las marcas
/// de otros programas, más justas que las cajas holgadas).
fn covered_chars(boxes: &[CharBox], quads: &[[f32; 4]]) -> Vec<usize> {
    boxes
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match c {
            CharBox::Char([l, b, r, t], _) => {
                let (x, y) = ((l + r) / 2.0, (b + t) / 2.0);
                quads
                    .iter()
                    .any(|q| {
                        let m = 0.25 * (q[3] - q[1]);
                        x >= q[0] && x <= q[2] && y >= q[1] - m && y <= q[3] + m
                    })
                    .then_some(i)
            }
            _ => None,
        })
        .collect()
}

/// Los trozos [inicio, fin) de caracteres seguidos con el mismo color. Los
/// espacios no cuentan (su color no separa dos trozos del mismo). El último
/// dato dice si el trozo llega hasta el siguiente (de otro color, en la misma
/// línea): entonces el espacio entre los dos es del primero, sin hueco.
fn color_runs(boxes: &[CharBox], colors: &[Option<[u8; 3]>]) -> Vec<(usize, usize, [u8; 3], bool)> {
    let mut runs: Vec<(usize, usize, [u8; 3], bool)> = Vec::new();
    for (i, c) in colors.iter().enumerate() {
        let Some(c) = c.filter(|_| !boxes[i].is_gap()) else { continue };
        match runs.last_mut() {
            Some((_, e, rc, _)) if *rc == c && (*e..i).all(|j| boxes[j].is_gap()) => *e = i + 1,
            _ => runs.push((i, i + 1, c, false)),
        }
    }
    for k in 1..runs.len() {
        let s = runs[k].0;
        let prev = &mut runs[k - 1];
        if (prev.1..s).all(|j| matches!(boxes[j], CharBox::Char(_, true) | CharBox::Empty)) {
            prev.1 = s;
            prev.3 = true;
        }
    }
    runs
}

/// Los rectángulos (izquierda, abajo, derecha, arriba) de los cuadriláteros
/// de una marca de texto.
fn quad_rects(a: &Annot) -> Vec<[f32; 4]> {
    let n = unsafe { sys::FPDFAnnot_CountAttachmentPoints(a.0) };
    (0..n)
        .filter_map(|i| {
            let mut q = sys::FS_QUADPOINTSF { x1: 0.0, y1: 0.0, x2: 0.0, y2: 0.0, x3: 0.0, y3: 0.0, x4: 0.0, y4: 0.0 };
            (unsafe { sys::FPDFAnnot_GetAttachmentPoints(a.0, i, &mut q) } != 0).then(|| {
                let xs = [q.x1, q.x2, q.x3, q.x4];
                let ys = [q.y1, q.y2, q.y3, q.y4];
                let min = |v: [f32; 4]| v.iter().copied().fold(f32::MAX, f32::min);
                let max = |v: [f32; 4]| v.iter().copied().fold(f32::MIN, f32::max);
                [min(xs), min(ys), max(xs), max(ys)]
            })
        })
        .collect()
}

/// El primer color de una apariencia ("1 0.84 0 rg", "0 G", "0 0 0 1 k"…):
/// el de una marca de texto o un dibujo, que solo pintan con uno.
fn appearance_color(ap: &str) -> Option<[u8; 3]> {
    let t: Vec<&str> = ap.split_whitespace().collect();
    let num = |s: &str| s.parse::<f32>().ok().map(|v| v.clamp(0.0, 1.0));
    let byte = |v: f32| (v * 255.0).round() as u8;
    for (i, op) in t.iter().enumerate() {
        let args = |n: usize| -> Option<Vec<f32>> { (i >= n).then(|| t[i - n..i].iter().map(|s| num(s)).collect::<Option<Vec<f32>>>()).flatten() };
        let c = match *op {
            "rg" | "RG" => args(3).map(|c| [byte(c[0]), byte(c[1]), byte(c[2])]),
            "g" | "G" => args(1).map(|c| [byte(c[0]); 3]),
            "k" | "K" => args(4).map(|c| [0, 1, 2].map(|j| byte((1.0 - c[j]) * (1.0 - c[3])))),
            _ => None,
        };
        if c.is_some() {
            return c;
        }
    }
    None
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

    /// La anotación de encima en el punto (coordenadas de página). En las
    /// marcas de texto cuenta lo marcado de cada línea, no el rectángulo que
    /// abarca todas.
    pub fn annotation_at(&self, page: usize, x: f32, y: f32) -> Option<Annotation> {
        self.annotations(page).into_iter().rev().find(|a| self.annotation_hit(a, x, y))
    }

    /// La marca de texto (resaltado, subrayado o tachado) de encima en el
    /// punto.
    pub fn markup_at(&self, page: usize, x: f32, y: f32) -> Option<Annotation> {
        self.annotations(page)
            .into_iter()
            .rev()
            .find(|a| matches!(a.kind, AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::StrikeOut) && self.annotation_hit(a, x, y))
    }

    fn annotation_hit(&self, a: &Annotation, x: f32, y: f32) -> bool {
        const SLACK: f32 = 2.0;
        let inside = |[l, b, r, t]: [f32; 4]| x >= l - SLACK && x <= r + SLACK && y >= b - SLACK && y <= t + SLACK;
        if !inside(a.rect) {
            return false;
        }
        if !matches!(a.kind, AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::StrikeOut) {
            return true;
        }
        let quads = self.annot(a.page, a.index).map(|an| quad_rects(&an)).unwrap_or_default();
        quads.is_empty() || quads.into_iter().any(inside)
    }

    /// Resalta, subraya o tacha los caracteres [start, start + count) de la
    /// página con `color`: un rectángulo por línea, con la altura de la letra
    /// (no la de cada trozo de texto: así el subrayado y el tachado van rectos
    /// y el resaltado es continuo). Lo que ya estaba marcado de otro color
    /// fuera de esos caracteres conserva el suyo.
    pub fn add_markup_range(&mut self, page: usize, kind: AnnotKind, start: i32, count: i32, color: [u8; 3]) -> Result<()> {
        self.mark_range(page, kind, start, count, Some(color)).map(|_| ())
    }

    /// Quita la marca de ese tipo de los caracteres [start, start + count)
    /// (lo de alrededor sigue marcado). true si había algo que quitar.
    pub fn remove_markup_range(&mut self, page: usize, kind: AnnotKind, start: i32, count: i32) -> Result<bool> {
        self.mark_range(page, kind, start, count, None)
    }

    /// Resalta, subraya o tacha el texto que ocupa `rects` (izquierda,
    /// arriba, derecha, abajo, como los de `text_rects`).
    pub fn add_markup(&mut self, page: usize, kind: AnnotKind, rects: &[[f64; 4]], color: [u8; 3]) -> Result<()> {
        let lines: Vec<[f32; 4]> = rects.iter().map(|&[l, t, r, b]| [l as f32, b.min(t) as f32, r as f32, t.max(b) as f32]).collect();
        let lines = merge_lines(lines);
        if lines.is_empty() {
            bail!("no hay texto seleccionado");
        }
        self.create_markup(page, kind, &lines, color)
    }

    /// Cambia el color de una marca de texto entera.
    pub fn set_markup_color(&mut self, page: usize, index: usize, color: [u8; 3]) -> Result<()> {
        let Some(info) = self.annotations(page).into_iter().find(|a| a.index == index) else { bail!("no existe esa anotación") };
        let Some(a) = self.annot(page, index) else { bail!("no existe esa anotación") };
        let lines = quad_rects(&a);
        drop(a);
        if lines.is_empty() {
            bail!("no es una marca de texto");
        }
        // PDFium no cambia el color de una anotación con apariencia: se hace
        // de nuevo, en el mismo sitio.
        self.create_markup(page, info.kind, &lines, color)?;
        self.remove_annotation(page, index)
    }

    /// Los caracteres de la página, para marcarlos.
    fn char_boxes(&self, page: usize) -> Vec<CharBox> {
        self.with_text_page(page, |tp| unsafe {
            (0..sys::FPDFText_CountChars(tp))
                .map(|i| {
                    let ch = char::from_u32(sys::FPDFText_GetUnicode(tp, i)).unwrap_or(' ');
                    if ch == '\r' || ch == '\n' {
                        return CharBox::Break;
                    }
                    let mut r = sys::FS_RECTF { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };
                    if sys::FPDFText_GetLooseCharBox(tp, i, &mut r) == 0 {
                        return CharBox::Empty;
                    }
                    let b = [r.left.min(r.right), r.bottom.min(r.top), r.left.max(r.right), r.bottom.max(r.top)];
                    if b[3] - b[1] <= 0.1 { CharBox::Empty } else { CharBox::Char(b, ch.is_whitespace()) }
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// Marca (con `Some`) o desmarca (con `None`) los caracteres [start,
    /// start + count). Las marcas del mismo tipo que los tocan se rehacen por
    /// caracteres: cada trozo conserva su color y los trozos seguidos del
    /// mismo color quedan en una sola marca (sin sumar colores ni dejar
    /// huecos). true si cambió algo.
    fn mark_range(&mut self, page: usize, kind: AnnotKind, start: i32, count: i32, color: Option<[u8; 3]>) -> Result<bool> {
        if !matches!(kind, AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::StrikeOut) {
            bail!("no es una marca de texto");
        }
        let boxes = self.char_boxes(page);
        let n = boxes.len();
        let (start, end) = (start.max(0) as usize, (start.max(0) + count.max(0)) as usize);
        let (start, end) = (start.min(n), end.min(n));
        if start >= end || line_rects(&boxes, start, end, false).is_empty() {
            bail!("no hay texto seleccionado");
        }
        // Lo más cercano antes y después que no es un espacio: una marca del
        // mismo color que llega hasta ahí se junta con la nueva.
        let before = (0..start).rev().find(|&i| !boxes[i].is_gap());
        let after = (end..n).find(|&i| !boxes[i].is_gap());
        let mut colors: Vec<Option<[u8; 3]>> = vec![None; n];
        let mut affected = Vec::new();
        for a in self.annotations(page).into_iter().filter(|a| a.kind == kind) {
            let Some(annot) = self.annot(page, a.index) else { continue };
            let quads = quad_rects(&annot);
            drop(annot);
            let chars = covered_chars(&boxes, &quads);
            let inside = chars.iter().any(|&c| c >= start && c < end);
            let next_to = color == Some(a.color) && chars.iter().any(|&c| Some(c) == before || Some(c) == after);
            if inside || next_to {
                for c in chars {
                    colors[c] = Some(a.color);
                }
                affected.push(a.index);
            }
        }
        if affected.is_empty() && color.is_none() {
            return Ok(false);
        }
        colors[start..end].fill(color);
        // De atrás adelante: borrar una no cambia el índice de las anteriores.
        affected.sort_unstable_by(|a, b| b.cmp(a));
        for i in affected {
            self.remove_annotation(page, i)?;
        }
        for (s, e, c, joined) in color_runs(&boxes, &colors) {
            let lines = line_rects(&boxes, s, e, joined);
            if !lines.is_empty() {
                self.create_markup(page, kind, &lines, c)?;
            }
        }
        Ok(true)
    }

    /// Una marca de texto nueva con esas líneas (izquierda, abajo, derecha,
    /// arriba).
    fn create_markup(&mut self, page: usize, kind: AnnotKind, lines: &[[f32; 4]], color: [u8; 3]) -> Result<()> {
        let subtype = match kind {
            AnnotKind::Highlight => sys::FPDF_ANNOT_HIGHLIGHT,
            AnnotKind::Underline => sys::FPDF_ANNOT_UNDERLINE,
            AnnotKind::StrikeOut => sys::FPDF_ANNOT_STRIKEOUT,
            _ => bail!("no es una marca de texto"),
        };
        let text = self.bounded_text(page, lines);
        let a = self.new_annot(page, subtype)?;
        let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for &[l, b, r, t] in lines {
            let q = sys::FS_QUADPOINTSF { x1: l, y1: t, x2: r, y2: t, x3: l, y3: b, x4: r, y4: b };
            unsafe { sys::FPDFAnnot_AppendAttachmentPoints(a.0, &q) };
            bounds = [bounds[0].min(l), bounds[1].min(b), bounds[2].max(r), bounds[3].max(t)];
        }
        a.set_rect(bounds);
        a.set_color(color);
        if !text.is_empty() {
            a.set_string(c"Contents", &text);
        }
        Ok(())
    }

    /// El texto dentro de esos rectángulos (izquierda, abajo, derecha, arriba).
    fn bounded_text(&self, page: usize, lines: &[[f32; 4]]) -> String {
        self.with_text_page(page, |tp| unsafe {
            let mut parts = Vec::new();
            for &[l, b, r, t] in lines {
                let (l, b, r, t) = (l as f64, b as f64, r as f64, t as f64);
                let n = sys::FPDFText_GetBoundedText(tp, l, t, r, b, std::ptr::null_mut(), 0);
                if n <= 0 {
                    continue;
                }
                let mut buf = vec![0u16; n as usize + 1];
                sys::FPDFText_GetBoundedText(tp, l, t, r, b, buf.as_mut_ptr(), buf.len() as c_int);
                let s = from_wide_buf(&buf);
                let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
                if !s.is_empty() {
                    parts.push(s);
                }
            }
            parts.join(" ")
        })
        .unwrap_or_default()
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
    use super::{appearance_color, parse_da};

    #[test]
    fn color_de_la_apariencia() {
        assert_eq!(appearance_color("/GS gs 1 0.839216 0 rg 72 700 m 200 700 l f"), Some([255, 214, 0]));
        assert_eq!(appearance_color("q 0.5 G 1 w 0 0 m 10 0 l S Q"), Some([128, 128, 128]));
        assert_eq!(appearance_color("0 0 0 1 k"), Some([0, 0, 0]));
        assert_eq!(appearance_color("72 700 m 200 700 l S"), None);
    }

    #[test]
    fn leer_da() {
        assert_eq!(parse_da("/Helv 14 Tf 0.000 0.000 1.000 rg"), (Some(14.0), Some([0, 0, 255])));
        assert_eq!(parse_da(""), (None, None));
    }
}
