//! Documento PDF abierto con su entorno de formularios (AcroForm o XFA).

use super::host::{AlertHandler, BrowseHandler, FormHost, ResponseHandler};
pub use crate::xfa::attachments::Attachment;
use super::{from_wide_buf, init, last_error_message, sys, to_wide};
use anyhow::{Context, Result, bail};
use crate::xfa::{self, CompatReport};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_int, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

pub use super::host::{AlertAnswer, AlertButtons, AlertIcon, AlertRequest};

/// Tipo de formulario que contiene el documento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormKind {
    None,
    AcroForm,
    /// XFA completo (dinámico): el contenido de las páginas lo genera XFA.
    XfaFull,
    /// XFA "foreground" (estático): páginas PDF normales con campos XFA.
    XfaForeground,
}

/// Cambios pendientes que la interfaz debe aplicar tras llamar a PDFium.
#[derive(Debug, Default)]
pub struct Updates {
    pub invalidated: Vec<usize>,
    pub invalidate_all: bool,
    pub form_changed: bool,
    pub layout_changed: bool,
    pub cursor: i32,
    /// Caret visible: página y rectángulo (izq, arriba, der, abajo) en coordenadas de página.
    pub caret: Option<(usize, [f64; 4])>,
    pub text_field_focused: bool,
    pub goto_page: Option<usize>,
    pub print_requested: bool,
    pub submit_requested: Option<String>,
    pub uris: Vec<String>,
    /// Ficheros (adjuntos) que el usuario ha pedido ver.
    pub open_files: Vec<PathBuf>,
}

/// Resultado de búsqueda: página y rectángulos en coordenadas de página.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub page: usize,
    pub rects: Vec<[f64; 4]>,
}

/// Opciones de apertura.
#[derive(Debug, Clone, Copy)]
pub struct OpenOptions {
    /// Aplicar la vista continua a los XFA dinámicos (recomendado: el motor de
    /// paginación de PDFium falla con muchos formularios).
    pub continuous_xfa: bool,
}

impl Default for OpenOptions {
    fn default() -> Self {
        OpenOptions { continuous_xfa: true }
    }
}

/// Factor para conservar precisión subpíxel en FPDF_DeviceToPage (que usa enteros).
const SUBPIXEL: f64 = 16.0;

pub struct Document {
    doc: sys::FPDF_DOCUMENT,
    form: sys::FPDF_FORMHANDLE,
    host: Box<FormHost>,
    kind: FormKind,
    xfa_loaded: bool,
    path: PathBuf,
    focus_page: Option<usize>,
    /// Punto al que se redirigió la última pulsación (clic en una etiqueta).
    press_redirect: Option<(f64, f64)>,
    /// La pulsación ya se resolvió (click ejecutado por script): ignorar la suelta.
    skip_up: bool,
    /// La pulsación en curso es sobre un botón.
    pressed_button: bool,
    /// Último campo enfocado con el ratón: (página, x, y, tipo).
    last_focus: Option<(usize, f64, f64, i32)>,
    /// El último refresco no pudo devolver el foco a un campo de texto.
    text_focus_lost: bool,
    /// Altura de contenido conocida, para detectar cambios de estructura.
    known_extent: Option<f32>,
    /// Botón del ratón pulsado (entre left_down y left_up).
    button_held: bool,
    /// Lista desplegable abierta: (página, zona del desplegable + su lista).
    open_list: Option<(usize, [f64; 4])>,
    /// Bytes del fichero tal como está en disco (base para guardar).
    base: Vec<u8>,
    compat: Option<CompatReport>,
    extent_cache: RefCell<HashMap<usize, f32>>,
    /// Bytes entregados a PDFium. PDFium los lee de forma perezosa: deben vivir tanto
    /// como `doc`. Se declara el último para que se libere después.
    _data: Box<[u8]>,
}

impl Document {
    /// Abre un PDF. El fichero se lee entero a memoria, así se puede
    /// sobrescribir al guardar sin corromper el documento abierto.
    pub fn open(path: &Path) -> Result<Document> {
        Self::open_with(path, OpenOptions::default())
    }

    pub fn open_with(path: &Path, opts: OpenOptions) -> Result<Document> {
        let data = std::fs::read(path).with_context(|| crate::t!("err.read", path.display()))?;
        Self::from_bytes(data, path.to_path_buf(), opts)
    }

    pub fn from_bytes(base: Vec<u8>, path: PathBuf, opts: OpenOptions) -> Result<Document> {
        init();
        let prepared = xfa::prepare(&base, opts.continuous_xfa);
        let compat = prepared.compat;
        let data: Box<[u8]> = prepared.render_bytes.into_boxed_slice();
        let doc = unsafe { sys::FPDF_LoadMemDocument64(data.as_ptr() as *const c_void, data.len(), std::ptr::null()) };
        if doc.is_null() {
            bail!("{}", crate::t!("err.open_pdf", last_error_message()));
        }
        let kind = match unsafe { sys::FPDF_GetFormType(doc) } as u32 {
            sys::FORMTYPE_ACRO_FORM => FormKind::AcroForm,
            sys::FORMTYPE_XFA_FULL => FormKind::XfaFull,
            sys::FORMTYPE_XFA_FOREGROUND => FormKind::XfaForeground,
            _ => FormKind::None,
        };

        let mut host = FormHost::new(doc);
        let form = unsafe { sys::FPDFDOC_InitFormFillEnvironment(doc, &mut host.info) };
        host.state.borrow_mut().form = form;

        let mut xfa_loaded = false;
        if matches!(kind, FormKind::XfaFull | FormKind::XfaForeground) {
            xfa_loaded = unsafe { sys::FPDF_LoadXFA(doc) } != 0;
            if !xfa_loaded {
                log::warn!("FPDF_LoadXFA falló: {}", last_error_message());
            }
        }

        if !form.is_null() {
            unsafe {
                // Resaltado suave de campos AcroForm, como en otros visores.
                sys::FPDF_SetFormFieldHighlightColor(form, 0, 0x00DDE4FF);
                sys::FPDF_SetFormFieldHighlightAlpha(form, 90);
                sys::FORM_DoDocumentJSAction(form);
                sys::FORM_DoDocumentOpenAction(form);
            }
        }

        let mut d = Document {
            doc,
            form,
            host,
            kind,
            xfa_loaded,
            path,
            focus_page: None,
            press_redirect: None,
            skip_up: false,
            pressed_button: false,
            last_focus: None,
            text_focus_lost: false,
            known_extent: None,
            button_held: false,
            open_list: None,
            base,
            compat,
            extent_cache: RefCell::new(HashMap::new()),
            _data: data,
        };
        // Adjuntos ya incrustados en el PDF.
        match crate::xfa::attachments::load(&d.base) {
            Ok(items) => d.host.attachments.borrow_mut().items = items,
            Err(e) => log::warn!("No se pudieron leer los adjuntos: {e:#}"),
        }
        // Recolocar los controles una vez maquetado todo (sin esto algunos
        // campos de texto salen partidos en varias líneas estrechas) y fijar
        // la altura de referencia para detectar cambios de estructura.
        if d.compat.as_ref().is_some_and(|c| c.refresh_button) {
            // (El estado del formulario de la última sesión ya se restauró en
            // el evento "ready" del formulario; ver xfa::formstate.)
            d.refresh_widgets();
        }
        // Descartar cambios "fantasma" producidos por los scripts de inicio.
        d.host.state.borrow_mut().form_changed = false;
        Ok(d)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> FormKind {
        self.kind
    }

    /// Correcciones de compatibilidad XFA aplicadas (si las hay).
    pub fn compat(&self) -> Option<&CompatReport> {
        self.compat.as_ref()
    }

    /// true si se usa la vista continua (una sola página alta).
    pub fn is_continuous(&self) -> bool {
        self.compat.as_ref().is_some_and(|c| c.continuous_layout)
    }

    /// Altura útil (en puntos) de la página: en vista continua, hasta el
    /// final del contenido; en el resto, la altura de la página.
    pub fn content_height(&self, idx: usize) -> f32 {
        let (w, h) = self.page_size(idx);
        if !self.is_continuous() {
            return h;
        }
        if let Some(v) = self.extent_cache.borrow().get(&idx) {
            return *v;
        }
        // Render a baja resolución y buscar la última fila no blanca.
        let scale = 0.25f32;
        let (pw, ph) = ((w * scale).max(1.0) as i32, (h * scale).max(1.0) as i32);
        let mut extent = h;
        if let Some(px) = self.render(idx, pw, ph) {
            let row = pw as usize * 4;
            let last = (0..ph as usize).rev().find(|&y| px[y * row..(y + 1) * row].chunks(4).any(|c| c[0] < 245 || c[1] < 245 || c[2] < 245));
            if let Some(y) = last {
                extent = ((y as f32 + 1.0) / scale + 30.0).min(h);
            }
        }
        self.extent_cache.borrow_mut().insert(idx, extent);
        extent
    }

    /// true si el formulario XFA se cargó y PDFium lo está ejecutando.
    pub fn is_xfa(&self) -> bool {
        self.xfa_loaded
    }

    /// true si las páginas las genera XFA (sin contenido PDF propio útil).
    pub fn is_dynamic_xfa(&self) -> bool {
        self.xfa_loaded && self.kind == FormKind::XfaFull
    }

    pub fn page_count(&self) -> usize {
        unsafe { sys::FPDF_GetPageCount(self.doc).max(0) as usize }
    }

    /// Handle de página (carga perezosa, cacheado en el host).
    pub fn page(&self, idx: usize) -> sys::FPDF_PAGE {
        self.host.get_page(idx)
    }

    /// Tamaño de página en puntos (1/72 pulgadas).
    pub fn page_size(&self, idx: usize) -> (f32, f32) {
        if self.xfa_loaded {
            let p = self.page(idx);
            if p.is_null() {
                return (612.0, 792.0);
            }
            return unsafe { (sys::FPDF_GetPageWidthF(p), sys::FPDF_GetPageHeightF(p)) };
        }
        let mut s = sys::FS_SIZEF { width: 612.0, height: 792.0 };
        unsafe { sys::FPDF_GetPageSizeByIndexF(self.doc, idx as c_int, &mut s) };
        (s.width, s.height)
    }

    pub fn set_alert_handler(&self, h: AlertHandler) {
        *self.host.alert_handler.borrow_mut() = Some(h);
    }

    pub fn set_browse_handler(&self, h: BrowseHandler) {
        *self.host.browse_handler.borrow_mut() = Some(h);
    }

    /// Diálogo para `app.response()` de los formularios.
    pub fn set_response_handler(&self, h: ResponseHandler) {
        *self.host.response_handler.borrow_mut() = Some(h);
    }

    /// Adjuntos incrustados (incluidos los añadidos y aún sin guardar).
    pub fn attachments(&self) -> Vec<Attachment> {
        self.host.attachments.borrow().items.clone()
    }

    /// true si los adjuntos han cambiado desde la última vez que se guardó.
    pub fn attachments_dirty(&self) -> bool {
        self.host.attachments.borrow().dirty
    }

    /// true si el formulario usa el componente de adjuntos (puente activo).
    pub fn has_attachment_bridge(&self) -> bool {
        self.compat.as_ref().is_some_and(|c| c.attachment_bridge)
    }

    pub fn set_current_page(&self, idx: usize) {
        self.host.state.borrow_mut().current_page = idx as i32;
    }

    // -----------------------------------------------------------------------
    // Renderizado
    // -----------------------------------------------------------------------

    /// Renderiza la página completa a `w`×`h` píxeles. Devuelve RGBA.
    pub fn render(&self, idx: usize, w: i32, h: i32) -> Option<Vec<u8>> {
        self.render_region(idx, w, h, 0, 0, w, h)
    }

    /// Renderiza solo la región (`x`,`y`,`rw`,`rh`) de la página escalada a
    /// `w`×`h`. Devuelve RGBA de `rw`×`rh`.
    #[allow(clippy::too_many_arguments)]
    pub fn render_region(&self, idx: usize, w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32) -> Option<Vec<u8>> {
        self.render_region_ex(idx, w, h, x, y, rw, rh, false)
    }

    /// Como `render_region`, con la opción de renderizar en modo impresión
    /// (oculta los elementos marcados como "no imprimir", p. ej. botones).
    #[allow(clippy::too_many_arguments)]
    pub fn render_region_ex(&self, idx: usize, w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, printing: bool) -> Option<Vec<u8>> {
        if rw <= 0 || rh <= 0 {
            return None;
        }
        let page = self.page(idx);
        if page.is_null() {
            return None;
        }
        unsafe {
            let bmp = sys::FPDFBitmap_CreateEx(rw, rh, sys::FPDFBitmap_BGRA as c_int, null_mut(), 0);
            if bmp.is_null() {
                return None;
            }
            sys::FPDFBitmap_FillRect(bmp, 0, 0, rw, rh, 0xFFFF_FFFF);
            let mut flags = (sys::FPDF_ANNOT | sys::FPDF_REVERSE_BYTE_ORDER) as c_int;
            if printing {
                flags |= sys::FPDF_PRINTING as c_int;
            }
            // En XFA dinámico el contenido PDF de la página es el aviso de
            // "necesita Adobe Reader": todo lo útil lo dibuja FFLDraw.
            if !self.is_dynamic_xfa() {
                sys::FPDF_RenderPageBitmap(bmp, page, -x, -y, w, h, 0, flags);
            }
            if !self.form.is_null() {
                sys::FPDF_FFLDraw(self.form, bmp, page, -x, -y, w, h, 0, flags);
            }
            let stride = sys::FPDFBitmap_GetStride(bmp) as usize;
            let src = sys::FPDFBitmap_GetBuffer(bmp) as *const u8;
            let mut out = Vec::with_capacity((rw * rh * 4) as usize);
            for row in 0..rh as usize {
                let line = std::slice::from_raw_parts(src.add(row * stride), rw as usize * 4);
                out.extend_from_slice(line);
            }
            sys::FPDFBitmap_Destroy(bmp);
            Some(out)
        }
    }

    // -----------------------------------------------------------------------
    // Coordenadas
    // -----------------------------------------------------------------------

    /// Píxel del dispositivo (página renderizada a `w`×`h`) -> coordenadas de página.
    pub fn device_to_page(&self, idx: usize, w: f64, h: f64, dx: f64, dy: f64) -> (f64, f64) {
        let page = self.page(idx);
        let (mut px, mut py) = (0.0, 0.0);
        unsafe {
            sys::FPDF_DeviceToPage(
                page,
                0,
                0,
                (w * SUBPIXEL) as c_int,
                (h * SUBPIXEL) as c_int,
                0,
                (dx * SUBPIXEL) as c_int,
                (dy * SUBPIXEL) as c_int,
                &mut px,
                &mut py,
            )
        };
        (px, py)
    }

    /// Coordenadas de página -> píxel del dispositivo (página a `w`×`h`).
    pub fn page_to_device(&self, idx: usize, w: f64, h: f64, px: f64, py: f64) -> (f64, f64) {
        let page = self.page(idx);
        let (mut dx, mut dy) = (0, 0);
        unsafe {
            sys::FPDF_PageToDevice(page, 0, 0, (w * SUBPIXEL) as c_int, (h * SUBPIXEL) as c_int, 0, px, py, &mut dx, &mut dy)
        };
        (dx as f64 / SUBPIXEL, dy as f64 / SUBPIXEL)
    }

    // -----------------------------------------------------------------------
    // Eventos de formulario
    // -----------------------------------------------------------------------

    pub fn mouse_move(&mut self, idx: usize, px: f64, py: f64, mods: i32) -> bool {
        let p = self.page(idx);
        unsafe { sys::FORM_OnMouseMove(self.form, p, mods, px, py) != 0 }
    }

    pub fn left_down(&mut self, idx: usize, px: f64, py: f64, mods: i32) -> bool {
        self.button_held = true;
        // Lista desplegable abierta y clic fuera de ella: PDFium la cierra mal
        // (sigue dibujando las opciones sin fondo). Se cierra antes con Escape,
        // que sí la cierra bien.
        let mut was_open_here = false;
        if let Some((li, r)) = self.open_list.take() {
            let inside = li == idx && px >= r[0] && px <= r[2] && py >= r[1] && py <= r[3];
            log::debug!("lista abierta en {r:?}; clic ({px},{py}) dentro={inside}");
            if inside {
                was_open_here = true;
            } else if let Some(lp) = Some(self.page(li)).filter(|p| !p.is_null()) {
                unsafe {
                    sys::FORM_OnKeyDown(self.form, lp, super::keys::VK_ESCAPE, 0);
                    sys::FORM_OnKeyUp(self.form, lp, super::keys::VK_ESCAPE, 0);
                }
            }
        }
        // Clic sobre una opción de la lista abierta: va directo a la lista.
        // Nada de FORM_OnFocus ni redirecciones: el punto cae encima de otro
        // campo (el que tapa la lista) y se le daría el foco a él.
        if was_open_here {
            self.press_redirect = None;
            self.pressed_button = false;
            let p = self.page(idx);
            return unsafe { sys::FORM_OnLButtonDown(self.form, p, mods, px, py) != 0 };
        }
        // ¿Puede este clic abrir una lista? Se compara el aspecto de la zona
        // antes y después para conocer el rectángulo exacto de la lista.
        let (tx, tyy) = self.click_target(idx, px, py);
        let ty = self.field_at(idx, tx, tyy);
        // También los campos de fecha (tipo 15 en XFA): su calendario se
        // comporta como una lista desplegable. Para no confundirlo con el
        // cambio de foco de un campo de texto se exige más altura.
        let probe = matches!(ty, 4 | 10 | 15) && !was_open_here;
        let min_h = if ty == 15 { 60.0 } else { 8.0 };
        let before = if probe { self.strip(idx, py) } else { None };
        // Borde inferior del campo, medido antes del clic: con la lista
        // abierta, PDFium la considera parte del campo.
        let mut bottom = tyy;
        if probe {
            while bottom < tyy + 80.0 && self.field_at(idx, tx, bottom + 0.5) == ty {
                bottom += 0.5;
            }
        }
        let handled = self.left_down_inner(idx, px, py, mods);
        if let Some(b) = before
            && let Some(after) = self.strip(idx, py) {
                // Solo cuenta como lista lo que cambia por debajo del campo: al
                // enfocar un desplegable sin abrirlo también cambian su
                // etiqueta (negrita) y su fondo, y eso no es una lista.
                let below = diff_bbox(&b, &after, bottom + 1.0);
                let opened = below.is_some_and(|r| r[3] - r[1] > min_h);
                self.open_list = diff_bbox(&b, &after, 0.0).filter(|_| opened).map(|r| (idx, [r[0] - 2.0, r[1] - 2.0, r[2] + 2.0, r[3] + 2.0]));
                log::debug!("lista abierta: {:?} (cambios bajo el campo: {below:?})", self.open_list);
            }
        handled
    }

    /// Franja de la página (±320 pt alrededor de `y`) renderizada a 1 px/pt.
    fn strip(&self, idx: usize, y: f64) -> Option<Strip> {
        let (w, h) = self.page_size(idx);
        let top = (y - 320.0).max(0.0) as i32;
        let bottom = ((y + 320.0) as i32).min(h as i32);
        let px = self.render_region(idx, w as i32, h as i32, 0, top, w as i32, bottom - top)?;
        Some(Strip { top, width: w as i32, px })
    }

    /// true si (probablemente) hay una lista desplegable abierta.
    pub fn list_open(&self) -> bool {
        self.open_list.is_some()
    }

    fn left_down_inner(&mut self, idx: usize, px: f64, py: f64, mods: i32) -> bool {
        self.set_focus_page(idx);
        let p = self.page(idx);
        self.press_redirect = None;
        let (x, y) = self.click_target(idx, px, py);
        if (x, y) != (px, py) {
            self.press_redirect = Some((x, y));
        }
        unsafe {
            // FORM_OnFocus es necesario para que PDFium enfoque bien el campo
            // XFA, pero solo sobre la zona de valor (ver click_target).
            let ty = self.field_at(idx, x, y);
            let is_button = matches!(ty, 1 | 13);
            log::debug!("left_down ({x:.0},{y:.0}) tipo={ty}");
            if is_button {
                // Quitar antes el foco para que el evento "enter" del botón se
                // dispare en cada clic (ver xfa::compat::open_readonly_buttons).
                sys::FORM_ForceToKillFocus(self.form);
                self.host.state.borrow_mut().click_point = Some((x, y));
            }
            if ty >= 0 {
                sys::FORM_OnFocus(self.form, p, mods, x, y);
            }
            self.host.state.borrow_mut().click_point = None;
            // ¿Un botón que PDFium no deja pulsar se ha apuntado? Ejecutar su
            // click con el botón invisible de órdenes y no enviar el clic.
            if is_button && self.host.state.borrow().pending_click.is_some() {
                let som = self.host.state.borrow_mut().pending_click.take().unwrap();
                self.run_command(&som);
                self.run_command("@relayout");
                self.press_redirect = None;
                self.skip_up = true;
                return true;
            }
            self.pressed_button = is_button;
            // Ojo: en campos de texto XFA, OnLButtonDown devuelve false aunque
            // el clic funcione; no sirve para saber si se enfocó.
            let handled = sys::FORM_OnLButtonDown(self.form, p, mods, x, y) != 0;
            self.last_focus = (ty >= 0).then_some((idx, x, y, ty));
            if handled {
                return true;
            }
            // Casillas: solo el cuadrado responde, no su texto. Un clic
            // rechazado no tiene efectos, así que se reintenta a lo largo del
            // mismo campo hasta dar con el cuadrado.
            if matches!(ty, 2 | 3 | 9) {
                for dir in [-1.0, 1.0] {
                    for k in 1..=75 {
                        let xx = x + dir * k as f64 * 2.0;
                        if self.field_at(idx, xx, y) != ty {
                            break;
                        }
                        if sys::FORM_OnLButtonDown(self.form, p, mods, xx, y) != 0 {
                            self.press_redirect = Some((xx, y));
                            self.last_focus = Some((idx, xx, y, ty));
                            return true;
                        }
                    }
                }
            }
            false
        }
    }

    /// Punto real al que enviar un clic. PDFium no trata la etiqueta
    /// (caption) como parte del campo: un clic ahí deja el foco a medias y lo
    /// que se escribe después acaba en otro campo. Adobe, en cambio, enfoca el
    /// campo. Si el clic cae fuera de un campo pero justo encima de uno
    /// (etiquetas superiores) o a la derecha de una casilla (texto de la
    /// casilla), se redirige a ese campo.
    fn click_target(&self, idx: usize, px: f64, py: f64) -> (f64, f64) {
        if self.field_at(idx, px, py) >= 0 {
            return (px, py);
        }
        // Justo a la derecha de un campo de texto/fecha: el botón del
        // calendario de los campos de fecha se dibuja en parte fuera de la
        // zona que PDFium detecta como campo. Se pulsa en su borde derecho.
        for k in 1..=12 {
            let x = px - k as f64;
            if x < 0.0 {
                break;
            }
            match self.field_at(idx, x, py) {
                6 | 15 => return (x, py),
                -1 => continue,
                _ => break,
            }
        }
        for k in 1..=18 {
            let y = py + k as f64 * 1.5; // hasta 27 pt hacia abajo
            if self.field_at(idx, px, y) >= 0 {
                return (px, y + 1.0);
            }
        }
        for k in 1..=40 {
            let x = px - k as f64 * 3.0; // hasta 120 pt a la izquierda
            if x < 0.0 {
                break;
            }
            let t = self.field_at(idx, x, py);
            if matches!(t, 2 | 3 | 9) {
                return (x - 1.0, py);
            }
            if t >= 0 {
                break;
            }
        }
        (px, py)
    }

    pub fn left_up(&mut self, idx: usize, px: f64, py: f64, mods: i32) -> bool {
        self.button_held = false;
        if std::mem::take(&mut self.skip_up) {
            return true;
        }
        let p = self.page(idx);
        let (x, y) = self.press_redirect.take().unwrap_or((px, py));
        let r = unsafe { sys::FORM_OnLButtonUp(self.form, p, mods, x, y) != 0 };
        if std::mem::take(&mut self.pressed_button) {
            // El click del botón puede haber quitado filas (removeInstance).
            self.run_command("@relayout");
        }
        r
    }

    /// Evalúa una expresión JavaScript en el formulario XFA y devuelve su
    /// valor como texto (diagnóstico y pruebas). Solo en vista continua.
    pub fn eval_js(&mut self, code: &str) -> Option<String> {
        self.host.state.borrow_mut().eval_result = None;
        self.run_command(&format!("@eval:{code}"));
        self.host.state.borrow_mut().eval_result.take()
    }

    /// Ejecuta el click de un botón por su expresión SOM (diagnóstico).
    pub fn exec_click(&mut self, som: &str) {
        self.run_command(som);
        self.run_command("@relayout");
    }

    /// Ejecuta una orden con el botón invisible de órdenes (ver
    /// xfa::compat::COMMAND_SCRIPT). Sin vista continua no existe y no hace nada.
    fn run_command(&mut self, cmd: &str) {
        if !self.compat.as_ref().is_some_and(|c| c.refresh_button) || self.page_count() == 0 {
            return;
        }
        // Recordar el foco: la orden lo mueve al botón invisible.
        self.host.state.borrow_mut().pending_click = Some(cmd.to_string());
        let (cx, cy) = crate::xfa::compat::COMMAND_POINT;
        let p0 = self.page(0);
        unsafe {
            sys::FORM_ForceToKillFocus(self.form);
            sys::FORM_OnFocus(self.form, p0, 0, cx, cy);
            sys::FORM_ForceToKillFocus(self.form);
            // Lo que haga el script dentro del evento "enter" no se maqueta
            // hasta el siguiente evento de PDFium. Tras el relayout se fuerza
            // ese ciclo con un movimiento de ratón. (Si se maquetara antes del
            // relayout, PDFium conservaría las filas eliminadas.)
            if cmd == "@relayout" {
                // En un punto sin controles (fuera de la página): si el
                // movimiento cae sobre un control, PDFium maqueta antes de
                // aplicar el relayout y conserva las filas eliminadas.
                sys::FORM_OnMouseMove(self.form, p0, 0, -50.0, -50.0);
            }
        }
        self.host.state.borrow_mut().pending_click = None;
    }

    pub fn left_double_click(&mut self, idx: usize, px: f64, py: f64, mods: i32) -> bool {
        let p = self.page(idx);
        let (x, y) = self.click_target(idx, px, py);
        self.press_redirect = if (x, y) != (px, py) { Some((x, y)) } else { None };
        unsafe { sys::FORM_OnLButtonDoubleClick(self.form, p, mods, x, y) != 0 }
    }

    pub fn right_down(&mut self, idx: usize, px: f64, py: f64, mods: i32) -> bool {
        let p = self.page(idx);
        unsafe { sys::FORM_OnRButtonDown(self.form, p, mods, px, py) != 0 }
    }

    pub fn right_up(&mut self, idx: usize, px: f64, py: f64, mods: i32) -> bool {
        let p = self.page(idx);
        unsafe { sys::FORM_OnRButtonUp(self.form, p, mods, px, py) != 0 }
    }

    /// Rueda del ratón sobre un widget (p. ej. lista desplegada). Devuelve
    /// true si PDFium la consumió.
    pub fn wheel(&mut self, idx: usize, px: f64, py: f64, dx: i32, dy: i32, mods: i32) -> bool {
        let p = self.page(idx);
        let pt = sys::FS_POINTF { x: px as f32, y: py as f32 };
        unsafe { sys::FORM_OnMouseWheel(self.form, p, mods, &pt, dx, dy) != 0 }
    }

    /// Tipo de campo bajo el punto (-1 si no hay ninguno).
    pub fn field_at(&self, idx: usize, px: f64, py: f64) -> i32 {
        let p = self.page(idx);
        unsafe { sys::FPDFPage_HasFormFieldAtPoint(self.form, p, px, py) }
    }

    fn set_focus_page(&mut self, idx: usize) {
        self.focus_page = Some(idx);
        self.set_current_page(idx);
    }

    pub fn focus_page(&self) -> Option<usize> {
        self.focus_page
    }

    fn key_page(&self) -> Option<sys::FPDF_PAGE> {
        self.focus_page.map(|i| self.page(i)).filter(|p| !p.is_null())
    }

    pub fn key_down(&mut self, vk: i32, mods: i32) -> bool {
        let Some(p) = self.key_page() else { return false };
        if vk == super::keys::VK_TAB {
            // El foco pasa a un campo que no conocemos.
            self.last_focus = None;
        }
        if matches!(vk, super::keys::VK_TAB | super::keys::VK_RETURN | super::keys::VK_ESCAPE) {
            self.open_list = None;
        }
        unsafe { sys::FORM_OnKeyDown(self.form, p, vk, mods) != 0 }
    }

    pub fn key_up(&mut self, vk: i32, mods: i32) -> bool {
        let Some(p) = self.key_page() else { return false };
        unsafe { sys::FORM_OnKeyUp(self.form, p, vk, mods) != 0 }
    }

    pub fn char_input(&mut self, ch: u32, mods: i32) -> bool {
        let Some(p) = self.key_page() else { return false };
        // Caracteres fuera del BMP: se envían como pares sustitutos UTF-16.
        let mut buf = [0u16; 2];
        let Some(c) = char::from_u32(ch) else { return false };
        let mut handled = false;
        for unit in c.encode_utf16(&mut buf).iter() {
            handled |= unsafe { sys::FORM_OnChar(self.form, p, *unit as c_int, mods) != 0 };
        }
        handled
    }

    pub fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.char_input(c as u32, 0);
        }
    }

    pub fn kill_focus(&mut self) {
        if let Some((li, _)) = self.open_list.take() {
            let lp = self.page(li);
            unsafe {
                sys::FORM_OnKeyDown(self.form, lp, super::keys::VK_ESCAPE, 0);
                sys::FORM_OnKeyUp(self.form, lp, super::keys::VK_ESCAPE, 0);
            }
        }
        if !self.form.is_null() {
            unsafe { sys::FORM_ForceToKillFocus(self.form) };
        }
        self.last_focus = None;
    }

    pub fn focused_text(&self) -> String {
        let Some(p) = self.key_page() else { return String::new() };
        self.read_wide(|buf, len| unsafe { sys::FORM_GetFocusedText(self.form, p, buf, len) })
    }

    pub fn selected_text(&self) -> String {
        let Some(p) = self.key_page() else { return String::new() };
        self.read_wide(|buf, len| unsafe { sys::FORM_GetSelectedText(self.form, p, buf, len) })
    }

    pub fn replace_selection(&mut self, text: &str) {
        let Some(p) = self.key_page() else { return };
        let w = to_wide(text);
        unsafe { sys::FORM_ReplaceSelection(self.form, p, w.as_ptr()) };
    }

    pub fn select_all(&mut self) -> bool {
        let Some(p) = self.key_page() else { return false };
        unsafe { sys::FORM_SelectAllText(self.form, p) != 0 }
    }

    pub fn undo(&mut self) -> bool {
        let Some(p) = self.key_page() else { return false };
        unsafe { sys::FORM_CanUndo(self.form, p) != 0 && sys::FORM_Undo(self.form, p) != 0 }
    }

    pub fn redo(&mut self) -> bool {
        let Some(p) = self.key_page() else { return false };
        unsafe { sys::FORM_CanRedo(self.form, p) != 0 && sys::FORM_Redo(self.form, p) != 0 }
    }

    fn read_wide(&self, f: impl Fn(*mut c_void, c_ulong) -> c_ulong) -> String {
        let need = f(null_mut(), 0) as usize;
        if need <= 2 {
            return String::new();
        }
        let mut buf = vec![0u16; need / 2];
        f(buf.as_mut_ptr() as *mut c_void, need as c_ulong);
        from_wide_buf(&buf)
    }

    // -----------------------------------------------------------------------
    // Estado pendiente, timers y layout
    // -----------------------------------------------------------------------

    /// Vacía y devuelve los rectángulos invalidados (página, rectángulo).
    pub fn take_invalid_rects(&mut self) -> Vec<(usize, [f64; 4])> {
        std::mem::take(&mut self.host.state.borrow_mut().invalid_rects)
    }

    /// Ejecuta el refresco de controles (ver xfa::compat): enfoca el botón
    /// invisible, cuyo evento "enter" recoloca todos los controles, y después
    /// devuelve el foco al campo de texto en el que estaba el usuario.
    /// Devuelve false si el documento no tiene el botón.
    pub fn refresh_widgets(&mut self) -> bool {
        if !self.compat.as_ref().is_some_and(|c| c.refresh_button) || self.page_count() == 0 {
            return false;
        }
        let p = self.page(0);
        let (x, y) = crate::xfa::compat::REFRESH_POINT;
        unsafe {
            sys::FORM_OnFocus(self.form, p, 0, x, y);
        }
        self.text_focus_lost = false;
        if let Some((i, fx, fy, t)) = self.last_focus {
            if matches!(t, 6 | 15) && self.field_at(i, fx, fy) == t {
                let pg = self.page(i);
                unsafe {
                    let a = sys::FORM_OnFocus(self.form, pg, 0, fx, fy);
                    let b = sys::FORM_OnLButtonDown(self.form, pg, 0, fx, fy);
                    let c = sys::FORM_OnLButtonUp(self.form, pg, 0, fx, fy);
                    log::debug!("restaurar foco en ({fx},{fy}): {a} {b} {c}");
                    // Cursor al final del texto, donde el usuario seguía escribiendo.
                    sys::FORM_OnKeyDown(self.form, pg, super::keys::VK_END, 0);
                    sys::FORM_OnKeyUp(self.form, pg, super::keys::VK_END, 0);
                }
                self.focus_page = Some(i);
            } else {
                self.text_focus_lost = matches!(t, 6 | 15);
                self.last_focus = None;
                unsafe { sys::FORM_ForceToKillFocus(self.form) };
            }
        } else {
            unsafe { sys::FORM_ForceToKillFocus(self.form) };
        }
        self.host.state.borrow_mut().invalidate_all = true;
        self.extent_cache.borrow_mut().clear();
        self.known_extent = Some(self.content_height(0));
        true
    }

    /// Detecta si el formulario ha cambiado de estructura (secciones que
    /// aparecen o desaparecen, filas añadidas) y, en ese caso, refresca los
    /// controles. Devuelve true si se refrescó.
    pub fn refresh_if_structure_changed(&mut self) -> bool {
        if !self.compat.as_ref().is_some_and(|c| c.refresh_button) {
            return false;
        }
        // Con una lista desplegable abierta no: el refresco mueve el foco y
        // la cerraría. Se comprobará cuando se cierre.
        // Ni a mitad de un clic: el refresco mueve el foco y PDFium perdería
        // la pulsación (al elegir una opción de una lista, por ejemplo).
        if self.open_list.is_some() || self.button_held {
            return false;
        }
        self.extent_cache.borrow_mut().clear();
        let h = self.content_height(0);
        match self.known_extent {
            None => {
                self.known_extent = Some(h);
                false
            }
            Some(k) if (k - h).abs() > 0.5 => {
                log::debug!("cambio de estructura: alto {k} -> {h}; refrescando controles");
                self.refresh_widgets()
            }
            _ => false,
        }
    }

    /// true si el último refresco dejó al usuario sin campo enfocado.
    pub fn lost_focus_on_refresh(&self) -> bool {
        self.text_focus_lost
    }

    /// Recoge (y vacía) los cambios que PDFium ha notificado.
    pub fn take_updates(&mut self) -> Updates {
        if self.host.state.borrow().layout_changed {
            self.sync_layout();
        }
        {
            let st = self.host.state.borrow();
            if st.layout_changed || st.invalidate_all || !st.invalidated.is_empty() {
                drop(st);
                self.extent_cache.borrow_mut().clear();
            }
        }
        let mut st = self.host.state.borrow_mut();
        let caret = st.caret.and_then(|(p, r)| st.index_of(p).map(|i| (i, r)));
        
        Updates {
            invalidated: std::mem::take(&mut st.invalidated).into_iter().collect(),
            invalidate_all: std::mem::take(&mut st.invalidate_all),
            form_changed: std::mem::take(&mut st.form_changed),
            layout_changed: std::mem::take(&mut st.layout_changed),
            cursor: st.cursor,
            caret,
            text_field_focused: st.text_field_focused,
            goto_page: st.goto_page.take().and_then(|g| usize::try_from(g).ok()),
            print_requested: std::mem::take(&mut st.print_requested),
            submit_requested: st.submit_requested.take(),
            uris: std::mem::take(&mut st.uris),
            open_files: std::mem::take(&mut st.open_files),
        }
    }

    /// Tras un cambio de layout XFA (páginas añadidas o eliminadas) descarta
    /// los handles de páginas que ya no existen.
    fn sync_layout(&mut self) {
        let count = self.page_count();
        let stale: Vec<sys::FPDF_PAGE> = {
            let mut st = self.host.state.borrow_mut();
            if st.pages.len() <= count {
                return;
            }
            let stale = st.pages.split_off(count);
            st.page_sizes.retain(|(p, _, _)| !stale.contains(p));
            stale
        };
        for p in stale.into_iter().filter(|p| !p.is_null()) {
            unsafe {
                sys::FORM_OnBeforeClosePage(p, self.form);
                sys::FPDF_ClosePage(p);
            }
        }
        if let Some(f) = self.focus_page
            && f >= count {
                self.focus_page = None;
            }
    }

    pub fn timer_count(&self) -> usize {
        self.host.state.borrow().timers.len()
    }

    /// Ejecuta los timers vencidos. Devuelve cuánto falta para el siguiente.
    pub fn process_timers(&mut self) -> Option<Duration> {
        let now = Instant::now();
        let due: Vec<(i32, unsafe extern "C" fn(c_int))> = {
            let mut st = self.host.state.borrow_mut();
            let mut due = Vec::new();
            for t in st.timers.iter_mut() {
                if t.next <= now {
                    due.push((t.id, t.cb));
                    t.next = now + t.interval;
                }
            }
            due
        };
        for (id, cb) in due {
            // Puede que un timer anterior haya matado a este.
            let alive = self.host.state.borrow().timers.iter().any(|t| t.id == id);
            if alive {
                unsafe { cb(id) };
            }
        }
        let st = self.host.state.borrow();
        st.timers.iter().map(|t| t.next.saturating_duration_since(Instant::now())).min()
    }

    // -----------------------------------------------------------------------
    // Guardado y exportación
    // -----------------------------------------------------------------------

    /// Serializa el documento (con los datos del formulario) a bytes.
    /// `incremental` conserva intacto el fichero original y añade los cambios
    /// al final, que es lo que hace Adobe Reader con formularios habilitados.
    pub fn save_to_bytes(&mut self, incremental: bool) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        let flags = if incremental { sys::FPDF_INCREMENTAL } else { sys::FPDF_NO_INCREMENTAL };
        let ok = unsafe { sys::FPDF_SaveAsCopy(self.doc, &mut w.base, flags as sys::FPDF_DWORD) };
        if ok == 0 {
            bail!("{}", crate::t!("err.serialize"));
        }
        Ok(w.buf)
    }

    /// Bytes que se escribirían al guardar.
    ///
    /// En XFA dinámico solo se añaden los datos (paquete `datasets`) al
    /// fichero original mediante una actualización incremental: la plantilla
    /// queda intacta aunque PDFium esté usando una copia corregida.
    pub fn bytes_for_save(&mut self) -> Result<Vec<u8>> {
        // Estado del formulario (accesos, secciones visibles, casillas sin
        // datos) para restaurarlo al reabrir. Capturarlo mueve el foco, así
        // que se hace antes de que PDFium serialice los datos.
        self.host.state.borrow_mut().form_state = None;
        self.run_command("@capture");
        let state = self.host.state.borrow_mut().form_state.take();
        let pdfium_bytes = self.save_to_bytes(true)?;
        if self.xfa_loaded && (self.compat.is_some() || self.kind == FormKind::XfaFull) {
            let st = self.host.attachments.borrow();
            let atts = st.dirty.then_some(st.items.as_slice());
            xfa::build_save(&self.base, &pdfium_bytes, atts, state.as_deref())
        } else {
            Ok(pdfium_bytes)
        }
    }

    /// Guarda en `path` de forma atómica (fichero temporal + rename).
    pub fn save(&mut self, path: &Path) -> Result<()> {
        let bytes = self.bytes_for_save()?;
        write_atomic(path, &bytes)?;
        self.base = bytes;
        self.host.attachments.borrow_mut().dirty = false;
        self.path = path.to_path_buf();
        Ok(())
    }

    /// Trozos de página que forman cada hoja del PDF plano:
    /// (página origen, y0, y1, ancho hoja, alto hoja), en puntos.
    fn flat_sheets(&self) -> Vec<(usize, f32, f32, f32, f32)> {
        let mut v = Vec::new();
        for i in 0..self.page_count() {
            let (w, h) = self.page_size(i);
            if !self.is_continuous() {
                v.push((i, 0.0, h, w, h));
                continue;
            }
            // Vista continua: trocear en hojas con proporción A4, cortando
            // por una franja en blanco para no partir campos ni líneas.
            let end = self.content_height(i);
            let sheet_h = w * 297.0 / 210.0;
            let mut y0 = 0.0f32;
            while y0 < end - 1.0 {
                let mut y1 = (y0 + sheet_h).min(end);
                if y1 < end {
                    y1 = self.find_blank_row(i, w, h, y1, sheet_h * 0.3).unwrap_or(y1);
                }
                v.push((i, y0, y1, w, sheet_h));
                y0 = y1;
            }
        }
        v
    }

    /// Busca, subiendo desde `y` hasta `max_up` puntos, una fila completamente
    /// blanca. Devuelve su posición en puntos.
    fn find_blank_row(&self, idx: usize, w: f32, h: f32, y: f32, max_up: f32) -> Option<f32> {
        let (pw, ph) = (w.round() as i32, h.round() as i32);
        let top = (y - max_up).max(0.0) as i32;
        let rh = (y as i32 - top).max(1);
        let px = self.render_region_ex(idx, pw, ph, 0, top, pw, rh, true)?;
        let row = pw as usize * 4;
        // Ignorar márgenes laterales (10 %).
        let (a, b) = (pw as usize / 10, pw as usize * 9 / 10);
        (0..rh as usize).rev().find(|&r| px[r * row + a * 4..r * row + b * 4].chunks(4).all(|c| c[0] > 250 && c[1] > 250 && c[2] > 250)).map(|r| (top + r as i32) as f32)
    }

    /// Crea un PDF "plano" (cada hoja es una imagen) a `dpi`. Sirve para
    /// imprimir o enviar el formulario a quien no tenga un visor XFA. En vista
    /// continua se trocea en hojas A4.
    pub fn export_flat(&mut self, path: &Path, dpi: f32) -> Result<()> {
        let sheets = self.flat_sheets();
        let k = dpi / 72.0;
        unsafe {
            let out = sys::FPDF_CreateNewDocument();
            if out.is_null() {
                bail!("{}", crate::t!("err.create_output"));
            }
            let result = (|| -> Result<()> {
                for (n, &(src, y0, y1, sw, sh)) in sheets.iter().enumerate() {
                    let (pw, ph) = self.page_size(src);
                    let (full_w, full_h) = ((pw * k).round() as i32, (ph * k).round() as i32);
                    let (bw, bh) = ((sw * k).round() as i32, (sh * k).round() as i32);
                    let (ry, rh) = ((y0 * k).round() as i32, ((y1 - y0) * k).round().min(bh as f32) as i32);
                    let rgba = self.render_region_ex(src, full_w, full_h, 0, ry, full_w.min(bw), rh, true).ok_or_else(|| anyhow::anyhow!("{}", crate::t!("err.rasterize", n + 1)))?;
                    let bmp = sys::FPDFBitmap_Create(bw, bh, 0);
                    if bmp.is_null() {
                        bail!("{}", crate::t!("err.rasterize_mem", n + 1));
                    }
                    sys::FPDFBitmap_FillRect(bmp, 0, 0, bw, bh, 0xFFFF_FFFF);
                    let stride = sys::FPDFBitmap_GetStride(bmp) as usize;
                    let dst = sys::FPDFBitmap_GetBuffer(bmp) as *mut u8;
                    let cw = full_w.min(bw) as usize;
                    for r in 0..rh as usize {
                        for c in 0..cw {
                            let s = &rgba[(r * cw + c) * 4..(r * cw + c) * 4 + 4];
                            let d = dst.add(r * stride + c * 4);
                            *d = s[2];
                            *d.add(1) = s[1];
                            *d.add(2) = s[0];
                            *d.add(3) = 255;
                        }
                    }
                    let newp = sys::FPDFPage_New(out, n as c_int, sw as f64, sh as f64);
                    let img = sys::FPDFPageObj_NewImageObj(out);
                    let mut pages = [newp];
                    let ok = sys::FPDFImageObj_SetBitmap(pages.as_mut_ptr(), 1, img, bmp);
                    sys::FPDFBitmap_Destroy(bmp);
                    if ok == 0 {
                        sys::FPDFPageObj_Destroy(img);
                        sys::FPDF_ClosePage(newp);
                        bail!("{}", crate::t!("err.insert_image", n + 1));
                    }
                    sys::FPDFImageObj_SetMatrix(img, sw as f64, 0.0, 0.0, sh as f64, 0.0, 0.0);
                    sys::FPDFPage_InsertObject(newp, img);
                    sys::FPDFPage_GenerateContent(newp);
                    sys::FPDF_ClosePage(newp);
                }
                let mut w = Writer::new();
                if sys::FPDF_SaveAsCopy(out, &mut w.base, sys::FPDF_NO_INCREMENTAL as sys::FPDF_DWORD) == 0 {
                    bail!("{}", crate::t!("err.serialize_flat"));
                }
                write_atomic(path, &w.buf)
            })();
            sys::FPDF_CloseDocument(out);
            result
        }
    }

    // -----------------------------------------------------------------------
    // Búsqueda de texto (PDF normales y XFA estático)
    // -----------------------------------------------------------------------

    /// Ejecuta `f` con la capa de texto de la página `idx` (para seleccionar
    /// y copiar texto). None si la página no tiene capa de texto.
    fn with_text_page<R>(&self, idx: usize, f: impl FnOnce(sys::FPDF_TEXTPAGE) -> R) -> Option<R> {
        let page = self.page(idx);
        if page.is_null() {
            return None;
        }
        unsafe {
            let tp = sys::FPDFText_LoadPage(page);
            if tp.is_null() {
                return None;
            }
            let r = f(tp);
            sys::FPDFText_ClosePage(tp);
            Some(r)
        }
    }

    /// Número de caracteres de la capa de texto de una página.
    pub fn text_char_count(&self, idx: usize) -> i32 {
        self.with_text_page(idx, |tp| unsafe { sys::FPDFText_CountChars(tp) }).unwrap_or(0).max(0)
    }

    /// Índice del carácter en (x, y) (coordenadas de página), con tolerancia
    /// creciente para poder arrastrar por los márgenes.
    pub fn text_char_at(&self, idx: usize, x: f64, y: f64) -> Option<i32> {
        self.with_text_page(idx, |tp| unsafe {
            for tol in [2.0, 8.0, 25.0] {
                let i = sys::FPDFText_GetCharIndexAtPos(tp, x, y, tol, tol);
                if i >= 0 {
                    return Some(i);
                }
            }
            None
        })
        .flatten()
    }

    /// Rectángulos (coordenadas de página: izquierda, arriba, derecha, abajo)
    /// que ocupan los caracteres [start, start + count).
    pub fn text_rects(&self, idx: usize, start: i32, count: i32) -> Vec<[f64; 4]> {
        self.with_text_page(idx, |tp| unsafe {
            let n = sys::FPDFText_CountRects(tp, start, count);
            let mut v = Vec::new();
            for r in 0..n.max(0) {
                let (mut l, mut t, mut rr, mut b) = (0.0, 0.0, 0.0, 0.0);
                if sys::FPDFText_GetRect(tp, r, &mut l, &mut t, &mut rr, &mut b) != 0 {
                    v.push([l, t, rr, b]);
                }
            }
            v
        })
        .unwrap_or_default()
    }

    /// Palabra que contiene el carácter `ch` de una página: (inicio, número
    /// de caracteres). Un espacio o un signo de puntuación es su propia
    /// "palabra".
    pub fn text_word(&self, idx: usize, ch: i32) -> (i32, i32) {
        self.with_text_page(idx, |tp| unsafe {
            let n = sys::FPDFText_CountChars(tp);
            if ch < 0 || ch >= n {
                return (ch.max(0), 0);
            }
            let is_word = |i: i32| char::from_u32(sys::FPDFText_GetUnicode(tp, i)).is_some_and(|c| c.is_alphanumeric() || c == '_');
            if !is_word(ch) {
                return (ch, 1);
            }
            let mut start = ch;
            while start > 0 && is_word(start - 1) {
                start -= 1;
            }
            let mut end = ch + 1;
            while end < n && is_word(end) {
                end += 1;
            }
            (start, end - start)
        })
        .unwrap_or((ch.max(0), 0))
    }

    /// Texto de los caracteres [start, start + count) de una página.
    pub fn text_range(&self, idx: usize, start: i32, count: i32) -> String {
        if count <= 0 {
            return String::new();
        }
        self.with_text_page(idx, |tp| unsafe {
            let mut buf = vec![0u16; count as usize + 1];
            let n = sys::FPDFText_GetText(tp, start, count, buf.as_mut_ptr());
            let len = (n.max(1) - 1) as usize;
            String::from_utf16_lossy(&buf[..len.min(buf.len())])
        })
        .unwrap_or_default()
        .replace('\r', "")
    }

    /// true si el usuario está editando un campo (texto con cursor, o un
    /// desplegable con el foco o abierto): las flechas son para el campo.
    pub fn editing_field(&self) -> bool {
        self.host.state.borrow().text_field_focused || self.open_list.is_some() || matches!(self.last_focus, Some((_, _, _, 4 | 10)))
    }

    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        let mut hits = Vec::new();
        if query.trim().is_empty() {
            return hits;
        }
        let wq = to_wide(query);
        for i in 0..self.page_count() {
            let page = self.page(i);
            if page.is_null() {
                continue;
            }
            unsafe {
                let tp = sys::FPDFText_LoadPage(page);
                if tp.is_null() {
                    continue;
                }
                let sh = sys::FPDFText_FindStart(tp, wq.as_ptr(), 0, 0);
                if !sh.is_null() {
                    while sys::FPDFText_FindNext(sh) != 0 {
                        let start = sys::FPDFText_GetSchResultIndex(sh);
                        let cnt = sys::FPDFText_GetSchCount(sh);
                        let nr = sys::FPDFText_CountRects(tp, start, cnt);
                        let mut rects = Vec::new();
                        for r in 0..nr {
                            let (mut l, mut t, mut rr, mut b) = (0.0, 0.0, 0.0, 0.0);
                            if sys::FPDFText_GetRect(tp, r, &mut l, &mut t, &mut rr, &mut b) != 0 {
                                rects.push([l, t, rr, b]);
                            }
                        }
                        if !rects.is_empty() {
                            hits.push(SearchHit { page: i, rects });
                        }
                    }
                    sys::FPDFText_FindClose(sh);
                }
                sys::FPDFText_ClosePage(tp);
            }
        }
        hits
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        unsafe {
            let pages: Vec<sys::FPDF_PAGE> = std::mem::take(&mut self.host.state.borrow_mut().pages);
            for p in pages.into_iter().filter(|p| !p.is_null()) {
                if !self.form.is_null() {
                    sys::FORM_DoPageAAction(p, self.form, sys::FPDFPAGE_AACTION_CLOSE as c_int);
                    sys::FORM_OnBeforeClosePage(p, self.form);
                }
                sys::FPDF_ClosePage(p);
            }
            if !self.form.is_null() {
                sys::FORM_DoDocumentAAction(self.form, sys::FPDFDOC_AACTION_WC as c_int);
                sys::FPDFDOC_ExitFormFillEnvironment(self.form);
            }
            sys::FPDF_CloseDocument(self.doc);
        }
    }
}

#[repr(C)]
struct Writer {
    base: sys::FPDF_FILEWRITE,
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Box<Writer> {
        Box::new(Writer { base: sys::FPDF_FILEWRITE { version: 1, WriteBlock: Some(write_block) }, buf: Vec::new() })
    }
}

unsafe extern "C" fn write_block(p: *mut sys::FPDF_FILEWRITE, data: *const c_void, size: c_ulong) -> c_int {
    unsafe {
        let w = &mut *(p as *mut Writer);
        if size > 0 {
            w.buf.extend_from_slice(std::slice::from_raw_parts(data as *const u8, size as usize));
        }
        1
    }
}

/// Trozo renderizado de una página: primera fila (en puntos) y píxeles RGBA.
struct Strip {
    top: i32,
    width: i32,
    px: Vec<u8>,
}

/// Rectángulo (en coordenadas de página) donde difieren dos franjas.
/// Rectángulo (en puntos de página) que engloba los píxeles distintos entre
/// dos franjas, considerando solo las filas con y >= `min_y`.
fn diff_bbox(a: &Strip, b: &Strip, min_y: f64) -> Option<[f64; 4]> {
    if a.top != b.top || a.width != b.width || a.px.len() != b.px.len() {
        return None;
    }
    let w = a.width as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, (pa, pb)) in a.px.chunks(4).zip(b.px.chunks(4)).enumerate() {
        if pa != pb {
            let (x, y) = (i % w, i / w);
            if ((a.top as usize + y) as f64) < min_y {
                continue;
            }
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (x0 != usize::MAX).then(|| [x0 as f64, (a.top as usize + y0) as f64, (x1 + 1) as f64, (a.top as usize + y1 + 1) as f64])
}

/// Escribe en un temporal del mismo directorio y lo renombra encima del destino.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "salida.pdf".into());
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp).with_context(|| crate::t!("err.create", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| crate::t!("err.write", path.display()))?;
    Ok(())
}
