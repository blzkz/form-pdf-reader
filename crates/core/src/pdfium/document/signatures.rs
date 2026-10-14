//! Firmas manuscritas en la página: una anotación de sello (/Stamp) cuya
//! apariencia son los trazados de la firma (`sign::Shape`). Se guardan con
//! la actualización incremental de PDFium, sin tocar lo que ya había.
//!
//! Mientras se colocan, las firmas solo están en `placed_signatures` (la
//! interfaz las dibuja encima de la página) y se pueden mover o quitar. Al
//! guardar, imprimir o exportar se escriben en el PDF (`commit_signatures`):
//! hacerlo en cada movimiento dejaba en el fichero una copia por cada uno,
//! porque PDFium conserva los objetos de las anotaciones quitadas.

use super::{Document, sys, to_wide};
use crate::sign::{SavedSignature, Segment, Shape, ShapePath};
use anyhow::{Result, bail};
use std::ffi::c_int;

/// Una firma puesta y aún no escrita en el PDF (se puede mover o quitar).
#[derive(Debug, Clone)]
pub struct PlacedSignature {
    pub page: usize,
    /// Izquierda, abajo, derecha, arriba, en puntos de página.
    pub rect: [f32; 4],
    pub signature: SavedSignature,
}

/// Tamaño con que se pone una firma de entrada, en puntos.
pub fn signature_size(sig: &SavedSignature) -> (f32, f32) {
    let shape = Shape::of(sig);
    let (max_w, max_h) = if sig.is_initials() { (70.0, 36.0) } else { (170.0, 50.0) };
    let s = (max_w / shape.width).min(max_h / shape.height);
    (shape.width * s, shape.height * s)
}

impl Document {
    /// Se pueden poner firmas: el guardado conserva las anotaciones de
    /// PDFium. No en XFA dinámico (se guarda a partir de la plantilla, y
    /// Adobe no muestra anotaciones en esos formularios).
    pub fn can_sign(&self) -> bool {
        !(self.xfa_loaded && (self.compat.is_some() || self.kind == super::FormKind::XfaFull))
    }

    pub fn placed_signatures(&self) -> &[PlacedSignature] {
        &self.signatures
    }

    /// Pone `sig` en la página, en `rect` (izquierda, abajo, derecha, arriba).
    /// Devuelve su posición en `placed_signatures`.
    pub fn add_signature(&mut self, page: usize, rect: [f32; 4], sig: &SavedSignature) -> Result<usize> {
        if !self.can_sign() {
            bail!("este documento no admite firmas sobre la página");
        }
        if page >= self.page_count() {
            bail!("no existe la página {}", page + 1);
        }
        self.signatures.push(PlacedSignature { page, rect, signature: sig.clone() });
        Ok(self.signatures.len() - 1)
    }

    /// Rectángulo para poner `sig` centrada en (x, y) de la página, sin
    /// salirse de ella.
    pub fn signature_rect_at(&self, page: usize, x: f32, y: f32, sig: &SavedSignature) -> [f32; 4] {
        let (w, h) = signature_size(sig);
        self.clamp_to_page(page, [x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0])
    }

    /// `r` movido para que quede dentro de la página.
    pub fn clamp_to_page(&self, page: usize, r: [f32; 4]) -> [f32; 4] {
        let (pw, ph) = self.page_size(page);
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        let dx = (-r[0]).max(0.0) - (r[2] - pw).max(0.0);
        let dy = (-r[1]).max(0.0) - (r[3] - ph).max(0.0);
        [r[0] + dx, r[1] + dy, r[0] + dx + w, r[1] + dy + h]
    }

    /// Mueve o cambia de tamaño una firma aún no escrita.
    pub fn move_signature(&mut self, i: usize, rect: [f32; 4]) -> Result<()> {
        let Some(p) = self.signatures.get_mut(i) else { bail!("no hay esa firma") };
        p.rect = rect;
        Ok(())
    }

    /// Quita una firma aún no escrita.
    pub fn remove_signature(&mut self, i: usize) {
        if i < self.signatures.len() {
            self.signatures.remove(i);
        }
    }

    /// Escribe en el PDF las firmas puestas (antes de guardar, imprimir o
    /// exportar). Desde entonces son parte de la página.
    pub fn commit_signatures(&mut self) -> Result<()> {
        for p in std::mem::take(&mut self.signatures) {
            self.create_stamp(&p)?;
        }
        Ok(())
    }

    fn create_stamp(&mut self, p: &PlacedSignature) -> Result<()> {
        let page = self.page(p.page);
        if page.is_null() {
            bail!("no existe la página {}", p.page + 1);
        }
        let [left, bottom, right, top] = p.rect;
        unsafe {
            let annot = sys::FPDFPage_CreateAnnot(page, sys::FPDF_ANNOT_STAMP as c_int);
            if annot.is_null() {
                bail!("PDFium no pudo crear la anotación");
            }
            let r = sys::FS_RECTF { left, top, right, bottom };
            sys::FPDFAnnot_SetRect(annot, &r);
            let (paths, _) = Shape::of(&p.signature).fit(left, bottom, right - left, top - bottom);
            for path in &paths {
                if let Some(obj) = path_object(path, p.signature.color) {
                    sys::FPDFAnnot_AppendObject(annot, obj);
                }
            }
            sys::FPDFAnnot_SetFlags(annot, sys::FPDF_ANNOT_FLAG_PRINT as c_int);
            self.sig_counter += 1;
            let nm = to_wide(&format!("pdfre-firma-{}-{}", std::process::id(), self.sig_counter));
            sys::FPDFAnnot_SetStringValue(annot, c"NM".as_ptr(), nm.as_ptr());
            sys::FPDFPage_CloseAnnot(annot);
        }
        Ok(())
    }
}

/// Un trazado de la firma como objeto de página de PDFium.
unsafe fn path_object(path: &ShapePath, [r, g, b]: [u8; 3]) -> Option<sys::FPDF_PAGEOBJECT> {
    let Some(Segment::MoveTo(x, y)) = path.segments.first() else { return None };
    unsafe {
        let obj = sys::FPDFPageObj_CreateNewPath(*x, *y);
        if obj.is_null() {
            return None;
        }
        for seg in &path.segments[1..] {
            match *seg {
                Segment::MoveTo(x, y) => sys::FPDFPath_MoveTo(obj, x, y),
                Segment::LineTo(x, y) => sys::FPDFPath_LineTo(obj, x, y),
                Segment::CubicTo(a, b, c, d, e, f) => sys::FPDFPath_BezierTo(obj, a, b, c, d, e, f),
                Segment::Close => sys::FPDFPath_Close(obj),
            };
        }
        let (r, g, b) = (r as u32, g as u32, b as u32);
        match path.stroke {
            None => {
                sys::FPDFPageObj_SetFillColor(obj, r, g, b, 255);
                sys::FPDFPath_SetDrawMode(obj, sys::FPDF_FILLMODE_WINDING as c_int, 0);
            }
            Some(w) => {
                sys::FPDFPageObj_SetStrokeColor(obj, r, g, b, 255);
                sys::FPDFPageObj_SetStrokeWidth(obj, w);
                sys::FPDFPageObj_SetLineCap(obj, sys::FPDF_LINECAP_ROUND as c_int);
                sys::FPDFPageObj_SetLineJoin(obj, sys::FPDF_LINEJOIN_ROUND as c_int);
                sys::FPDFPath_SetDrawMode(obj, sys::FPDF_FILLMODE_NONE as c_int, 1);
            }
        }
        Some(obj)
    }
}

/// Imagen de la firma para mostrarla en la interfaz: RGBA con fondo
/// transparente, `height` píxeles de alto. Devuelve (ancho, alto, píxeles).
pub fn render_signature(sig: &SavedSignature, height: i32) -> Option<(i32, i32, Vec<u8>)> {
    super::init();
    let shape = Shape::of(sig);
    let h = height.max(1);
    let w = ((shape.width / shape.height) * h as f32).ceil().max(1.0) as i32;
    unsafe {
        let doc = sys::FPDF_CreateNewDocument();
        if doc.is_null() {
            return None;
        }
        let page = sys::FPDFPage_New(doc, 0, shape.width as f64, shape.height as f64);
        let mut out = None;
        if !page.is_null() {
            let (paths, _) = shape.fit(0.0, 0.0, shape.width, shape.height);
            for path in &paths {
                if let Some(obj) = path_object(path, sig.color) {
                    sys::FPDFPage_InsertObject(page, obj);
                }
            }
            sys::FPDFPage_GenerateContent(page);
            let bmp = sys::FPDFBitmap_Create(w, h, 1);
            if !bmp.is_null() {
                sys::FPDFBitmap_FillRect(bmp, 0, 0, w, h, 0x0000_0000);
                sys::FPDF_RenderPageBitmap(bmp, page, 0, 0, w, h, 0, sys::FPDF_REVERSE_BYTE_ORDER as c_int);
                let stride = sys::FPDFBitmap_GetStride(bmp) as usize;
                let src = sys::FPDFBitmap_GetBuffer(bmp) as *const u8;
                let mut px = Vec::with_capacity((w * h * 4) as usize);
                for row in 0..h as usize {
                    px.extend_from_slice(std::slice::from_raw_parts(src.add(row * stride), w as usize * 4));
                }
                sys::FPDFBitmap_Destroy(bmp);
                out = Some((w, h, px));
            }
            sys::FPDF_ClosePage(page);
        }
        sys::FPDF_CloseDocument(doc);
        out
    }
}
