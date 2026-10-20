//! Organizar las páginas de un PDF: girar, borrar, mover, insertar las de
//! otro PDF y extraer unas cuantas a un PDF nuevo.
//!
//! Trabajan sobre los bytes del documento (los de `Document::bytes_for_save`,
//! con los datos del formulario y las anotaciones) y devuelven los bytes
//! nuevos, que la interfaz vuelve a abrir: así ninguna página abierta queda
//! apuntando a una que ya no existe. Los cambios se añaden como actualización
//! incremental (se conserva el cifrado del original). No sirven para los
//! formularios XFA, cuyas páginas las genera el formulario.

use super::{PasswordRequired, init, last_error_message, sys};
use anyhow::{Result, bail};
use std::ffi::{c_int, c_ulong, c_void};

unsafe extern "C" {
    // fpdf_ppo.h (no está en sys.rs).
    fn FPDF_ImportPagesByIndex(dest: sys::FPDF_DOCUMENT, src: sys::FPDF_DOCUMENT, page_indices: *const c_int, length: c_ulong, index: c_int) -> sys::FPDF_BOOL;
    fn FPDF_CopyViewerPreferences(dest: sys::FPDF_DOCUMENT, src: sys::FPDF_DOCUMENT) -> sys::FPDF_BOOL;
}

/// Un cambio en las páginas. Los números de página empiezan en 0 y se
/// refieren al documento tal como queda tras los cambios anteriores.
#[derive(Debug, Clone, PartialEq)]
pub enum PageEdit {
    /// Girar 90° por cada cuarto de vuelta (positivo: a la derecha).
    Rotate { pages: Vec<usize>, quarter_turns: i32 },
    Delete { pages: Vec<usize> },
    /// Mover las páginas (en ese orden) para que la primera quede en `to`
    /// (posición contada sin ellas).
    Move { pages: Vec<usize>, to: usize },
    /// Insertar todas las páginas de otro PDF delante de la página `at`
    /// (`at` = número de páginas: al final).
    Insert { pdf: Vec<u8>, password: Option<String>, at: usize },
}

/// Un PDF abierto desde unos bytes, que viven tanto como él.
struct MemDoc<'a> {
    doc: sys::FPDF_DOCUMENT,
    _bytes: &'a [u8],
}

impl<'a> MemDoc<'a> {
    fn open(bytes: &'a [u8], password: Option<&str>) -> Result<MemDoc<'a>> {
        init();
        let pw = password.filter(|p| !p.is_empty()).map(|p| std::ffi::CString::new(p).unwrap_or_default());
        let doc = unsafe { sys::FPDF_LoadMemDocument64(bytes.as_ptr() as *const c_void, bytes.len(), pw.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())) };
        if doc.is_null() {
            if unsafe { sys::FPDF_GetLastError() } as u32 == sys::FPDF_ERR_PASSWORD {
                return Err(PasswordRequired { wrong: pw.is_some() }.into());
            }
            bail!("{}", crate::t!("err.open_pdf", last_error_message()));
        }
        Ok(MemDoc { doc, _bytes: bytes })
    }

    fn page_count(&self) -> usize {
        unsafe { sys::FPDF_GetPageCount(self.doc).max(0) as usize }
    }

    fn save(&self, incremental: bool) -> Result<Vec<u8>> {
        save_doc(self.doc, incremental)
    }
}

impl Drop for MemDoc<'_> {
    fn drop(&mut self) {
        unsafe { sys::FPDF_CloseDocument(self.doc) }
    }
}

/// Guarda un documento abierto de PDFium en bytes.
pub(crate) fn save_doc(doc: sys::FPDF_DOCUMENT, incremental: bool) -> Result<Vec<u8>> {
    let mut w = super::document::Writer::new();
    let flags = if incremental { sys::FPDF_INCREMENTAL } else { sys::FPDF_NO_INCREMENTAL };
    if unsafe { sys::FPDF_SaveAsCopy(doc, &mut w.base, flags as sys::FPDF_DWORD) } == 0 {
        bail!("{}", crate::t!("err.serialize"));
    }
    Ok(w.buf)
}

/// Número de páginas de un PDF (para comprobar uno que se va a insertar:
/// falla con `PasswordRequired` si pide contraseña).
pub fn page_count(bytes: &[u8], password: Option<&str>) -> Result<usize> {
    Ok(MemDoc::open(bytes, password)?.page_count())
}

fn check_pages(pages: &[usize], n: usize) -> Result<Vec<c_int>> {
    if pages.is_empty() {
        bail!("no hay páginas elegidas");
    }
    if let Some(&p) = pages.iter().find(|&&p| p >= n) {
        bail!("no existe la página {}", p + 1);
    }
    Ok(pages.iter().map(|&p| p as c_int).collect())
}

/// Aplica los cambios y devuelve el PDF resultante.
pub fn edit_pages(bytes: &[u8], password: Option<&str>, edits: &[PageEdit]) -> Result<Vec<u8>> {
    let d = MemDoc::open(bytes, password)?;
    for e in edits {
        let n = d.page_count();
        match e {
            PageEdit::Rotate { pages, quarter_turns } => {
                for &p in &check_pages(pages, n)? {
                    unsafe {
                        let page = sys::FPDF_LoadPage(d.doc, p);
                        if page.is_null() {
                            bail!("no se pudo leer la página {}", p + 1);
                        }
                        let r = (sys::FPDFPage_GetRotation(page) + quarter_turns).rem_euclid(4);
                        sys::FPDFPage_SetRotation(page, r);
                        sys::FPDF_ClosePage(page);
                    }
                }
            }
            PageEdit::Delete { pages } => {
                let mut idx = check_pages(pages, n)?;
                if idx.len() >= n {
                    bail!("{}", crate::t!("pages.delete_all"));
                }
                idx.sort_unstable_by(|a, b| b.cmp(a));
                idx.dedup();
                for p in idx {
                    unsafe { sys::FPDFPage_Delete(d.doc, p) };
                }
            }
            PageEdit::Move { pages, to } => {
                let idx = check_pages(pages, n)?;
                let to = (*to).min(n - idx.len());
                if unsafe { sys::FPDF_MovePages(d.doc, idx.as_ptr(), idx.len() as c_ulong, to as c_int) } == 0 {
                    bail!("PDFium no pudo mover las páginas");
                }
            }
            PageEdit::Insert { pdf, password, at } => {
                let src = MemDoc::open(pdf, password.as_deref())?;
                let at = (*at).min(n);
                if unsafe { FPDF_ImportPagesByIndex(d.doc, src.doc, std::ptr::null(), 0, at as c_int) } == 0 {
                    bail!("PDFium no pudo insertar las páginas: {}", last_error_message());
                }
            }
        }
    }
    d.save(true)
}

/// Un PDF nuevo con esas páginas (en ese orden).
pub fn extract_pages(bytes: &[u8], password: Option<&str>, pages: &[usize]) -> Result<Vec<u8>> {
    let src = MemDoc::open(bytes, password)?;
    let idx = check_pages(pages, src.page_count())?;
    unsafe {
        let dest = sys::FPDF_CreateNewDocument();
        if dest.is_null() {
            bail!("PDFium no pudo crear el documento");
        }
        let ok = FPDF_ImportPagesByIndex(dest, src.doc, idx.as_ptr(), idx.len() as c_ulong, 0) != 0;
        FPDF_CopyViewerPreferences(dest, src.doc);
        let out = if ok { save_doc(dest, false) } else { Err(anyhow::anyhow!("PDFium no pudo copiar las páginas")) };
        sys::FPDF_CloseDocument(dest);
        out
    }
}

/// Giro de cada página (0 a 3 cuartos de vuelta), para las miniaturas.
pub fn rotations(bytes: &[u8], password: Option<&str>) -> Result<Vec<i32>> {
    let d = MemDoc::open(bytes, password)?;
    Ok((0..d.page_count() as c_int)
        .map(|p| unsafe {
            let page = sys::FPDF_LoadPage(d.doc, p);
            if page.is_null() {
                return 0;
            }
            let r = sys::FPDFPage_GetRotation(page);
            sys::FPDF_ClosePage(page);
            r
        })
        .collect())
}
