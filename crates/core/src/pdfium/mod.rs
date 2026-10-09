//! Capa segura sobre PDFium.
//!
//! PDFium no es thread-safe y el isolate de V8 queda ligado al hilo que llama a
//! `init()`. Todo el uso de este módulo debe hacerse desde un único hilo (el
//! hilo principal de la interfaz).

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unsafe_op_in_unsafe_fn,
    clippy::all
)]
pub mod sys;

mod document;
mod fonts;
mod host;
pub mod keys;

pub use document::{AlertAnswer, AlertButtons, AlertIcon, AlertRequest, Document, FormKind, OpenOptions, SearchHit, Updates, write_atomic};

use std::sync::Once;

static INIT: Once = Once::new();

/// Inicializa PDFium (y V8) una sola vez por proceso.
pub fn init() {
    INIT.call_once(|| unsafe {
        let mut cfg: sys::FPDF_LIBRARY_CONFIG = std::mem::zeroed();
        cfg.version = 2;
        sys::FPDF_InitLibraryWithConfig(&cfg);
        fonts::install();
    });
}

/// Cadena UTF-16LE terminada en cero, como la espera PDFium (FPDF_WIDESTRING).
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Lee una FPDF_WIDESTRING terminada en cero.
///
/// # Safety
/// `p` debe ser nulo o apuntar a una cadena UTF-16 terminada en cero.
pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

/// Convierte un buffer UTF-16 devuelto por PDFium (puede incluir el cero final).
pub fn from_wide_buf(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Escribe `s` como UTF-16LE terminado en cero en un buffer de PDFium y
/// devuelve el número de bytes necesarios (convención habitual de PDFium).
///
/// # Safety
/// `buf` debe ser nulo o apuntar a `len` bytes escribibles.
pub unsafe fn write_wide_out(s: &str, buf: *mut std::ffi::c_void, len: i32) -> i32 {
    let w = to_wide(s);
    let bytes = (w.len() * 2) as i32;
    if !buf.is_null() && len >= bytes {
        unsafe { std::ptr::copy_nonoverlapping(w.as_ptr() as *const u8, buf as *mut u8, bytes as usize) };
    }
    bytes
}

/// Descripción legible de FPDF_GetLastError().
pub fn last_error_message() -> String {
    let code = unsafe { sys::FPDF_GetLastError() } as u32;
    match code {
        sys::FPDF_ERR_SUCCESS => crate::t!("pdfium.ok"),
        sys::FPDF_ERR_FILE => crate::t!("pdfium.file"),
        sys::FPDF_ERR_FORMAT => crate::t!("pdfium.format"),
        sys::FPDF_ERR_PASSWORD => crate::t!("pdfium.password"),
        sys::FPDF_ERR_SECURITY => crate::t!("pdfium.security"),
        sys::FPDF_ERR_PAGE => crate::t!("pdfium.page"),
        7 => crate::t!("pdfium.xfa_load"),
        8 => crate::t!("pdfium.xfa_layout"),
        other => crate::t!("pdfium.unknown", other),
    }
}
