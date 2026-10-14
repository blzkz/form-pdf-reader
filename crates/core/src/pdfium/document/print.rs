//! Imprimir y exportar en plano: el documento se convierte en hojas, cada
//! una una imagen de la página tal como se ve (con los datos de los
//! formularios y las firmas). Las XFA dinámicas en vista continua se trocean
//! en hojas A4.
//!
//! La impresión va hoja a hoja (`render_sheet`) para que la interfaz pueda
//! enseñar el progreso; `FlatPdf` junta las hojas en un PDF (para CUPS, para
//! Android o para "Exportar PDF plano").

use super::{Document, Writer, sys, write_atomic};
use anyhow::{Result, bail};
use std::ffi::c_int;
use std::path::Path;

/// Una hoja impresa: un trozo de una página del documento (la página entera
/// salvo en las XFA continuas).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sheet {
    /// Página de origen.
    pub page: usize,
    /// Franja de la página, en puntos desde arriba.
    pub y0: f32,
    pub y1: f32,
    /// Tamaño de la hoja, en puntos.
    pub width: f32,
    pub height: f32,
}

impl Document {
    /// Prepara el documento para imprimirlo o exportarlo en plano: escribe
    /// las firmas puestas y devuelve sus hojas.
    pub fn print_sheets(&mut self) -> Result<Vec<Sheet>> {
        self.commit_signatures()?;
        Ok(self.print_layout())
    }

    /// Las hojas en que se imprimiría, sin escribir aún las firmas puestas
    /// (para contarlas o enseñar una vista previa).
    pub fn print_layout(&self) -> Vec<Sheet> {
        let mut v = Vec::new();
        for page in 0..self.page_count() {
            let (w, h) = self.page_size(page);
            if !self.is_continuous() {
                v.push(Sheet { page, y0: 0.0, y1: h, width: w, height: h });
                continue;
            }
            // Vista continua: trocear en hojas con proporción A4, cortando
            // por una franja en blanco para no partir campos ni líneas.
            let end = self.content_height(page);
            let sheet_h = w * 297.0 / 210.0;
            let mut y0 = 0.0f32;
            while y0 < end - 1.0 {
                let mut y1 = (y0 + sheet_h).min(end);
                if y1 < end {
                    y1 = self.find_blank_row(page, w, h, y1, sheet_h * 0.3).unwrap_or(y1);
                }
                v.push(Sheet { page, y0, y1, width: w, height: sheet_h });
                y0 = y1;
            }
        }
        v
    }

    /// La hoja a `dpi`: (ancho, alto, RGBA), con fondo blanco donde la
    /// página no llega.
    pub fn render_sheet(&self, s: &Sheet, dpi: f32) -> Option<(i32, i32, Vec<u8>)> {
        let k = dpi / 72.0;
        let (pw, ph) = self.page_size(s.page);
        let (full_w, full_h) = ((pw * k).round() as i32, (ph * k).round() as i32);
        let (bw, bh) = ((s.width * k).round() as i32, (s.height * k).round() as i32);
        let (ry, rh) = ((s.y0 * k).round() as i32, ((s.y1 - s.y0) * k).round().min(bh as f32) as i32);
        let cw = full_w.min(bw);
        let src = self.render_region_ex(s.page, full_w, full_h, 0, ry, cw, rh, true)?;
        if (cw, rh) == (bw, bh) {
            return Some((bw, bh, src));
        }
        let mut out = vec![255u8; bw as usize * bh as usize * 4];
        let (cw, row) = (cw as usize * 4, bw as usize * 4);
        for r in 0..rh as usize {
            out[r * row..r * row + cw].copy_from_slice(&src[r * cw..r * cw + cw]);
        }
        Some((bw, bh, out))
    }

    /// Crea un PDF "plano" (cada hoja es una imagen) a `dpi`. Sirve para
    /// imprimir o enviar el formulario a quien no tenga un visor XFA. En vista
    /// continua se trocea en hojas A4.
    pub fn export_flat(&mut self, path: &Path, dpi: f32) -> Result<()> {
        let sheets = self.print_sheets()?;
        let mut pdf = FlatPdf::new()?;
        for (n, s) in sheets.iter().enumerate() {
            let (w, h, rgba) = self.render_sheet(s, dpi).ok_or_else(|| anyhow::anyhow!("{}", crate::t!("err.rasterize", n + 1)))?;
            pdf.add(s.width, s.height, w, h, &rgba)?;
        }
        pdf.save(path)
    }
}

/// Un PDF nuevo hecho de imágenes, una por hoja.
pub struct FlatPdf {
    doc: sys::FPDF_DOCUMENT,
    pages: c_int,
}

impl FlatPdf {
    pub fn new() -> Result<FlatPdf> {
        crate::pdfium::init();
        let doc = unsafe { sys::FPDF_CreateNewDocument() };
        if doc.is_null() {
            bail!("{}", crate::t!("err.create_output"));
        }
        Ok(FlatPdf { doc, pages: 0 })
    }

    /// Añade una hoja de `width`×`height` puntos con la imagen RGBA `w`×`h`.
    pub fn add(&mut self, width: f32, height: f32, w: i32, h: i32, rgba: &[u8]) -> Result<()> {
        let n = self.pages as usize + 1;
        unsafe {
            let bmp = sys::FPDFBitmap_Create(w, h, 0);
            if bmp.is_null() {
                bail!("{}", crate::t!("err.rasterize_mem", n));
            }
            let stride = sys::FPDFBitmap_GetStride(bmp) as usize;
            let dst = std::slice::from_raw_parts_mut(sys::FPDFBitmap_GetBuffer(bmp) as *mut u8, stride * h as usize);
            let row = w as usize * 4;
            for (r, line) in rgba.chunks_exact(row).take(h as usize).enumerate() {
                for (d, s) in dst[r * stride..r * stride + row].as_chunks_mut::<4>().0.iter_mut().zip(line.as_chunks::<4>().0) {
                    *d = [s[2], s[1], s[0], 255];
                }
            }
            let page = sys::FPDFPage_New(self.doc, self.pages, width as f64, height as f64);
            let img = sys::FPDFPageObj_NewImageObj(self.doc);
            let mut pages = [page];
            let ok = sys::FPDFImageObj_SetBitmap(pages.as_mut_ptr(), 1, img, bmp);
            sys::FPDFBitmap_Destroy(bmp);
            if ok == 0 {
                sys::FPDFPageObj_Destroy(img);
                sys::FPDF_ClosePage(page);
                bail!("{}", crate::t!("err.insert_image", n));
            }
            sys::FPDFImageObj_SetMatrix(img, width as f64, 0.0, 0.0, height as f64, 0.0, 0.0);
            sys::FPDFPage_InsertObject(page, img);
            sys::FPDFPage_GenerateContent(page);
            sys::FPDF_ClosePage(page);
        }
        self.pages += 1;
        Ok(())
    }

    pub fn page_count(&self) -> usize {
        self.pages as usize
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        if unsafe { sys::FPDF_SaveAsCopy(self.doc, &mut w.base, sys::FPDF_NO_INCREMENTAL as sys::FPDF_DWORD) } == 0 {
            bail!("{}", crate::t!("err.serialize_flat"));
        }
        Ok(w.buf)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, &self.to_bytes()?)
    }
}

impl Drop for FlatPdf {
    fn drop(&mut self) {
        unsafe { sys::FPDF_CloseDocument(self.doc) };
    }
}

/// Lee un rango de páginas como "1-3, 5, 8-" (numeradas desde 1) y devuelve
/// los índices (desde 0), en ese orden y sin repetir. `None` si no es
/// válido o se sale de `count`.
pub fn parse_page_ranges(text: &str, count: usize) -> Option<Vec<usize>> {
    let mut out = Vec::new();
    for part in text.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let num = |s: &str| -> Option<usize> { s.trim().parse::<usize>().ok().filter(|n| (1..=count).contains(n)) };
        let (a, b) = match part.split_once(['-', '–']) {
            Some((a, b)) => (if a.trim().is_empty() { 1 } else { num(a)? }, if b.trim().is_empty() { count } else { num(b)? }),
            None => {
                let n = num(part)?;
                (n, n)
            }
        };
        if a > b {
            return None;
        }
        for n in a..=b {
            if !out.contains(&(n - 1)) {
                out.push(n - 1);
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::parse_page_ranges;

    #[test]
    fn rangos_de_paginas() {
        assert_eq!(parse_page_ranges("1-3, 5", 10), Some(vec![0, 1, 2, 4]));
        assert_eq!(parse_page_ranges("8-", 10), Some(vec![7, 8, 9]));
        assert_eq!(parse_page_ranges("-2", 10), Some(vec![0, 1]));
        assert_eq!(parse_page_ranges("3,1,3", 10), Some(vec![2, 0]));
        assert_eq!(parse_page_ranges("2–4", 5), Some(vec![1, 2, 3]));
        assert_eq!(parse_page_ranges("", 10), None);
        assert_eq!(parse_page_ranges("0", 10), None);
        assert_eq!(parse_page_ranges("11", 10), None);
        assert_eq!(parse_page_ranges("4-2", 10), None);
        assert_eq!(parse_page_ranges("a", 10), None);
    }
}
