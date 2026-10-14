//! Impresión en Windows: las impresoras del sistema (winspool) y las hojas
//! dibujadas en la impresora con GDI, a su resolución (hasta 300 ppp).
//!
//! Lo elegido en el diálogo se escribe en el DEVMODE de la impresora
//! (copias, color, doble cara, orientación de cada hoja), y el botón de
//! preferencias abre el diálogo propio del controlador para lo demás
//! (papel, bandeja, calidad…).

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

use form_pdf_reader::pdfium::{Document, Sheet};
use form_pdf_reader::t;

use super::{Duplex, Options, PRINT_DPI, Printer};

/// Hay diálogo de preferencias del controlador.
pub const HAS_PROPERTIES: bool = true;

type Handle = isize;

#[repr(C)]
struct PrinterInfo4 {
    name: *const u16,
    server: *const u16,
    attributes: u32,
}

#[repr(C)]
struct DocInfo {
    size: i32,
    doc_name: *const u16,
    output: *const u16,
    datatype: *const u16,
    flags: u32,
}

#[repr(C)]
struct BitmapInfo {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    clr_used: u32,
    clr_important: u32,
    colors: [u32; 1],
}

#[link(name = "winspool")]
unsafe extern "system" {
    fn EnumPrintersW(flags: u32, name: *const u16, level: u32, buf: *mut u8, size: u32, needed: *mut u32, returned: *mut u32) -> i32;
    fn GetDefaultPrinterW(buf: *mut u16, size: *mut u32) -> i32;
    fn OpenPrinterW(name: *const u16, printer: *mut Handle, defaults: *const c_void) -> i32;
    fn ClosePrinter(printer: Handle) -> i32;
    fn DocumentPropertiesW(hwnd: isize, printer: Handle, name: *const u16, out: *mut u8, input: *const u8, mode: u32) -> i32;
    fn DeviceCapabilitiesW(device: *const u16, port: *const u16, capability: u16, output: *mut u16, devmode: *const u8) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateDCW(driver: *const u16, device: *const u16, port: *const u16, devmode: *const u8) -> Handle;
    fn DeleteDC(hdc: Handle) -> i32;
    fn ResetDCW(hdc: Handle, devmode: *const u8) -> Handle;
    fn StartDocW(hdc: Handle, info: *const DocInfo) -> i32;
    fn EndDoc(hdc: Handle) -> i32;
    fn AbortDoc(hdc: Handle) -> i32;
    fn StartPage(hdc: Handle) -> i32;
    fn EndPage(hdc: Handle) -> i32;
    fn GetDeviceCaps(hdc: Handle, index: i32) -> i32;
    fn SetStretchBltMode(hdc: Handle, mode: i32) -> i32;
    fn SetBrushOrgEx(hdc: Handle, x: i32, y: i32, old: *mut [i32; 2]) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn StretchDIBits(hdc: Handle, xd: i32, yd: i32, wd: i32, hd: i32, xs: i32, ys: i32, ws: i32, hs: i32, bits: *const c_void, info: *const BitmapInfo, usage: u32, rop: u32) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

const PRINTER_ENUM_LOCAL: u32 = 0x2;
const PRINTER_ENUM_CONNECTIONS: u32 = 0x4;
const DC_DUPLEX: u16 = 7;
const DC_COPIES: u16 = 18;
const DC_COLORDEVICE: u16 = 32;
const DM_OUT_BUFFER: u32 = 2;
const DM_IN_PROMPT: u32 = 4;
const DM_IN_BUFFER: u32 = 8;
const IDOK: i32 = 1;
const HORZRES: i32 = 8;
const VERTRES: i32 = 10;
const LOGPIXELSX: i32 = 88;
const LOGPIXELSY: i32 = 90;
const HALFTONE: i32 = 4;
const SRCCOPY: u32 = 0x00CC_0020;

// DEVMODEW: posiciones de los campos que se tocan.
const DM_FIELDS: usize = 72;
const DM_ORIENTATION_AT: usize = 76;
const DM_COPIES_AT: usize = 86;
const DM_COLOR_AT: usize = 92;
const DM_DUPLEX_AT: usize = 94;
const DM_COLLATE_AT: usize = 100;
const DM_ORIENTATION: u32 = 0x0001;
const DM_COPIES: u32 = 0x0100;
const DM_COLOR: u32 = 0x0800;
const DM_DUPLEX: u32 = 0x1000;
const DM_COLLATE: u32 = 0x8000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

fn get16(dm: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([dm[at], dm[at + 1]])
}

fn set16(dm: &mut [u8], at: usize, v: i16) {
    dm[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn set_field(dm: &mut [u8], at: usize, flag: u32, v: i16) {
    set16(dm, at, v);
    let f = u32::from_le_bytes(dm[DM_FIELDS..DM_FIELDS + 4].try_into().expect("4 bytes")) | flag;
    dm[DM_FIELDS..DM_FIELDS + 4].copy_from_slice(&f.to_le_bytes());
}

pub fn list_printers() -> Result<Vec<Printer>, String> {
    let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
    let (mut needed, mut count) = (0u32, 0u32);
    unsafe {
        EnumPrintersW(flags, null(), 4, null_mut(), 0, &mut needed, &mut count);
        if needed == 0 {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        if EnumPrintersW(flags, null(), 4, buf.as_mut_ptr() as *mut u8, needed, &mut needed, &mut count) == 0 {
            return Err(t!("print.win_failed", GetLastError()));
        }
        let infos = std::slice::from_raw_parts(buf.as_ptr() as *const PrinterInfo4, count as usize);
        let default = default_printer();
        let mut list: Vec<Printer> = infos
            .iter()
            .map(|i| {
                let name = from_wide(i.name);
                let w = wide(&name);
                let color = DeviceCapabilitiesW(w.as_ptr(), null(), DC_COLORDEVICE, null_mut(), null()) == 1;
                let duplex = DeviceCapabilitiesW(w.as_ptr(), null(), DC_DUPLEX, null_mut(), null()) == 1;
                Printer { is_default: default.as_deref() == Some(name.as_str()), label: name.clone(), name, color, duplex }
            })
            .collect();
        list.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.label.to_lowercase().cmp(&b.label.to_lowercase())));
        Ok(list)
    }
}

fn default_printer() -> Option<String> {
    let mut size = 0u32;
    unsafe {
        GetDefaultPrinterW(null_mut(), &mut size);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u16; size as usize];
        (GetDefaultPrinterW(buf.as_mut_ptr(), &mut size) != 0).then(|| from_wide(buf.as_ptr()))
    }
}

/// El DEVMODE de la impresora: el guardado (si lo hay) validado por el
/// controlador, o el predeterminado. Con `prompt`, abre antes el diálogo
/// del controlador (None si se cancela).
fn devmode(printer: &str, base: Option<&[u8]>, prompt: bool) -> Option<Vec<u8>> {
    let name = wide(printer);
    unsafe {
        let mut h: Handle = 0;
        if OpenPrinterW(name.as_ptr(), &mut h, null()) == 0 {
            return None;
        }
        let size = DocumentPropertiesW(0, h, name.as_ptr(), null_mut(), null(), 0);
        let result = (|| {
            if size <= 0 {
                return None;
            }
            let mut out = vec![0u8; size as usize];
            let input = base.filter(|b| b.len() == out.len());
            let mut mode = DM_OUT_BUFFER;
            if input.is_some() {
                mode |= DM_IN_BUFFER;
            }
            if prompt {
                mode |= DM_IN_PROMPT;
            }
            let hwnd = if prompt { crate::snap::hwnd() } else { 0 };
            let r = DocumentPropertiesW(hwnd, h, name.as_ptr(), out.as_mut_ptr(), input.map_or(null(), |b| b.as_ptr()), mode);
            if r < 0 || (prompt && r != IDOK) {
                return None;
            }
            Some(out)
        })();
        ClosePrinter(h);
        result
    }
}

/// Ajustes del controlador elegidos en su diálogo, por impresora (mientras
/// la aplicación está abierta).
#[derive(Default)]
pub struct DriverSettings {
    by_printer: HashMap<String, Vec<u8>>,
}

/// Lo elegido en el diálogo del controlador, para enseñarlo en el nuestro.
pub struct DriverChoice {
    pub copies: u32,
    pub color: bool,
    pub duplex: Duplex,
}

impl DriverSettings {
    /// Abre las preferencias de la impresora.
    pub fn properties(&mut self, printer: &str) -> Option<DriverChoice> {
        let dm = devmode(printer, self.by_printer.get(printer).map(Vec::as_slice), true)?;
        let choice = DriverChoice {
            copies: get16(&dm, DM_COPIES_AT).max(1) as u32,
            color: get16(&dm, DM_COLOR_AT) != 1,
            duplex: match get16(&dm, DM_DUPLEX_AT) {
                2 => Duplex::LongEdge,
                3 => Duplex::ShortEdge,
                _ => Duplex::Off,
            },
        };
        self.by_printer.insert(printer.to_string(), dm);
        Some(choice)
    }
}

/// Trabajo de impresión abierto en la impresora.
pub struct Spool {
    hdc: Handle,
    dm: Option<Vec<u8>>,
    /// Orientación de la última hoja (1 vertical, 2 apaisada).
    orientation: i16,
    driver_copies: bool,
}

impl Spool {
    pub fn start(opts: &Options, driver: &mut DriverSettings) -> Result<Spool, String> {
        let name = wide(&opts.printer);
        let copies_max = unsafe { DeviceCapabilitiesW(name.as_ptr(), null(), DC_COPIES, null_mut(), null()) };
        let driver_copies = copies_max >= opts.copies as i32;
        let mut dm = devmode(&opts.printer, driver.by_printer.get(&opts.printer).map(Vec::as_slice), false);
        if let Some(dm) = dm.as_mut() {
            set_field(dm, DM_COPIES_AT, DM_COPIES, if driver_copies { opts.copies as i16 } else { 1 });
            set_field(dm, DM_COLLATE_AT, DM_COLLATE, 1);
            set_field(dm, DM_COLOR_AT, DM_COLOR, if opts.color { 2 } else { 1 });
            let duplex = match opts.duplex {
                Duplex::Off => 1,
                Duplex::LongEdge => 2,
                Duplex::ShortEdge => 3,
            };
            set_field(dm, DM_DUPLEX_AT, DM_DUPLEX, duplex);
        }
        let driver_name = wide("WINSPOOL");
        let hdc = unsafe { CreateDCW(driver_name.as_ptr(), name.as_ptr(), null(), dm.as_ref().map_or(null(), |d| d.as_ptr())) };
        if hdc == 0 {
            return Err(t!("print.win_failed", unsafe { GetLastError() }));
        }
        let title = wide(&opts.title);
        let info = DocInfo { size: size_of::<DocInfo>() as i32, doc_name: title.as_ptr(), output: null(), datatype: null(), flags: 0 };
        if unsafe { StartDocW(hdc, &info) } <= 0 {
            let err = unsafe { GetLastError() };
            unsafe { DeleteDC(hdc) };
            // 1223: el usuario canceló (p. ej. el "Guardar como" de Microsoft Print to PDF).
            return Err(if err == 1223 { t!("print.cancelled") } else { t!("print.win_failed", err) });
        }
        Ok(Spool { hdc, dm, orientation: 0, driver_copies })
    }

    pub fn handles_copies(&self) -> bool {
        self.driver_copies
    }

    pub fn add(&mut self, doc: &Document, sheet: &Sheet, gray: bool) -> Result<(), String> {
        let hdc = self.hdc;
        // Cada hoja con su orientación.
        let orientation = if sheet.width > sheet.height { 2 } else { 1 };
        if orientation != self.orientation
            && let Some(dm) = self.dm.as_mut()
        {
            set_field(dm, DM_ORIENTATION_AT, DM_ORIENTATION, orientation);
            unsafe { ResetDCW(hdc, dm.as_ptr()) };
            self.orientation = orientation;
        }
        unsafe {
            if StartPage(hdc) <= 0 {
                return Err(t!("print.win_failed", GetLastError()));
            }
            // Zona imprimible y resolución de la impresora.
            let (aw, ah) = (GetDeviceCaps(hdc, HORZRES) as f32, GetDeviceCaps(hdc, VERTRES) as f32);
            let (dx, dy) = (GetDeviceCaps(hdc, LOGPIXELSX) as f32, GetDeviceCaps(hdc, LOGPIXELSY) as f32);
            // A tamaño real, o más pequeña si no cabe entre los márgenes.
            let (sw, sh) = (sheet.width / 72.0 * dx, sheet.height / 72.0 * dy);
            let k = (aw / sw).min(ah / sh).min(1.0);
            let (dw, dh) = ((sw * k).round() as i32, (sh * k).round() as i32);
            let (x, y) = (((aw - dw as f32) / 2.0).round() as i32, ((ah - dh as f32) / 2.0).round() as i32);
            let dpi = (dx * k).min(PRINT_DPI);
            let (w, h, mut px) = super::render(doc, sheet, dpi, gray).ok_or_else(|| t!("err.rasterize", 1))?;
            // RGBA → BGRA.
            for p in px.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
            SetStretchBltMode(hdc, HALFTONE);
            SetBrushOrgEx(hdc, 0, 0, null_mut());
            // En franjas: algunos controladores no aceptan imágenes muy grandes.
            const BAND: i32 = 512;
            let mut top = 0;
            while top < h {
                let rows = BAND.min(h - top);
                let y0 = y + (top as i64 * dh as i64 / h as i64) as i32;
                let y1 = y + ((top + rows) as i64 * dh as i64 / h as i64) as i32;
                let info = BitmapInfo {
                    size: 40,
                    width: w,
                    height: -rows,
                    planes: 1,
                    bit_count: 32,
                    compression: 0,
                    size_image: 0,
                    x_ppm: 0,
                    y_ppm: 0,
                    clr_used: 0,
                    clr_important: 0,
                    colors: [0],
                };
                let bits = px.as_ptr().add(top as usize * w as usize * 4) as *const c_void;
                if StretchDIBits(hdc, x, y0, dw, y1 - y0, 0, 0, w, rows, bits, &info, 0, SRCCOPY) == 0 {
                    let err = GetLastError();
                    EndPage(hdc);
                    return Err(t!("print.win_failed", err));
                }
                top += rows;
            }
            if EndPage(hdc) <= 0 {
                return Err(t!("print.win_failed", GetLastError()));
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Result<(), String> {
        let ok = unsafe { EndDoc(self.hdc) } > 0;
        let err = unsafe { GetLastError() };
        unsafe { DeleteDC(self.hdc) };
        if ok { Ok(()) } else { Err(t!("print.win_failed", err)) }
    }

    pub fn abort(self) {
        unsafe {
            AbortDoc(self.hdc);
            DeleteDC(self.hdc);
        }
    }
}
