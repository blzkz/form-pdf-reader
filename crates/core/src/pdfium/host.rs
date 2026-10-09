//! Implementación de FPDF_FORMFILLINFO e IPDF_JSPLATFORM: los callbacks con los
//! que PDFium pide cosas a la aplicación (páginas, timers, repintados, alertas
//! de JavaScript, cambios de página del layout XFA...).
//!
//! Regla de oro: ninguna función de Rust mantiene un `borrow()` de `state`
//! mientras llama a PDFium, porque PDFium puede reentrar en estos callbacks.

use super::{from_wide_ptr, sys, write_wide_out};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ffi::{CStr, c_int, c_void};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

/// Botones de una alerta de JavaScript (app.alert / xfa.host.messageBox).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertButtons {
    Ok,
    OkCancel,
    YesNo,
    YesNoCancel,
}

/// Icono de una alerta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertIcon {
    Error,
    Warning,
    Question,
    Info,
}

#[derive(Debug, Clone)]
pub struct AlertRequest {
    pub title: String,
    pub message: String,
    pub buttons: AlertButtons,
    pub icon: AlertIcon,
}

/// Respuesta a una alerta, con los códigos de Acrobat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertAnswer {
    Ok = 1,
    Cancel = 2,
    No = 3,
    Yes = 4,
}

pub type AlertHandler = Box<dyn FnMut(&AlertRequest) -> AlertAnswer>;
pub type BrowseHandler = Box<dyn FnMut() -> Option<String>>;
/// app.response(pregunta, título, valor por defecto) -> texto o None si se cancela.
pub type ResponseHandler = Box<dyn FnMut(&str, &str, &str) -> Option<String>>;

/// Prefijo de las llamadas del puente de adjuntos (ver xfa::compat).
pub const ATTACH_PREFIX: &str = "PDFRE|";

/// Adjuntos del documento en memoria. Se escriben en el PDF al guardar.
#[derive(Default)]
pub struct AttachmentStore {
    pub items: Vec<crate::xfa::attachments::Attachment>,
    pub dirty: bool,
}

pub struct Timer {
    pub id: i32,
    pub interval: Duration,
    pub next: Instant,
    pub cb: unsafe extern "C" fn(c_int),
}

pub struct HostState {
    pub doc: sys::FPDF_DOCUMENT,
    pub form: sys::FPDF_FORMHANDLE,
    /// Handles de página ya cargados, por índice.
    pub pages: Vec<sys::FPDF_PAGE>,
    pub current_page: i32,
    /// Páginas (handles) que PDFium pide repintar.
    pub invalidated: BTreeSet<usize>,
    pub invalidate_all: bool,
    /// Rectángulos invalidados (página, [x0, y0, x1, y1] en coordenadas de página).
    pub invalid_rects: Vec<(usize, [f64; 4])>,
    pub form_changed: bool,
    pub cursor: i32,
    /// Caret de edición: página y rectángulo en coordenadas de página.
    pub caret: Option<(sys::FPDF_PAGE, [f64; 4])>,
    pub text_field_focused: bool,
    pub timers: Vec<Timer>,
    pub next_timer_id: i32,
    pub layout_changed: bool,
    pub goto_page: Option<i32>,
    pub print_requested: bool,
    pub submit_requested: Option<String>,
    pub uris: Vec<String>,
    /// Ficheros temporales que la interfaz debe abrir (adjuntos a "Ver").
    pub open_files: Vec<std::path::PathBuf>,
    /// Punto (coordenadas de página) del clic de ratón en curso; lo consultan
    /// los scripts de botones convertidos (ver xfa::compat).
    pub click_point: Option<(f64, f64)>,
    /// Botón cuyo click debe ejecutarse (expresión SOM; ver xfa::compat).
    pub pending_click: Option<String>,
    /// Estado del formulario capturado por script al guardar (JSON).
    pub form_state: Option<String>,
    /// Resultado de la última orden "@eval:" (diagnóstico y pruebas).
    pub eval_result: Option<String>,
    /// Tamaño de página (en puntos) por handle, para FFI_GetPageViewRect.
    pub page_sizes: Vec<(sys::FPDF_PAGE, f64, f64)>,
}

impl HostState {
    fn new(doc: sys::FPDF_DOCUMENT) -> Self {
        HostState {
            doc,
            form: null_mut(),
            pages: Vec::new(),
            current_page: 0,
            invalidated: BTreeSet::new(),
            invalidate_all: false,
            invalid_rects: Vec::new(),
            form_changed: false,
            cursor: 0,
            caret: None,
            text_field_focused: false,
            timers: Vec::new(),
            next_timer_id: 1,
            layout_changed: false,
            goto_page: None,
            print_requested: false,
            submit_requested: None,
            uris: Vec::new(),
            open_files: Vec::new(),
            click_point: None,
            pending_click: None,
            form_state: None,
            eval_result: None,
            page_sizes: Vec::new(),
        }
    }

    pub fn index_of(&self, page: sys::FPDF_PAGE) -> Option<usize> {
        self.pages.iter().position(|&p| p == page && !p.is_null())
    }
}

/// Estructura que se pasa a PDFium. `info` DEBE ser el primer campo: los
/// callbacks reciben `*mut FPDF_FORMFILLINFO` y lo convierten en `*FormHost`.
#[repr(C)]
pub struct FormHost {
    pub info: sys::FPDF_FORMFILLINFO,
    pub js: sys::IPDF_JSPLATFORM,
    pub state: RefCell<HostState>,
    pub alert_handler: RefCell<Option<AlertHandler>>,
    pub browse_handler: RefCell<Option<BrowseHandler>>,
    browse_result: RefCell<Option<String>>,
    pub response_handler: RefCell<Option<ResponseHandler>>,
    pub attachments: RefCell<AttachmentStore>,
}

impl FormHost {
    pub fn new(doc: sys::FPDF_DOCUMENT) -> Box<FormHost> {
        let mut host = Box::new(FormHost {
            info: unsafe { std::mem::zeroed() },
            js: unsafe { std::mem::zeroed() },
            state: RefCell::new(HostState::new(doc)),
            alert_handler: RefCell::new(None),
            browse_handler: RefCell::new(None),
            browse_result: RefCell::new(None),
            response_handler: RefCell::new(None),
            attachments: RefCell::new(AttachmentStore::default()),
        });

        let i = &mut host.info;
        i.version = 2; // 2 = soporte XFA
        i.FFI_Invalidate = Some(ffi_invalidate);
        i.FFI_OutputSelectedRect = Some(ffi_output_selected_rect);
        i.FFI_SetCursor = Some(ffi_set_cursor);
        i.FFI_SetTimer = Some(ffi_set_timer);
        i.FFI_KillTimer = Some(ffi_kill_timer);
        i.FFI_GetLocalTime = Some(ffi_get_local_time);
        i.FFI_OnChange = Some(ffi_on_change);
        i.FFI_GetPage = Some(ffi_get_page);
        i.FFI_GetCurrentPage = Some(ffi_get_current_page);
        i.FFI_GetRotation = Some(ffi_get_rotation);
        i.FFI_ExecuteNamedAction = Some(ffi_execute_named_action);
        i.FFI_SetTextFieldFocus = Some(ffi_set_text_field_focus);
        i.FFI_DoURIAction = Some(ffi_do_uri_action);
        i.FFI_DoGoToAction = Some(ffi_do_goto_action);
        i.FFI_DisplayCaret = Some(ffi_display_caret);
        i.FFI_GetCurrentPageIndex = Some(ffi_get_current_page_index);
        i.FFI_SetCurrentPage = Some(ffi_set_current_page);
        i.FFI_GotoURL = Some(ffi_goto_url);
        i.FFI_GetPageViewRect = Some(ffi_get_page_view_rect);
        i.FFI_PageEvent = Some(ffi_page_event);
        i.FFI_PopupMenu = Some(ffi_popup_menu);
        i.FFI_GetPlatform = Some(ffi_get_platform);
        i.FFI_GetLanguage = Some(ffi_get_language);
        i.xfa_disabled = 0;

        let j = &mut host.js;
        j.version = 3;
        j.app_alert = Some(js_app_alert);
        j.app_beep = Some(js_app_beep);
        j.app_response = Some(js_app_response);
        j.Doc_print = Some(js_doc_print);
        j.Doc_submitForm = Some(js_doc_submit_form);
        j.Doc_gotoPage = Some(js_doc_goto_page);
        j.Field_browse = Some(js_field_browse);

        // Punteros cruzados. El Box garantiza que las direcciones no cambian.
        let info_ptr: *mut sys::FPDF_FORMFILLINFO = &mut host.info;
        let js_ptr: *mut sys::IPDF_JSPLATFORM = &mut host.js;
        host.js.m_pFormfillinfo = info_ptr as *mut c_void;
        host.info.m_pJsPlatform = js_ptr;
        host
    }

    /// Devuelve el handle de la página `idx`, cargándola si hace falta.
    pub fn get_page(&self, idx: usize) -> sys::FPDF_PAGE {
        let (doc, form, existing) = {
            let st = self.state.borrow();
            (st.doc, st.form, st.pages.get(idx).copied().unwrap_or(null_mut()))
        };
        if !existing.is_null() {
            return existing;
        }
        let count = unsafe { sys::FPDF_GetPageCount(doc) };
        if idx as i32 >= count {
            return null_mut();
        }
        let page = unsafe { sys::FPDF_LoadPage(doc, idx as c_int) };
        if page.is_null() {
            return page;
        }
        let (w, h) = unsafe { (sys::FPDF_GetPageWidthF(page) as f64, sys::FPDF_GetPageHeightF(page) as f64) };
        {
            let mut st = self.state.borrow_mut();
            // FPDF_LoadPage puede haber reentrado y cargado la página ya.
            if let Some(&p) = st.pages.get(idx)
                && !p.is_null() && p != page {
                    drop(st);
                    unsafe { sys::FPDF_ClosePage(page) };
                    return self.state.borrow().pages[idx];
                }
            if st.pages.len() <= idx {
                st.pages.resize(idx + 1, null_mut());
            }
            st.pages[idx] = page;
            st.page_sizes.retain(|(p, _, _)| *p != page);
            st.page_sizes.push((page, w, h));
        }
        if !form.is_null() {
            unsafe {
                sys::FORM_OnAfterLoadPage(page, form);
                sys::FORM_DoPageAAction(page, form, sys::FPDFPAGE_AACTION_OPEN as c_int);
            }
        }
        page
    }
}

unsafe fn host<'a>(p: *mut sys::FPDF_FORMFILLINFO) -> &'a FormHost {
    unsafe { &*(p as *const FormHost) }
}

unsafe fn host_js<'a>(p: *mut sys::IPDF_JSPLATFORM) -> &'a FormHost {
    unsafe { host((*p).m_pFormfillinfo as *mut sys::FPDF_FORMFILLINFO) }
}

fn mark_invalid(h: &FormHost, page: sys::FPDF_PAGE) {
    let mut st = h.state.borrow_mut();
    match st.index_of(page) {
        Some(i) => {
            st.invalidated.insert(i);
        }
        None => st.invalidate_all = true,
    }
}

// ---------------------------------------------------------------------------
// FPDF_FORMFILLINFO
// ---------------------------------------------------------------------------

unsafe extern "C" fn ffi_invalidate(p: *mut sys::FPDF_FORMFILLINFO, page: sys::FPDF_PAGE, l: f64, t: f64, r: f64, b: f64) {
    unsafe {
        let h = host(p);
        mark_invalid(h, page);
        let mut st = h.state.borrow_mut();
        if let Some(i) = st.index_of(page) {
            st.invalid_rects.push((i, [l.min(r), t.min(b), l.max(r), t.max(b)]));
        }
    }
}

unsafe extern "C" fn ffi_output_selected_rect(p: *mut sys::FPDF_FORMFILLINFO, page: sys::FPDF_PAGE, _l: f64, _t: f64, _r: f64, _b: f64) {
    unsafe { mark_invalid(host(p), page) }
}

unsafe extern "C" fn ffi_set_cursor(p: *mut sys::FPDF_FORMFILLINFO, t: c_int) {
    unsafe { host(p).state.borrow_mut().cursor = t }
}

unsafe extern "C" fn ffi_set_timer(p: *mut sys::FPDF_FORMFILLINFO, elapse: c_int, cb: sys::TimerCallback) -> c_int {
    unsafe {
        let Some(cb) = cb else { return 0 };
        let mut st = host(p).state.borrow_mut();
        let id = st.next_timer_id;
        st.next_timer_id += 1;
        let interval = Duration::from_millis(elapse.max(1) as u64);
        st.timers.push(Timer { id, interval, next: Instant::now() + interval, cb });
        id
    }
}

unsafe extern "C" fn ffi_kill_timer(p: *mut sys::FPDF_FORMFILLINFO, id: c_int) {
    unsafe { host(p).state.borrow_mut().timers.retain(|t| t.id != id) }
}

unsafe extern "C" fn ffi_get_local_time(_p: *mut sys::FPDF_FORMFILLINFO) -> sys::FPDF_SYSTEMTIME {
    unsafe {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let secs = now.as_secs() as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        sys::FPDF_SYSTEMTIME {
            wYear: (tm.tm_year + 1900) as u16,
            wMonth: (tm.tm_mon + 1) as u16,
            wDayOfWeek: tm.tm_wday as u16,
            wDay: tm.tm_mday as u16,
            wHour: tm.tm_hour as u16,
            wMinute: tm.tm_min as u16,
            wSecond: tm.tm_sec as u16,
            wMilliseconds: now.subsec_millis() as u16,
        }
    }
}

unsafe extern "C" fn ffi_on_change(p: *mut sys::FPDF_FORMFILLINFO) {
    unsafe { host(p).state.borrow_mut().form_changed = true }
}

unsafe extern "C" fn ffi_get_page(p: *mut sys::FPDF_FORMFILLINFO, _doc: sys::FPDF_DOCUMENT, idx: c_int) -> sys::FPDF_PAGE {
    if idx < 0 {
        return null_mut();
    }
    unsafe { host(p).get_page(idx as usize) }
}

unsafe extern "C" fn ffi_get_current_page(p: *mut sys::FPDF_FORMFILLINFO, _doc: sys::FPDF_DOCUMENT) -> sys::FPDF_PAGE {
    unsafe {
        let h = host(p);
        let cur = h.state.borrow().current_page.max(0) as usize;
        h.get_page(cur)
    }
}

unsafe extern "C" fn ffi_get_rotation(_p: *mut sys::FPDF_FORMFILLINFO, _page: sys::FPDF_PAGE) -> c_int {
    0
}

unsafe extern "C" fn ffi_execute_named_action(p: *mut sys::FPDF_FORMFILLINFO, name: sys::FPDF_BYTESTRING) {
    unsafe {
        if name.is_null() {
            return;
        }
        let name = CStr::from_ptr(name).to_string_lossy().into_owned();
        let h = host(p);
        let doc = h.state.borrow().doc;
        let count = sys::FPDF_GetPageCount(doc);
        let mut st = h.state.borrow_mut();
        let cur = st.current_page;
        match name.as_str() {
            "Print" => st.print_requested = true,
            "NextPage" => st.goto_page = Some((cur + 1).min(count - 1)),
            "PrevPage" => st.goto_page = Some((cur - 1).max(0)),
            "FirstPage" => st.goto_page = Some(0),
            "LastPage" => st.goto_page = Some(count - 1),
            other => log::info!("Acción con nombre no soportada: {other}"),
        }
    }
}

unsafe extern "C" fn ffi_set_text_field_focus(p: *mut sys::FPDF_FORMFILLINFO, _v: sys::FPDF_WIDESTRING, _len: sys::FPDF_DWORD, focus: sys::FPDF_BOOL) {
    unsafe { host(p).state.borrow_mut().text_field_focused = focus != 0 }
}

unsafe extern "C" fn ffi_do_uri_action(p: *mut sys::FPDF_FORMFILLINFO, uri: sys::FPDF_BYTESTRING) {
    unsafe {
        if uri.is_null() {
            return;
        }
        let s = CStr::from_ptr(uri).to_string_lossy().into_owned();
        host(p).state.borrow_mut().uris.push(s);
    }
}

unsafe extern "C" fn ffi_do_goto_action(p: *mut sys::FPDF_FORMFILLINFO, idx: c_int, _zoom: c_int, _pos: *mut f32, _n: c_int) {
    unsafe { host(p).state.borrow_mut().goto_page = Some(idx) }
}

unsafe extern "C" fn ffi_display_caret(p: *mut sys::FPDF_FORMFILLINFO, page: sys::FPDF_PAGE, visible: sys::FPDF_BOOL, l: f64, t: f64, r: f64, b: f64) {
    unsafe {
        let h = host(p);
        let mut st = h.state.borrow_mut();
        st.caret = if visible != 0 { Some((page, [l, t, r, b])) } else { None };
        if let Some(i) = st.index_of(page) {
            st.invalidated.insert(i);
        }
    }
}

unsafe extern "C" fn ffi_get_current_page_index(p: *mut sys::FPDF_FORMFILLINFO, _doc: sys::FPDF_DOCUMENT) -> c_int {
    unsafe { host(p).state.borrow().current_page }
}

unsafe extern "C" fn ffi_set_current_page(p: *mut sys::FPDF_FORMFILLINFO, _doc: sys::FPDF_DOCUMENT, idx: c_int) {
    unsafe { host(p).state.borrow_mut().goto_page = Some(idx) }
}

unsafe extern "C" fn ffi_goto_url(p: *mut sys::FPDF_FORMFILLINFO, _doc: sys::FPDF_DOCUMENT, url: sys::FPDF_WIDESTRING) {
    unsafe {
        let s = from_wide_ptr(url);
        if !s.is_empty() {
            host(p).state.borrow_mut().uris.push(s);
        }
    }
}

/// Necesario para que se abran los desplegables XFA: PDFium calcula si cabe
/// la lista por debajo o por encima del campo a partir de este rectángulo.
/// Devolvemos la página completa (coordenadas XFA: origen arriba-izquierda).
unsafe extern "C" fn ffi_get_page_view_rect(p: *mut sys::FPDF_FORMFILLINFO, page: sys::FPDF_PAGE, l: *mut f64, t: *mut f64, r: *mut f64, b: *mut f64) {
    unsafe {
        let st = host(p).state.borrow();
        let (w, hgt) = st
            .page_sizes
            .iter()
            .find(|(pp, _, _)| *pp == page)
            .map(|(_, w, h)| (*w, *h))
            .unwrap_or_else(|| (sys::FPDF_GetPageWidthF(page) as f64, sys::FPDF_GetPageHeightF(page) as f64));
        *l = 0.0;
        *t = 0.0;
        *r = w;
        *b = hgt;
    }
}

unsafe extern "C" fn ffi_page_event(p: *mut sys::FPDF_FORMFILLINFO, count: c_int, ev: sys::FPDF_DWORD) {
    log::debug!("FFI_PageEvent count={count} type={ev}");
    unsafe {
        let mut st = host(p).state.borrow_mut();
        st.layout_changed = true;
        st.invalidate_all = true;
    }
}

unsafe extern "C" fn ffi_popup_menu(_p: *mut sys::FPDF_FORMFILLINFO, _page: sys::FPDF_PAGE, _w: sys::FPDF_WIDGET, _flags: c_int, _x: f32, _y: f32) -> sys::FPDF_BOOL {
    0
}

unsafe extern "C" fn ffi_get_platform(_p: *mut sys::FPDF_FORMFILLINFO, buf: *mut c_void, len: c_int) -> c_int {
    unsafe { write_wide_out("UNIX", buf, len) }
}

unsafe extern "C" fn ffi_get_language(_p: *mut sys::FPDF_FORMFILLINFO, buf: *mut c_void, len: c_int) -> c_int {
    unsafe { write_wide_out("es_ES", buf, len) }
}

// ---------------------------------------------------------------------------
// IPDF_JSPLATFORM
// ---------------------------------------------------------------------------

unsafe extern "C" fn js_app_alert(p: *mut sys::IPDF_JSPLATFORM, msg: sys::FPDF_WIDESTRING, title: sys::FPDF_WIDESTRING, ty: c_int, icon: c_int) -> c_int {
    unsafe {
        let h = host_js(p);
        let req = AlertRequest {
            title: from_wide_ptr(title),
            message: from_wide_ptr(msg),
            buttons: match ty as u32 {
                sys::JSPLATFORM_ALERT_BUTTON_OKCANCEL => AlertButtons::OkCancel,
                sys::JSPLATFORM_ALERT_BUTTON_YESNO => AlertButtons::YesNo,
                sys::JSPLATFORM_ALERT_BUTTON_YESNOCANCEL => AlertButtons::YesNoCancel,
                _ => AlertButtons::Ok,
            },
            icon: match icon as u32 {
                sys::JSPLATFORM_ALERT_ICON_ERROR => AlertIcon::Error,
                sys::JSPLATFORM_ALERT_ICON_WARNING => AlertIcon::Warning,
                sys::JSPLATFORM_ALERT_ICON_QUESTION => AlertIcon::Question,
                _ => AlertIcon::Info,
            },
        };
        log::info!("app.alert: {:?}", req);
        // Se saca el handler del RefCell mientras se ejecuta, por si hubiera
        // reentrada (otra alerta durante la primera).
        let handler = h.alert_handler.borrow_mut().take();
        match handler {
            Some(mut f) => {
                let ans = f(&req);
                *h.alert_handler.borrow_mut() = Some(f);
                ans as c_int
            }
            None => match req.buttons {
                AlertButtons::Ok | AlertButtons::OkCancel => AlertAnswer::Ok as c_int,
                _ => AlertAnswer::Yes as c_int,
            },
        }
    }
}

unsafe extern "C" fn js_app_beep(_p: *mut sys::IPDF_JSPLATFORM, _t: c_int) {}

unsafe extern "C" fn js_doc_print(p: *mut sys::IPDF_JSPLATFORM, _ui: sys::FPDF_BOOL, _s: c_int, _e: c_int, _silent: sys::FPDF_BOOL, _shrink: sys::FPDF_BOOL, _img: sys::FPDF_BOOL, _rev: sys::FPDF_BOOL, _annots: sys::FPDF_BOOL) {
    unsafe { host_js(p).state.borrow_mut().print_requested = true }
}

unsafe extern "C" fn js_doc_submit_form(p: *mut sys::IPDF_JSPLATFORM, _data: *mut c_void, _len: c_int, url: sys::FPDF_WIDESTRING) {
    unsafe {
        let u = from_wide_ptr(url);
        host_js(p).state.borrow_mut().submit_requested = Some(u);
    }
}

unsafe extern "C" fn js_doc_goto_page(p: *mut sys::IPDF_JSPLATFORM, n: c_int) {
    unsafe { host_js(p).state.borrow_mut().goto_page = Some(n) }
}

/// Selección de fichero para campos de tipo "file select". PDFium espera la
/// ruta en la codificación del sistema (UTF-8) terminada en cero.
unsafe extern "C" fn js_field_browse(p: *mut sys::IPDF_JSPLATFORM, buf: *mut c_void, len: c_int) -> c_int {
    // PDFium llama dos veces: primero con buffer nulo para saber el tamaño y
    // luego para copiar. El diálogo solo se muestra en la primera llamada.
    unsafe {
        let h = host_js(p);
        if buf.is_null() || h.browse_result.borrow().is_none() {
            let handler = h.browse_handler.borrow_mut().take();
            let Some(mut f) = handler else { return 0 };
            let path = f();
            *h.browse_handler.borrow_mut() = Some(f);
            *h.browse_result.borrow_mut() = path;
        }
        let Some(path) = h.browse_result.borrow().clone() else { return 0 };
        let bytes = path.into_bytes();
        let need = bytes.len() as c_int + 1;
        if !buf.is_null() && len >= need {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, bytes.len());
            *(buf as *mut u8).add(bytes.len()) = 0;
            *h.browse_result.borrow_mut() = None;
        }
        need
    }
}


// ---------------------------------------------------------------------------
// app.response y puente de adjuntos
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn js_app_response(
    p: *mut sys::IPDF_JSPLATFORM,
    question: sys::FPDF_WIDESTRING,
    title: sys::FPDF_WIDESTRING,
    default: sys::FPDF_WIDESTRING,
    _label: sys::FPDF_WIDESTRING,
    _password: sys::FPDF_BOOL,
    response: *mut c_void,
    length: c_int,
) -> c_int {
    unsafe {
        let h = host_js(p);
        let q = from_wide_ptr(question);
        let answer = if let Some(cmd) = q.strip_prefix(ATTACH_PREFIX) {
            Some(attach_command(h, cmd))
        } else {
            let handler = h.response_handler.borrow_mut().take();
            match handler {
                Some(mut f) => {
                    let r = f(&q, &from_wide_ptr(title), &from_wide_ptr(default));
                    *h.response_handler.borrow_mut() = Some(f);
                    r
                }
                None => None,
            }
        };
        let Some(text) = answer else { return 0 };
        let bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let n = bytes.len().min(length.max(0) as usize) & !1;
        if !response.is_null() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), response as *mut u8, n);
        }
        n as c_int
    }
}

fn describe(a: &crate::xfa::attachments::Attachment) -> String {
    format!("{}|{}|{}", a.name, a.data.len(), a.file_name)
}

/// Órdenes del puente de adjuntos. Respuestas: "nombre|tamaño|fichero".
fn attach_command(h: &FormHost, cmd: &str) -> String {
    let (verb, arg) = cmd.split_once('|').unwrap_or((cmd, ""));
    log::debug!("adjuntos: {verb} {}", arg.chars().take(200).collect::<String>());
    match verb {
        // El script comprueba que el punto cae dentro de su propio botón: al
        // dar el foco, PDFium puede pasar antes por otros botones.
        "CLICKING" => h.state.borrow().click_point.map(|(x, y)| format!("{x}|{y}")).unwrap_or_default(),
        "CANDIDATE" => {
            h.state.borrow_mut().pending_click = Some(arg.to_string());
            String::new()
        }
        "PENDING" => h.state.borrow_mut().pending_click.take().unwrap_or_default(),
        "LOG" => {
            log::debug!("script: {arg}");
            String::new()
        }
        "EVAL" => {
            log::debug!("eval: {arg}");
            h.state.borrow_mut().eval_result = Some(arg.to_string());
            String::new()
        }
        "STATE" => {
            log::debug!("estado del formulario capturado: {} bytes", arg.len());
            h.state.borrow_mut().form_state = Some(arg.to_string());
            String::new()
        }
        "COUNT" => h.attachments.borrow().items.len().to_string(),
        "ITEM" => {
            let i: usize = arg.parse().unwrap_or(usize::MAX);
            h.attachments.borrow().items.get(i).map(describe).unwrap_or_default()
        }
        "GET" => h.attachments.borrow().items.iter().find(|a| a.name == arg).map(describe).unwrap_or_default(),
        "IMPORT" => {
            let handler = h.browse_handler.borrow_mut().take();
            let Some(mut f) = handler else { return String::new() };
            let picked = f();
            *h.browse_handler.borrow_mut() = Some(f);
            let Some(path) = picked else { return String::new() };
            let path = std::path::PathBuf::from(path);
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("No se pudo leer {}: {e}", path.display());
                    return String::new();
                }
            };
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| arg.to_string());
            let a = crate::xfa::attachments::Attachment { name: arg.to_string(), file_name, data };
            let r = describe(&a);
            let mut st = h.attachments.borrow_mut();
            st.items.retain(|x| x.name != arg);
            st.items.push(a);
            st.dirty = true;
            drop(st);
            h.state.borrow_mut().form_changed = true;
            r
        }
        "REMOVE" => {
            let mut st = h.attachments.borrow_mut();
            let before = st.items.len();
            st.items.retain(|x| x.name != arg);
            let removed = st.items.len() != before;
            st.dirty |= removed;
            drop(st);
            if removed {
                h.state.borrow_mut().form_changed = true;
            }
            "ok".into()
        }
        "OPEN" => {
            let item = h.attachments.borrow().items.iter().find(|a| a.name == arg).cloned();
            if let Some(a) = item {
                let dir = std::env::temp_dir().join("form-pdf-reader").join("adjuntos");
                let _ = std::fs::create_dir_all(&dir);
                let safe: String = a.file_name.chars().map(|c| if c == '/' || c == '\\' { '_' } else { c }).collect();
                let path = dir.join(safe);
                if std::fs::write(&path, &a.data).is_ok() {
                    h.state.borrow_mut().open_files.push(path);
                }
            }
            "ok".into()
        }
        _ => String::new(),
    }
}
