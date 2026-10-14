//! Windows: el marco de la ventana sin la barra de título del sistema.
//!
//! - Los bordes de redimensionar de izquierda, derecha y abajo siguen siendo
//!   del sistema (arriba llega nuestra barra), como en Edge o Chrome: Windows
//!   redimensiona por ellos, dibuja la sombra y redondea las esquinas.
//! - El menú de diseños de ajuste (snap layouts) al dejar el ratón sobre
//!   nuestro botón de maximizar: Windows lo muestra si la ventana le dice que
//!   el ratón está ahí (WM_NCHITTEST → HTMAXBUTTON) y la ventana tiene marco
//!   del sistema (sin él no aparecía). Esa zona pasa a ser "no cliente" y
//!   egui no recibe allí el ratón, así que aquí se lleva la cuenta de si está
//!   encima y se maximiza al soltar.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

type Hwnd = isize;

const GWLP_WNDPROC: i32 = -4;
const WM_NCCALCSIZE: u32 = 0x0083;
const WM_NCHITTEST: u32 = 0x0084;
const WM_NCLBUTTONDOWN: u32 = 0x00A1;
const WM_NCLBUTTONUP: u32 = 0x00A2;
const WM_NCLBUTTONDBLCLK: u32 = 0x00A3;
const WM_NCMOUSELEAVE: u32 = 0x02A2;
const HTMAXBUTTON: isize = 9;
const SW_MAXIMIZE: i32 = 3;
const SW_RESTORE: i32 = 9;
const SM_CXSIZEFRAME: i32 = 32;
const SM_CXPADDEDBORDER: i32 = 92;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWCP_ROUND: u32 = 2;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct WinRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn CallWindowProcW(prev: isize, hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn ScreenToClient(hwnd: Hwnd, point: *mut Point) -> i32;
    fn IsZoomed(hwnd: Hwnd) -> i32;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: Hwnd, attribute: u32, value: *const u32, size: u32) -> i32;
}

static OLD_PROC: AtomicIsize = AtomicIsize::new(0);
static CTX: OnceLock<egui::Context> = OnceLock::new();
/// Botón de maximizar en píxeles de la ventana: izquierda, arriba, derecha, abajo.
static BUTTON: Mutex<Option<[f32; 4]>> = Mutex::new(None);
static HOVER: AtomicBool = AtomicBool::new(false);

/// Engancha el procedimiento de la ventana. Una sola ventana por proceso.
pub fn install(window: &impl HasWindowHandle, ctx: &egui::Context) {
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
    if CTX.set(ctx.clone()).is_err() {
        return;
    }
    let hwnd = h.hwnd.get();
    let old = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc as WndProc as usize as isize) };
    OLD_PROC.store(old, Ordering::SeqCst);
    unsafe {
        // Esquinas redondeadas de Windows 11 (sin efecto en Windows 10).
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND, size_of::<u32>() as u32);
        // Que Windows vuelva a calcular el marco (WM_NCCALCSIZE).
        const SWP_FRAMECHANGED: u32 = 0x0020;
        const SWP_NOMOVE_NOSIZE_NOZORDER_NOACTIVATE: u32 = 0x0002 | 0x0001 | 0x0004 | 0x0010;
        SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE_NOSIZE_NOZORDER_NOACTIVATE);
    }
}

/// Dónde está el botón de maximizar este fotograma (en puntos de egui).
pub fn set_max_button(rect: egui::Rect, pixels_per_point: f32) {
    let px = [rect.min.x, rect.min.y, rect.max.x, rect.max.y].map(|v| v * pixels_per_point);
    if let Ok(mut b) = BUTTON.lock() {
        *b = Some(px);
    }
}

/// El ratón está sobre el botón de maximizar (egui no lo ve).
pub fn max_hovered() -> bool {
    HOVER.load(Ordering::Relaxed)
}

fn set_hover(on: bool) {
    if HOVER.swap(on, Ordering::Relaxed) != on
        && let Some(ctx) = CTX.get()
    {
        ctx.request_repaint();
    }
}

fn over_button(hwnd: Hwnd, lparam: isize) -> bool {
    let mut p = Point { x: (lparam & 0xFFFF) as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as i16 as i32 };
    unsafe { ScreenToClient(hwnd, &mut p) };
    let Ok(b) = BUTTON.lock() else { return false };
    b.is_some_and(|[l, t, r, bt]| (p.x as f32) >= l && (p.x as f32) < r && (p.y as f32) >= t && (p.y as f32) < bt)
}

unsafe extern "system" fn proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    let old = OLD_PROC.load(Ordering::SeqCst);
    match msg {
        // Sin maximizar: bordes del sistema salvo arriba. Maximizada, lo
        // hace winit (la ventana se ajusta a la pantalla).
        WM_NCCALCSIZE if wparam != 0 && unsafe { IsZoomed(hwnd) } == 0 => {
            let frame = unsafe { GetSystemMetrics(SM_CXSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER) };
            // El primer rectángulo de NCCALCSIZE_PARAMS: el de la ventana,
            // que pasa a ser el del área cliente.
            let r = unsafe { &mut *(lparam as *mut WinRect) };
            r.left += frame;
            r.right -= frame;
            r.bottom -= frame;
            return 0;
        }
        // Windows pregunta en cada movimiento qué hay bajo el ratón: así se
        // sabe también cuándo deja el botón.
        WM_NCHITTEST => {
            let on = over_button(hwnd, lparam);
            set_hover(on);
            if on {
                return HTMAXBUTTON;
            }
        }
        WM_NCMOUSELEAVE => set_hover(false),
        // Pulsar no hace nada (sin esto Windows dibuja su botón clásico).
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if wparam as isize == HTMAXBUTTON => return 0,
        WM_NCLBUTTONUP if wparam as isize == HTMAXBUTTON => {
            unsafe { ShowWindow(hwnd, if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE }) };
            set_hover(false);
            return 0;
        }
        _ => {}
    }
    unsafe { CallWindowProcW(old, hwnd, msg, wparam, lparam) }
}
