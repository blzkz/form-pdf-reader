//! UniFFI bindings of the Form PDF Reader core, for the Android app.
//!
//! The Kotlin side gets a `PdfDocument` object and implements `FormHandler`
//! for the form's dialogs. Bindings are generated with uniffi-bindgen
//! (see `scripts/android/build-rust.sh`).
//!
//! **Threads:** PDFium and its V8 engine are tied to the thread that
//! initialises them. The app must call `init_pdfium()` and every
//! `PdfDocument` method from one and the same thread (the Android app uses a
//! single-thread executor). The `Mutex` only serialises calls; it does not
//! make PDFium usable from several threads.

use form_pdf_reader::pdfium::{self, AlertAnswer as CoreAnswer, AlertButtons as CoreButtons, Document, FormKind, OpenOptions, keys};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

uniffi::setup_scaffolding!();

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Errors returned to Kotlin as exceptions.
#[derive(Debug, uniffi::Error)]
#[uniffi(flat_error)]
pub enum PdfError {
    Open(String),
    Save(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::Open(m) | PdfError::Save(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for PdfError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DocKind {
    Pdf,
    AcroForm,
    XfaStatic,
    XfaDynamic,
}

/// Page size in PDF points.
#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Key {
    Backspace,
    Delete,
    Enter,
    Tab,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AlertButtons {
    Ok,
    OkCancel,
    YesNo,
    YesNoCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AlertAnswer {
    Ok,
    Cancel,
    Yes,
    No,
}

/// What the app has to do after an input event.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct InputResult {
    /// The page has to be drawn again.
    pub redraw: bool,
    /// The form data changed (unsaved changes).
    pub modified: bool,
    /// The form changed its structure (sections, rows): page heights may
    /// have changed.
    pub restructured: bool,
    /// A text field has the cursor: show the keyboard.
    pub text_input: bool,
    /// Attachments the form asked to view (temporary files).
    pub open_files: Vec<String>,
    /// Links the form asked to open.
    pub uris: Vec<String>,
    /// URL the form tried to submit to (not supported).
    pub submit: Option<String>,
}

/// A rectangle in pixels on a page drawn at a given size.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct PageRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// A search result: where it is on its page, in pixels of the page drawn at
/// its size in points (multiply by the zoom).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SearchHit {
    pub page: u32,
    pub rects: Vec<PageRect>,
}

/// Characters [start, start + count) of a page's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct TextRange {
    pub start: i32,
    pub count: i32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AttachmentInfo {
    pub name: String,
    pub file_name: String,
    pub size: u64,
}

/// Dialogs requested by the form. Implemented by the app. The calls block
/// the PDFium thread until the user answers, so the app must show them on
/// the UI thread and wait.
#[uniffi::export(callback_interface)]
pub trait FormHandler: Send + Sync {
    /// `app.alert()` / `xfa.host.messageBox()`.
    fn alert(&self, title: String, message: String, buttons: AlertButtons) -> AlertAnswer;
    /// File picker for form attachments: path of a local copy, or None.
    fn pick_file(&self) -> Option<String>;
    /// `app.response()`: text question; None if cancelled.
    fn ask(&self, question: String, title: String, default_value: String) -> Option<String>;
}

// ---------------------------------------------------------------------------
// Global functions
// ---------------------------------------------------------------------------

/// Initialises PDFium. Call once, on the thread that will use the documents.
#[uniffi::export]
pub fn init_pdfium() {
    pdfium::init();
}

/// Language of the texts produced by the core (error messages): "en", "es"…
/// Call before anything else.
#[uniffi::export]
pub fn set_language(code: String) {
    form_pdf_reader::i18n::set_language(&code);
}

/// Directory for temporary files (attachments extracted to be viewed).
/// Android has no /tmp: pass the app's cache directory. Call at start-up,
/// before opening documents.
#[uniffi::export]
pub fn set_temp_dir(path: String) {
    let _ = std::fs::create_dir_all(&path);
    // SAFETY: called once at start-up from the PDFium thread, before any
    // other code of this library reads the environment.
    unsafe { std::env::set_var("TMPDIR", path) };
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

#[derive(uniffi::Object)]
pub struct PdfDocument {
    doc: Mutex<Document>,
}

// SAFETY: see the module documentation. The app only uses the document from
// the thread that initialised PDFium; uniffi needs Send + Sync to hand the
// object to Kotlin.
unsafe impl Send for PdfDocument {}
unsafe impl Sync for PdfDocument {}

fn usize_page(p: u32) -> usize {
    p as usize
}

impl PdfDocument {
    fn lock(&self) -> MutexGuard<'_, Document> {
        self.doc.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Page coordinates from a position on the page drawn at
    /// `page_w` × `page_h` pixels.
    fn to_page(doc: &Document, page: usize, page_w: f64, page_h: f64, x: f64, y: f64) -> (f64, f64) {
        doc.device_to_page(page, page_w, page_h, x, y)
    }

    fn finish(doc: &mut Document) -> InputResult {
        let mut u = doc.take_updates();
        let restructured = doc.refresh_if_structure_changed();
        if restructured {
            // The refresh may produce more updates.
            let u2 = doc.take_updates();
            u.form_changed |= u2.form_changed;
            u.open_files.extend(u2.open_files);
            u.uris.extend(u2.uris);
        }
        InputResult {
            redraw: true,
            modified: u.form_changed,
            restructured,
            text_input: doc.editing_field() && u.caret.is_some() || u.text_field_focused,
            open_files: u.open_files.into_iter().map(|p| p.display().to_string()).collect(),
            uris: u.uris,
            submit: u.submit_requested,
        }
    }
}

#[uniffi::export]
impl PdfDocument {
    /// Opens a PDF. `continuous_xfa`: show dynamic XFA forms as one long
    /// page (recommended; PDFium's XFA pagination loses sections).
    #[uniffi::constructor]
    pub fn open(path: String, continuous_xfa: bool) -> Result<Arc<Self>, PdfError> {
        pdfium::init();
        let doc = Document::open_with(Path::new(&path), OpenOptions { continuous_xfa }).map_err(|e| PdfError::Open(format!("{e:#}")))?;
        Ok(Arc::new(PdfDocument { doc: Mutex::new(doc) }))
    }

    /// Connects the form's dialogs to the app.
    pub fn set_handler(&self, handler: Box<dyn FormHandler>) {
        let h: Arc<dyn FormHandler> = Arc::from(handler);
        let doc = self.lock();
        let a = h.clone();
        doc.set_alert_handler(Box::new(move |req| {
            let buttons = match req.buttons {
                CoreButtons::Ok => AlertButtons::Ok,
                CoreButtons::OkCancel => AlertButtons::OkCancel,
                CoreButtons::YesNo => AlertButtons::YesNo,
                CoreButtons::YesNoCancel => AlertButtons::YesNoCancel,
            };
            match a.alert(req.title.clone(), req.message.clone(), buttons) {
                AlertAnswer::Ok => CoreAnswer::Ok,
                AlertAnswer::Cancel => CoreAnswer::Cancel,
                AlertAnswer::Yes => CoreAnswer::Yes,
                AlertAnswer::No => CoreAnswer::No,
            }
        }));
        let b = h.clone();
        doc.set_browse_handler(Box::new(move || b.pick_file()));
        let c = h;
        doc.set_response_handler(Box::new(move |q, t, d| c.ask(q.to_string(), t.to_string(), d.to_string())));
    }

    pub fn kind(&self) -> DocKind {
        let doc = self.lock();
        match doc.kind() {
            FormKind::XfaFull if doc.is_xfa() => DocKind::XfaDynamic,
            FormKind::XfaForeground if doc.is_xfa() => DocKind::XfaStatic,
            FormKind::None => DocKind::Pdf,
            _ => DocKind::AcroForm,
        }
    }

    /// true for a dynamic XFA form shown as one long page.
    pub fn is_continuous(&self) -> bool {
        self.lock().is_continuous()
    }

    pub fn page_count(&self) -> u32 {
        self.lock().page_count() as u32
    }

    /// Page size in points.
    pub fn page_size(&self, page: u32) -> PageSize {
        let (width, height) = self.lock().page_size(usize_page(page));
        PageSize { width, height }
    }

    /// Height to show, in points: in continuous view, up to the end of the
    /// content (the page itself is much taller).
    pub fn content_height(&self, page: u32) -> f32 {
        self.lock().content_height(usize_page(page))
    }

    /// Renders part of a page drawn at `page_w` × `page_h` pixels: the
    /// `w` × `h` rectangle starting at (`x`, `y`). Returns RGBA pixels
    /// (the byte order of an Android ARGB_8888 bitmap), or an empty list.
    #[allow(clippy::too_many_arguments)]
    pub fn render(&self, page: u32, page_w: i32, page_h: i32, x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
        self.lock().render_region(usize_page(page), page_w, page_h, x, y, w, h).unwrap_or_default()
    }

    /// Type of form field at a position (see `tap`), or -1.
    pub fn field_at(&self, page: u32, page_w: f64, page_h: f64, x: f64, y: f64) -> i32 {
        let doc = self.lock();
        let (px, py) = Self::to_page(&doc, usize_page(page), page_w, page_h, x, y);
        doc.field_at(usize_page(page), px, py)
    }

    /// A tap at (`x`, `y`) pixels on the page drawn at `page_w` × `page_h`.
    pub fn tap(&self, page: u32, page_w: f64, page_h: f64, x: f64, y: f64) -> InputResult {
        let mut doc = self.lock();
        let p = usize_page(page);
        let (px, py) = Self::to_page(&doc, p, page_w, page_h, x, y);
        doc.set_current_page(p);
        doc.mouse_move(p, px, py, 0);
        doc.left_down(p, px, py, 0);
        doc.left_up(p, px, py, 0);
        Self::finish(&mut doc)
    }

    /// Text typed on the keyboard.
    pub fn type_text(&self, text: String) -> InputResult {
        let mut doc = self.lock();
        doc.type_text(&text);
        // PDFium only reports the change when the field is committed.
        let mut r = Self::finish(&mut doc);
        r.modified |= !text.is_empty();
        r
    }

    /// A special key.
    pub fn key(&self, key: Key) -> InputResult {
        let mut doc = self.lock();
        let vk = match key {
            Key::Backspace => keys::VK_BACK,
            Key::Delete => keys::VK_DELETE,
            Key::Enter => keys::VK_RETURN,
            Key::Tab => keys::VK_TAB,
            Key::Escape => keys::VK_ESCAPE,
            Key::Left => keys::VK_LEFT,
            Key::Right => keys::VK_RIGHT,
            Key::Up => keys::VK_UP,
            Key::Down => keys::VK_DOWN,
            Key::Home => keys::VK_HOME,
            Key::End => keys::VK_END,
        };
        doc.key_down(vk, 0);
        if let Some(c) = keys::char_for_vk(vk) {
            doc.char_input(c as u32, 0);
        }
        doc.key_up(vk, 0);
        let mut r = Self::finish(&mut doc);
        r.modified |= matches!(key, Key::Backspace | Key::Delete | Key::Enter);
        r
    }

    /// Commits the field being edited and removes the focus.
    pub fn kill_focus(&self) -> InputResult {
        let mut doc = self.lock();
        doc.kill_focus();
        Self::finish(&mut doc)
    }

    /// Checks again whether the form changed its structure. PDFium sometimes
    /// lays out one event later: call it a few times after each input.
    pub fn check_structure(&self) -> InputResult {
        let mut doc = self.lock();
        Self::finish(&mut doc)
    }

    /// Runs PDFium's pending timers. Returns the milliseconds until the next
    /// call, or None if there are no timers.
    pub fn process_timers(&self) -> Option<u64> {
        self.lock().process_timers().map(|d| d.as_millis() as u64)
    }

    /// A dropdown list (or the date picker) is open.
    pub fn list_open(&self) -> bool {
        self.lock().list_open()
    }

    /// Text of the field being edited.
    pub fn focused_text(&self) -> String {
        self.lock().focused_text()
    }

    /// The saved PDF: the original file plus an incremental update with the
    /// form data, its attachments and state.
    pub fn save_bytes(&self) -> Result<Vec<u8>, PdfError> {
        self.lock().bytes_for_save().map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    /// Saves to a file.
    pub fn save(&self, path: String) -> Result<(), PdfError> {
        self.lock().save(Path::new(&path)).map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    pub fn attachments(&self) -> Vec<AttachmentInfo> {
        self.lock().attachments().into_iter().map(|a| AttachmentInfo { size: a.data.len() as u64, name: a.name, file_name: a.file_name }).collect()
    }

    pub fn attachment_data(&self, name: String) -> Option<Vec<u8>> {
        self.lock().attachments().into_iter().find(|a| a.name == name).map(|a| a.data)
    }

    // Text selection. Character indices refer to the page's text layer;
    // positions and rectangles are pixels on the page drawn at
    // `page_w` × `page_h`, as in `tap`.

    /// Number of characters in the text layer of a page.
    pub fn text_char_count(&self, page: u32) -> i32 {
        self.lock().text_char_count(usize_page(page))
    }

    /// Character at (`x`, `y`) (or near it), or -1.
    pub fn text_char_at(&self, page: u32, page_w: f64, page_h: f64, x: f64, y: f64) -> i32 {
        let doc = self.lock();
        let p = usize_page(page);
        let (px, py) = Self::to_page(&doc, p, page_w, page_h, x, y);
        doc.text_char_at(p, px, py).unwrap_or(-1)
    }

    /// The word containing character `index`.
    pub fn text_word_at(&self, page: u32, index: i32) -> TextRange {
        let (start, count) = self.lock().text_word(usize_page(page), index);
        TextRange { start, count }
    }

    /// Rectangles covering characters [start, start + count), one per line.
    #[allow(clippy::too_many_arguments)]
    pub fn text_rects(&self, page: u32, page_w: f64, page_h: f64, start: i32, count: i32) -> Vec<PageRect> {
        let doc = self.lock();
        let p = usize_page(page);
        doc.text_rects(p, start, count)
            .into_iter()
            .map(|[l, t, r, b]| {
                let (x1, y1) = doc.page_to_device(p, page_w, page_h, l, t);
                let (x2, y2) = doc.page_to_device(p, page_w, page_h, r, b);
                PageRect { left: x1.min(x2) as f32, top: y1.min(y2) as f32, right: x1.max(x2) as f32, bottom: y1.max(y2) as f32 }
            })
            .collect()
    }

    /// Text of characters [start, start + count).
    pub fn text_range(&self, page: u32, start: i32, count: i32) -> String {
        self.lock().text_range(usize_page(page), start, count)
    }

    /// Every occurrence of `query` (ignoring case), in page order. The text
    /// of dynamic XFA forms is not in the text layer and is not searched.
    pub fn search(&self, query: String) -> Vec<SearchHit> {
        let doc = self.lock();
        doc.search(&query)
            .into_iter()
            .map(|h| {
                let (w, h_pt) = doc.page_size(h.page);
                let rects = h
                    .rects
                    .into_iter()
                    .map(|[l, t, r, b]| {
                        let (x1, y1) = doc.page_to_device(h.page, w as f64, h_pt as f64, l, t);
                        let (x2, y2) = doc.page_to_device(h.page, w as f64, h_pt as f64, r, b);
                        PageRect { left: x1.min(x2) as f32, top: y1.min(y2) as f32, right: x1.max(x2) as f32, bottom: y1.max(y2) as f32 }
                    })
                    .collect();
                SearchHit { page: h.page as u32, rects }
            })
            .collect()
    }

    /// Plain text of a page (for copying).
    pub fn page_text(&self, page: u32) -> String {
        let doc = self.lock();
        let p = usize_page(page);
        let n = doc.text_char_count(p);
        doc.text_range(p, 0, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures").join(name).display().to_string()
    }

    struct Auto;
    impl FormHandler for Auto {
        fn alert(&self, _t: String, _m: String, _b: AlertButtons) -> AlertAnswer {
            AlertAnswer::Ok
        }
        fn pick_file(&self) -> Option<String> {
            None
        }
        fn ask(&self, _q: String, _t: String, d: String) -> Option<String> {
            Some(d)
        }
    }

    /// The API as the Android app uses it, on one thread (PDFium requires it).
    #[test]
    fn uso_desde_la_app() {
        init_pdfium();
        assert!(PdfDocument::open("/no/existe.pdf".into(), true).is_err());

        // Normal PDF: pages, render, text.
        let d = PdfDocument::open(fixture("normal.pdf"), true).unwrap();
        d.set_handler(Box::new(Auto));
        assert_eq!(d.kind(), DocKind::Pdf);
        assert_eq!(d.page_count(), 3);
        let s = d.page_size(0);
        assert!(s.width > 500.0 && s.height > 700.0);
        let px = d.render(0, 600, 800, 0, 0, 600, 100);
        assert_eq!(px.len(), 600 * 100 * 4);
        assert!(px.chunks(4).any(|p| p[0] < 128), "debería haber texto negro");
        assert!(d.page_text(0).contains("ZANAHORIA"));

        // Text selection: the word under a position, its rectangles.
        let n = d.text_char_count(0);
        let i = (0..n).find(|&i| d.text_range(0, i, 9) == "ZANAHORIA").expect("ZANAHORIA en la capa de texto");
        assert_eq!(d.text_word_at(0, i + 3), TextRange { start: i, count: 9 });
        let rects = d.text_rects(0, 600.0, 800.0, i, 9);
        assert!(!rects.is_empty());
        let r = rects[0];
        assert!(r.left < r.right && r.top < r.bottom && r.right <= 600.0 && r.bottom <= 800.0, "{r:?}");
        let c = d.text_char_at(0, 600.0, 800.0, ((r.left + r.right) / 2.0) as f64, ((r.top + r.bottom) / 2.0) as f64);
        assert!((i..i + 9).contains(&c), "carácter {c} fuera de la palabra");
        assert_eq!(d.text_char_at(0, 600.0, 800.0, 599.0, 799.0), -1);

        // Search: one hit per page, inside the page.
        let hits = d.search("zanahoria".into());
        assert_eq!(hits.iter().map(|h| h.page).collect::<Vec<_>>(), vec![0, 1, 2]);
        let r = hits[0].rects[0];
        assert!(r.left < r.right && r.top < r.bottom && r.right <= s.width && r.bottom <= s.height, "{r:?}");
        assert!(d.search("no aparece".into()).is_empty());

        // AcroForm: tap a text field, type, save, reopen.
        let f = PdfDocument::open(fixture("acroform.pdf"), true).unwrap();
        f.set_handler(Box::new(Auto));
        assert_eq!(f.kind(), DocKind::AcroForm);
        let ps = f.page_size(0);
        let (pw, ph) = (ps.width as f64, ps.height as f64);
        // Find a text field by scanning the page at 1 px/pt.
        let mut hit = None;
        'scan: for y in (0..ph as i32).step_by(4) {
            for x in (0..pw as i32).step_by(4) {
                if f.field_at(0, pw, ph, x as f64, y as f64) == 6 {
                    hit = Some((x as f64 + 2.0, y as f64 + 2.0));
                    break 'scan;
                }
            }
        }
        let (x, y) = hit.expect("el AcroForm de prueba tiene un campo de texto");
        let r = f.tap(0, pw, ph, x, y);
        assert!(r.redraw);
        let r = f.type_text("Hola Android".into());
        assert!(r.modified, "escribir debería marcar cambios");
        f.key(Key::Backspace);
        assert_eq!(f.focused_text(), "Hola Androi");
        f.kill_focus();
        let out = std::env::temp_dir().join(format!("ffi-{}.pdf", std::process::id()));
        f.save(out.display().to_string()).unwrap();
        assert!(!f.save_bytes().unwrap().is_empty());
        drop(f);
        let g = PdfDocument::open(out.display().to_string(), true).unwrap();
        g.tap(0, pw, ph, x, y);
        assert_eq!(g.focused_text(), "Hola Androi");
        let _ = std::fs::remove_file(out);
    }
}
