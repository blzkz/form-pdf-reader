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

use form_pdf_reader::sign::{self as core_sign, SavedSignature, SignatureKind, TypeStyle};
use form_pdf_reader::pdfium::pages::{self as core_pages, PageEdit};
use form_pdf_reader::pdfium::{self, AlertAnswer as CoreAnswer, AlertButtons as CoreButtons, AnnotKind, Document, FormKind, OpenOptions, PasswordRequired, keys};
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
    /// The PDF needs a password to open (open it again with one).
    Password(String),
    /// The password given is not correct.
    WrongPassword(String),
    /// A handwritten signature could not be saved or placed.
    Sign(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::Open(m) | PdfError::Save(m) | PdfError::Password(m) | PdfError::WrongPassword(m) | PdfError::Sign(m) => f.write_str(m),
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
    /// The text field that got the cursor with this tap or press, in pixels
    /// of the page drawn at the size given to it (only from `tap` and
    /// `press`).
    pub field: Option<PageRect>,
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

/// Language of the texts produced by the core (error messages): "en", "es",
/// "fr", "it", "pt" or "de"; any other language uses English.
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
// Digital signatures with a certificate (PAdES)
// ---------------------------------------------------------------------------

/// A certificate, summarised to choose one.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CertificateInfo {
    pub subject: String,
    pub issuer: String,
    /// Holder's serial number (the ID number in Spanish certificates).
    pub holder_id: Option<String>,
    pub valid_from: String,
    pub valid_to: String,
    pub expired: bool,
    pub for_signing: bool,
}

/// Summary of the certificate `der` (None if it cannot be read).
#[uniffi::export]
pub fn certificate_info(der: Vec<u8>) -> Option<CertificateInfo> {
    let i = form_pdf_reader::pades::cert_info(&der)?;
    Some(CertificateInfo { subject: i.subject, issuer: i.issuer, holder_id: i.holder_id, valid_from: i.valid_from, valid_to: i.valid_to, expired: i.expired, for_signing: i.for_signing })
}

/// The key of the certificate `der` is RSA (otherwise, elliptic curve).
#[uniffi::export]
pub fn certificate_is_rsa(der: Vec<u8>) -> bool {
    matches!(form_pdf_reader::pades::key_kind(&der), Ok(form_pdf_reader::pades::KeyKind::Rsa))
}

/// A certificate file (.p12, .pfx) opened with its password. The private
/// key is wiped from memory when the object is released.
#[derive(uniffi::Object)]
pub struct P12Certificate {
    key: Mutex<form_pdf_reader::pades::P12Key>,
}

#[uniffi::export]
impl P12Certificate {
    #[uniffi::constructor]
    pub fn open(data: Vec<u8>, password: String) -> Result<Arc<Self>, PdfError> {
        let key = form_pdf_reader::pades::P12Key::open(&data, &password).map_err(|e| {
            if e.downcast_ref::<form_pdf_reader::pades::p12::WrongPassword>().is_some() {
                PdfError::WrongPassword(format!("{e}"))
            } else {
                PdfError::Sign(format!("{e:#}"))
            }
        })?;
        Ok(Arc::new(P12Certificate { key: Mutex::new(key) }))
    }

    /// The certificate and its intermediates (DER), signer first.
    pub fn chain(&self) -> Vec<Vec<u8>> {
        use form_pdf_reader::pades::Signer;
        self.key.lock().unwrap_or_else(|e| e.into_inner()).chain()
    }

    /// Signs `data` (SHA-256 with RSA or ECDSA).
    pub fn sign(&self, data: Vec<u8>) -> Result<Vec<u8>, PdfError> {
        use form_pdf_reader::pades::Signer;
        self.key.lock().unwrap_or_else(|e| e.into_inner()).sign(&data).map_err(|e| PdfError::Sign(format!("{e:#}")))
    }
}

/// Where a digital signature goes.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum DigitalPlacement {
    /// Nothing on the page (it shows in the signatures panel of readers).
    Invisible,
    /// An empty signature field of the document.
    Field { name: String },
    /// A new box centred at (x, y), in points of the page from its top left
    /// corner.
    New { page: u32, x: f32, y: f32, width: f32, height: f32 },
}

/// A signature field of the document.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SignatureFieldInfo {
    pub name: String,
    pub page: Option<u32>,
    pub signed: bool,
}

/// A digital signature waiting for the key to sign `data_to_sign`.
#[derive(uniffi::Object)]
pub struct PendingSignature {
    prepared: Mutex<Option<form_pdf_reader::pades::Prepared>>,
    to_sign: Vec<u8>,
}

#[uniffi::export]
impl PendingSignature {
    /// What the private key has to sign (SHA-256 with RSA or ECDSA).
    pub fn data_to_sign(&self) -> Vec<u8> {
        self.to_sign.clone()
    }

    /// Puts the signature in and returns the signed PDF.
    pub fn finish(&self, signature: Vec<u8>) -> Result<Vec<u8>, PdfError> {
        let p = self.prepared.lock().unwrap_or_else(|e| e.into_inner()).take().ok_or_else(|| PdfError::Sign("already finished".into()))?;
        p.finish(&signature).map_err(|e| PdfError::Sign(format!("{e:#}")))
    }
}

// ---------------------------------------------------------------------------
// Handwritten signatures
// ---------------------------------------------------------------------------

/// An image of a signature: RGBA, transparent background.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SignatureImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A signature saved on the device.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SignatureInfo {
    pub id: String,
    pub initials: bool,
}

/// A drawn stroke: its width and points x0, y0, x1, y1… in the units of
/// the pad it was drawn on (y downwards).
#[derive(Debug, Clone, uniffi::Record)]
pub struct InkStroke {
    pub width: f32,
    pub points: Vec<f32>,
}

/// Kind of annotation the app can add, edit or remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AnnotationKind {
    Highlight,
    Underline,
    StrikeOut,
    Note,
    FreeText,
    Ink,
}

impl From<AnnotKind> for AnnotationKind {
    fn from(k: AnnotKind) -> Self {
        match k {
            AnnotKind::Highlight => AnnotationKind::Highlight,
            AnnotKind::Underline => AnnotationKind::Underline,
            AnnotKind::StrikeOut => AnnotationKind::StrikeOut,
            AnnotKind::Note => AnnotationKind::Note,
            AnnotKind::FreeText => AnnotationKind::FreeText,
            AnnotKind::Ink => AnnotationKind::Ink,
        }
    }
}

impl From<AnnotationKind> for AnnotKind {
    fn from(k: AnnotationKind) -> Self {
        match k {
            AnnotationKind::Highlight => AnnotKind::Highlight,
            AnnotationKind::Underline => AnnotKind::Underline,
            AnnotationKind::StrikeOut => AnnotKind::StrikeOut,
            AnnotationKind::Note => AnnotKind::Note,
            AnnotationKind::FreeText => AnnotKind::FreeText,
            AnnotationKind::Ink => AnnotKind::Ink,
        }
    }
}

/// An annotation on a page.
#[derive(Debug, Clone, uniffi::Record)]
pub struct AnnotationInfo {
    pub page: u32,
    /// Position in the page's list (it changes when an earlier one is removed).
    pub index: u32,
    pub kind: AnnotationKind,
    /// In points of the page, from its top left corner.
    pub rect: PageRect,
    /// The comment (note), the text (free text) or the marked text.
    pub contents: String,
    /// 0xRRGGBB.
    pub color: u32,
    pub font_size: f32,
}

/// A change to the pages (numbers from 0, as the document is after the
/// previous changes).
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PageEditOp {
    /// 90° per quarter turn (positive: clockwise).
    Rotate { pages: Vec<u32>, quarter_turns: i32 },
    Delete { pages: Vec<u32> },
    /// Move the pages so the first one ends at `to` (counted without them).
    Move { pages: Vec<u32>, to: u32 },
    /// Insert all the pages of another PDF before page `at`.
    Insert { pdf: Vec<u8>, password: Option<String>, at: u32 },
}

impl From<PageEditOp> for PageEdit {
    fn from(e: PageEditOp) -> Self {
        let v = |p: Vec<u32>| p.into_iter().map(|p| p as usize).collect();
        match e {
            PageEditOp::Rotate { pages, quarter_turns } => PageEdit::Rotate { pages: v(pages), quarter_turns },
            PageEditOp::Delete { pages } => PageEdit::Delete { pages: v(pages) },
            PageEditOp::Move { pages, to } => PageEdit::Move { pages: v(pages), to: to as usize },
            PageEditOp::Insert { pdf, password, at } => PageEdit::Insert { pdf, password, at: at as usize },
        }
    }
}

/// Number of pages of a PDF (to check one before inserting it:
/// `PdfError::Password` if it needs a password).
#[uniffi::export]
pub fn pdf_page_count(pdf: Vec<u8>, password: Option<String>) -> Result<u32, PdfError> {
    core_pages::page_count(&pdf, password.as_deref()).map(|n| n as u32).map_err(|e| match e.downcast_ref::<PasswordRequired>() {
        Some(p) if p.wrong => PdfError::WrongPassword(e.to_string()),
        Some(_) => PdfError::Password(e.to_string()),
        None => PdfError::Open(format!("{e:#}")),
    })
}

/// A signature placed on the document and not yet written to the PDF
/// (that happens when saving): it can still be moved or removed.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PlacedSignatureInfo {
    pub index: u32,
    pub page: u32,
    /// In points of the page, from its top left corner.
    pub rect: PageRect,
    pub id: String,
}

fn image(sig: &SavedSignature, height: u32) -> Option<SignatureImage> {
    let (w, h, rgba) = pdfium::render_signature(sig, height as i32)?;
    Some(SignatureImage { width: w as u32, height: h as u32, rgba })
}

fn style_of(key: &str) -> TypeStyle {
    TypeStyle::from_key(key).unwrap_or(TypeStyle::GreatVibes)
}

fn rgb(color: u32) -> [u8; 3] {
    [(color >> 16) as u8, (color >> 8) as u8, color as u8]
}

/// How the signed content of a digital signature is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SignatureState {
    /// It covers the whole document and the content matches.
    Intact,
    /// The signed content matches, but changes were added afterwards.
    ChangedAfter,
    /// The content does not match the signature: the document was altered.
    Broken,
    /// It could not be checked.
    Unknown,
}

/// A digital signature the document already has.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DigitalSignatureInfo {
    pub signer: Option<String>,
    pub issuer: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub time: Option<String>,
    pub reason: Option<String>,
    pub sub_filter: Option<String>,
    /// Certification (DocMDP): 1 no changes, 2 filling in and signing, 3 also
    /// comments. None: approval signature.
    pub certification: Option<u32>,
    pub state: SignatureState,
}

/// Keys of the handwriting styles for typed signatures.
#[uniffi::export]
pub fn signature_styles() -> Vec<String> {
    TypeStyle::ALL.iter().map(|s| s.key().to_string()).collect()
}

/// Image of a typed signature not saved yet (to choose the style).
/// Uses PDFium: call it from the PDFium thread.
#[uniffi::export]
pub fn preview_typed(text: String, style: String, initials: bool, color: u32, height: u32) -> Option<SignatureImage> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let style = style_of(&style);
    let kind = if initials { SignatureKind::Initials { text, style } } else { SignatureKind::Typed { text, style } };
    image(&SavedSignature { id: "preview".into(), kind, color: rgb(color) }, height)
}

/// Initials of a name: "Juan Pérez García" → "JPG".
#[uniffi::export]
pub fn name_initials(name: String) -> String {
    core_sign::library::initials_of(&name)
}

/// The signatures saved on this device, in a folder of the app.
#[derive(uniffi::Object)]
pub struct SignatureLibrary {
    lib: core_sign::Library,
}

impl SignatureLibrary {
    fn get(&self, id: &str) -> Option<SavedSignature> {
        self.lib.get(id)
    }
}

#[uniffi::export]
impl SignatureLibrary {
    #[uniffi::constructor]
    pub fn new(dir: String) -> Arc<Self> {
        Arc::new(SignatureLibrary { lib: core_sign::Library::new(dir) })
    }

    /// Oldest first.
    pub fn list(&self) -> Vec<SignatureInfo> {
        self.lib.list().into_iter().map(|s| SignatureInfo { initials: s.is_initials(), id: s.id }).collect()
    }

    /// Image of a saved signature, `height` pixels high. Uses PDFium: call
    /// it from the PDFium thread.
    pub fn preview(&self, id: String, height: u32) -> Option<SignatureImage> {
        image(&self.get(&id)?, height)
    }

    /// Saves a typed signature (name or initials); returns its id.
    pub fn add_typed(&self, text: String, style: String, initials: bool, color: u32) -> Result<String, PdfError> {
        let style = style_of(&style);
        let kind = if initials { SignatureKind::Initials { text, style } } else { SignatureKind::Typed { text, style } };
        self.lib.add(kind, rgb(color)).map(|s| s.id).map_err(|e| PdfError::Sign(format!("{e:#}")))
    }

    /// Saves a drawn signature; returns its id.
    pub fn add_drawn(&self, strokes: Vec<InkStroke>, color: u32) -> Result<String, PdfError> {
        let strokes = strokes
            .into_iter()
            .map(|s| core_sign::Stroke { width: s.width, points: s.points.as_chunks::<2>().0.iter().map(|&[x, y]| (x, y)).collect() })
            .collect();
        self.lib.add(SignatureKind::Drawn { strokes }, rgb(color)).map(|s| s.id).map_err(|e| PdfError::Sign(format!("{e:#}")))
    }

    pub fn remove(&self, id: String) -> bool {
        self.lib.remove(&id).is_ok()
    }
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
            text_input: doc.editing_text() || doc.editing_field() && u.caret.is_some(),
            open_files: u.open_files.into_iter().map(|p| p.display().to_string()).collect(),
            uris: u.uris,
            submit: u.submit_requested,
            field: None,
        }
    }

    /// `finish`, plus the rectangle of the text field that has the cursor.
    fn finish_with_field(doc: &mut Document, page_w: f64, page_h: f64) -> InputResult {
        let mut r = Self::finish(doc);
        if r.text_input
            && let Some((p, b)) = doc.focused_text_field()
        {
            let (x0, y0) = doc.page_to_device(p, page_w, page_h, b[0], b[1]);
            let (x1, y1) = doc.page_to_device(p, page_w, page_h, b[2], b[3]);
            r.field = Some(PageRect { left: x0.min(x1) as f32, top: y0.min(y1) as f32, right: x0.max(x1) as f32, bottom: y0.max(y1) as f32 });
        }
        r
    }

    fn pointer(&self, page: u32, page_w: f64, page_h: f64, x: f64, y: f64, f: impl FnOnce(&mut Document, usize, f64, f64)) -> InputResult {
        let mut doc = self.lock();
        let p = usize_page(page);
        let (px, py) = Self::to_page(&doc, p, page_w, page_h, x, y);
        doc.set_current_page(p);
        f(&mut doc, p, px, py);
        Self::finish_with_field(&mut doc, page_w, page_h)
    }
}

#[uniffi::export]
impl PdfDocument {
    /// Opens a PDF. `continuous_xfa`: show dynamic XFA forms as one long
    /// page (recommended; PDFium's XFA pagination loses sections).
    /// `password`: for PDFs that need one to open (`PdfError::Password`
    /// asks for it, `PdfError::WrongPassword` says it was not correct).
    #[uniffi::constructor]
    pub fn open(path: String, continuous_xfa: bool, password: Option<String>) -> Result<Arc<Self>, PdfError> {
        pdfium::init();
        let doc = Document::open_with(Path::new(&path), OpenOptions { continuous_xfa, password }).map_err(|e| match e.downcast_ref::<PasswordRequired>() {
            Some(p) if p.wrong => PdfError::WrongPassword(e.to_string()),
            Some(_) => PdfError::Password(e.to_string()),
            None => PdfError::Open(format!("{e:#}")),
        })?;
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
        self.pointer(page, page_w, page_h, x, y, |doc, p, px, py| {
            doc.mouse_move(p, px, py, 0);
            doc.left_down(p, px, py, 0);
            doc.left_up(p, px, py, 0);
        })
    }

    /// Mouse button (or finger) down at (`x`, `y`), as in `tap`. Until
    /// `release`, `drag` selects text in a text field.
    pub fn press(&self, page: u32, page_w: f64, page_h: f64, x: f64, y: f64) -> InputResult {
        self.pointer(page, page_w, page_h, x, y, |doc, p, px, py| {
            doc.mouse_move(p, px, py, 0);
            doc.left_down(p, px, py, 0);
        })
    }

    /// Moves the pressed button to (`x`, `y`).
    pub fn drag(&self, page: u32, page_w: f64, page_h: f64, x: f64, y: f64) -> InputResult {
        self.pointer(page, page_w, page_h, x, y, |doc, p, px, py| {
            doc.mouse_move(p, px, py, keys::MOD_LBUTTON);
        })
    }

    /// Releases the button pressed with `press`.
    pub fn release(&self, page: u32, page_w: f64, page_h: f64, x: f64, y: f64) -> InputResult {
        self.pointer(page, page_w, page_h, x, y, |doc, p, px, py| {
            doc.left_up(p, px, py, 0);
        })
    }

    /// Double click: selects the word in a text field.
    pub fn double_tap(&self, page: u32, page_w: f64, page_h: f64, x: f64, y: f64) -> InputResult {
        self.pointer(page, page_w, page_h, x, y, |doc, p, px, py| {
            doc.left_double_click(p, px, py, 0);
        })
    }

    /// Text selected in the field being edited.
    pub fn selected_text(&self) -> String {
        self.lock().selected_text()
    }

    /// Replaces the text selected in the field being edited (empty: deletes
    /// it; with no selection: inserts at the cursor).
    pub fn replace_selection(&self, text: String) -> InputResult {
        let mut doc = self.lock();
        doc.replace_selection(&text);
        let mut r = Self::finish(&mut doc);
        r.modified = true;
        r
    }

    /// Selects all the text of the field being edited.
    pub fn select_all_in_field(&self) -> InputResult {
        let mut doc = self.lock();
        doc.select_all();
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
        self.key_with(key, false, false)
    }

    /// A special key with Shift (extends the selection with the arrows,
    /// Home and End) and/or Ctrl (moves by words, or to the start or end).
    pub fn key_with(&self, key: Key, shift: bool, ctrl: bool) -> InputResult {
        let mut doc = self.lock();
        let mods = if shift { keys::MOD_SHIFT } else { 0 } | if ctrl { keys::MOD_CTRL } else { 0 };
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
        doc.key_down(vk, mods);
        if let Some(c) = keys::char_for_vk(vk) {
            doc.char_input(c as u32, mods);
        }
        doc.key_up(vk, mods);
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

    /// The digital signatures of the document, oldest first.
    pub fn digital_signatures(&self) -> Vec<DigitalSignatureInfo> {
        use form_pdf_reader::pdfium::SignedState;
        self.lock()
            .digital_signatures()
            .into_iter()
            .map(|s| DigitalSignatureInfo {
                signer: s.signer,
                issuer: s.issuer,
                valid_from: s.valid_from,
                valid_to: s.valid_to,
                time: s.time,
                reason: s.reason,
                sub_filter: s.sub_filter,
                certification: s.certification,
                state: match s.state {
                    SignedState::Intact => SignatureState::Intact,
                    SignedState::ChangedAfter => SignatureState::ChangedAfter,
                    SignedState::Broken => SignatureState::Broken,
                    SignedState::Unknown => SignatureState::Unknown,
                },
            })
            .collect()
    }

    /// It has the Adobe Reader usage rights (filling in and saving in Reader).
    pub fn has_usage_rights(&self) -> bool {
        self.lock().has_usage_rights()
    }

    /// Handwritten signatures can be placed on the pages (not on dynamic
    /// XFA forms).
    pub fn can_sign(&self) -> bool {
        self.lock().can_sign()
    }

    /// Places signature `id` of `library` centred at (x, y), in points of
    /// the page from its top left corner. Returns its index.
    pub fn add_signature(&self, library: Arc<SignatureLibrary>, id: String, page: u32, x: f32, y: f32) -> Result<u32, PdfError> {
        let sig = library.get(&id).ok_or_else(|| PdfError::Sign("no such signature".into()))?;
        let mut doc = self.lock();
        let p = usize_page(page);
        let (w, h) = doc.page_size(p);
        let (px, py) = doc.device_to_page(p, w as f64, h as f64, x as f64, y as f64);
        let r = doc.signature_rect_at(p, px as f32, py as f32, &sig);
        doc.add_signature(p, r, &sig).map(|i| i as u32).map_err(|e| PdfError::Sign(format!("{e:#}")))
    }

    /// Moves or resizes a placed signature; `rect` in points of its page
    /// from the top left corner.
    pub fn move_signature(&self, index: u32, rect: PageRect) -> Result<(), PdfError> {
        let mut doc = self.lock();
        let Some(p) = doc.placed_signatures().get(index as usize).map(|s| s.page) else {
            return Err(PdfError::Sign("no such signature".into()));
        };
        let (w, h) = doc.page_size(p);
        let (x0, y0) = doc.device_to_page(p, w as f64, h as f64, rect.left as f64, rect.top as f64);
        let (x1, y1) = doc.device_to_page(p, w as f64, h as f64, rect.right as f64, rect.bottom as f64);
        let r = [x0.min(x1) as f32, y0.min(y1) as f32, x0.max(x1) as f32, y0.max(y1) as f32];
        let r = doc.clamp_to_page(p, r);
        doc.move_signature(index as usize, r).map_err(|e| PdfError::Sign(format!("{e:#}")))
    }

    pub fn remove_signature(&self, index: u32) {
        self.lock().remove_signature(index as usize);
    }

    /// Signatures placed and not yet written (saving writes them).
    pub fn placed_signatures(&self) -> Vec<PlacedSignatureInfo> {
        let doc = self.lock();
        doc.placed_signatures()
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (w, h) = doc.page_size(s.page);
                let [l, b, r, t] = s.rect;
                let (x0, y0) = doc.page_to_device(s.page, w as f64, h as f64, l as f64, t as f64);
                let (x1, y1) = doc.page_to_device(s.page, w as f64, h as f64, r as f64, b as f64);
                PlacedSignatureInfo {
                    index: i as u32,
                    page: s.page as u32,
                    rect: PageRect { left: x0.min(x1) as f32, top: y0.min(y1) as f32, right: x0.max(x1) as f32, bottom: y0.max(y1) as f32 },
                    id: s.signature.id.clone(),
                }
            })
            .collect()
    }

    /// Sheets the printed document would have (a continuous XFA form is
    /// cut into A4 sheets).
    pub fn print_sheet_count(&self) -> u32 {
        self.lock().print_layout().len() as u32
    }

    /// Writes the PDF to print to `path`: every sheet as an image at `dpi`,
    /// with the form data and the placed signatures (which are written into
    /// the document, as when saving).
    pub fn export_print(&self, path: String, dpi: f32) -> Result<(), PdfError> {
        self.lock().export_flat(Path::new(&path), dpi).map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    /// Locked by a digital signature: it accepts no changes nor more
    /// signatures (certified with no changes allowed, or a field lock).
    pub fn locked_by_signature(&self) -> bool {
        self.lock().locked_by_signature()
    }

    /// The signature fields of the document (empty or signed).
    pub fn signature_fields(&self) -> Vec<SignatureFieldInfo> {
        self.lock().signature_fields().into_iter().map(|f| SignatureFieldInfo { name: f.name, page: f.page.map(|p| p as u32), signed: f.signed }).collect()
    }

    /// Prepares a digital signature of the document as it is now (with the
    /// form data and the handwritten signatures), with the certificate
    /// `chain` (signer first). The returned object gives what the key has
    /// to sign; `finish` with that signature gives the signed PDF.
#[allow(clippy::too_many_arguments)]
        pub fn prepare_digital_signature(
        &self,
        placement: DigitalPlacement,
        reason: Option<String>,
        location: Option<String>,
        library: Option<Arc<SignatureLibrary>>,
        handwritten_id: Option<String>,
        chain: Vec<Vec<u8>>,
        lock: bool,
    ) -> Result<Arc<PendingSignature>, PdfError> {
        use form_pdf_reader::pades::{Placement, Request};
        let mut doc = self.lock();
        let placement = match placement {
            DigitalPlacement::Invisible => Placement::Invisible,
            DigitalPlacement::Field { name } => Placement::Field(name),
            DigitalPlacement::New { page, x, y, width, height } => {
                let p = usize_page(page);
                let (w, h) = doc.page_size(p);
                let (x0, y0) = doc.device_to_page(p, w as f64, h as f64, (x - width / 2.0) as f64, (y - height / 2.0) as f64);
                let (x1, y1) = doc.device_to_page(p, w as f64, h as f64, (x + width / 2.0) as f64, (y + height / 2.0) as f64);
                let r = [x0.min(x1) as f32, y0.min(y1) as f32, x0.max(x1) as f32, y0.max(y1) as f32];
                Placement::New { page: p, rect: doc.clamp_to_page(p, r) }
            }
        };
        let handwritten = match (library, handwritten_id) {
            (Some(l), Some(id)) => l.get(&id),
            _ => None,
        };
        let req = Request { placement, reason: reason.filter(|s| !s.trim().is_empty()), location: location.filter(|s| !s.trim().is_empty()), handwritten, timestamp: None, lock };
        let prepared = doc.prepare_digital(&req, &chain).map_err(|e| PdfError::Sign(format!("{e:#}")))?;
        let to_sign = prepared.data_to_sign().to_vec();
        Ok(Arc::new(PendingSignature { prepared: Mutex::new(Some(prepared)), to_sign }))
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

    // ---- Annotations ----

    /// Annotations can be added (not to XFA forms).
    pub fn can_annotate(&self) -> bool {
        self.lock().can_annotate()
    }

    /// The annotations of a page that can be edited or removed.
    pub fn annotations(&self, page: u32) -> Vec<AnnotationInfo> {
        let doc = self.lock();
        let p = usize_page(page);
        let (w, h) = doc.page_size(p);
        doc.annotations(p)
            .into_iter()
            .map(|a| {
                let [l, b, r, t] = a.rect;
                let (x0, y0) = doc.page_to_device(p, w as f64, h as f64, l as f64, t as f64);
                let (x1, y1) = doc.page_to_device(p, w as f64, h as f64, r as f64, b as f64);
                AnnotationInfo {
                    page,
                    index: a.index as u32,
                    kind: a.kind.into(),
                    rect: PageRect { left: x0.min(x1) as f32, top: y0.min(y1) as f32, right: x0.max(x1) as f32, bottom: y0.max(y1) as f32 },
                    contents: a.contents,
                    color: (a.color[0] as u32) << 16 | (a.color[1] as u32) << 8 | a.color[2] as u32,
                    font_size: a.font_size,
                }
            })
            .collect()
    }

    /// The topmost annotation at (x, y), in points from the top left corner.
    pub fn annotation_at(&self, page: u32, x: f32, y: f32) -> Option<AnnotationInfo> {
        let (px, py) = self.point(page, x, y);
        let index = self.lock().annotation_at(usize_page(page), px, py)?.index as u32;
        self.annotations(page).into_iter().find(|a| a.index == index)
    }

    /// Highlights, underlines or strikes out characters [start, start + count).
    pub fn add_markup(&self, page: u32, kind: AnnotationKind, start: i32, count: i32, color: u32) -> Result<(), PdfError> {
        let mut doc = self.lock();
        let p = usize_page(page);
        let rects = doc.text_rects(p, start, count);
        let text = doc.text_range(p, start, count);
        doc.add_markup(p, kind.into(), &rects, rgb(color), text.trim()).map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    /// A note with its top left corner at (x, y), in points from the top left.
    pub fn add_note(&self, page: u32, x: f32, y: f32, text: String, color: u32) -> Result<(), PdfError> {
        let (px, py) = self.point(page, x, y);
        self.lock().add_note(usize_page(page), px, py, &text, rgb(color)).map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    /// Text written on the page, its top left corner at (x, y).
    pub fn add_free_text(&self, page: u32, x: f32, y: f32, text: String, size: f32, color: u32) -> Result<(), PdfError> {
        let (px, py) = self.point(page, x, y);
        self.lock().add_free_text(usize_page(page), px, py, &text, size, rgb(color)).map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    /// A drawing: strokes with points in points of the page from its top
    /// left corner, `width` points wide.
    pub fn add_ink(&self, page: u32, strokes: Vec<InkStroke>, width: f32, color: u32) -> Result<(), PdfError> {
        let mut doc = self.lock();
        let p = usize_page(page);
        let (w, h) = doc.page_size(p);
        let strokes: Vec<Vec<[f32; 2]>> = strokes
            .iter()
            .map(|s| {
                s.points
                    .chunks_exact(2)
                    .map(|c| {
                        let (x, y) = doc.device_to_page(p, w as f64, h as f64, c[0] as f64, c[1] as f64);
                        [x as f32, y as f32]
                    })
                    .collect()
            })
            .collect();
        doc.add_ink(p, &strokes, width, rgb(color)).map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    /// Changes the comment of a note or the text of a free text.
    pub fn set_annotation_text(&self, page: u32, index: u32, text: String) -> Result<(), PdfError> {
        self.lock().set_annotation_text(usize_page(page), index as usize, &text).map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    pub fn remove_annotation(&self, page: u32, index: u32) -> Result<(), PdfError> {
        self.lock().remove_annotation(usize_page(page), index as usize).map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    // ---- Pages ----

    /// The pages can be organised (not in XFA forms).
    pub fn can_edit_pages(&self) -> bool {
        self.lock().can_edit_pages()
    }

    /// The document (with its form data, annotations and placed signatures)
    /// with the pages changed: open it again from these bytes.
    pub fn edit_pages(&self, edits: Vec<PageEditOp>) -> Result<Vec<u8>, PdfError> {
        let mut doc = self.lock();
        let bytes = doc.bytes_for_save().map_err(|e| PdfError::Save(format!("{e:#}")))?;
        let edits: Vec<PageEdit> = edits.into_iter().map(Into::into).collect();
        core_pages::edit_pages(&bytes, doc.password(), &edits).map_err(|e| PdfError::Save(format!("{e:#}")))
    }

    /// A new PDF with these pages (in this order).
    pub fn extract_pages(&self, pages: Vec<u32>) -> Result<Vec<u8>, PdfError> {
        let mut doc = self.lock();
        let bytes = doc.bytes_for_save().map_err(|e| PdfError::Save(format!("{e:#}")))?;
        let pages: Vec<usize> = pages.into_iter().map(|p| p as usize).collect();
        core_pages::extract_pages(&bytes, doc.password(), &pages).map_err(|e| PdfError::Save(format!("{e:#}")))
    }
}

impl PdfDocument {
    /// Page coordinates of (x, y) in points from the page's top left corner.
    fn point(&self, page: u32, x: f32, y: f32) -> (f32, f32) {
        let doc = self.lock();
        let p = usize_page(page);
        let (w, h) = doc.page_size(p);
        let (px, py) = doc.device_to_page(p, w as f64, h as f64, x as f64, y as f64);
        (px as f32, py as f32)
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
        assert!(PdfDocument::open("/no/existe.pdf".into(), true, None).is_err());

        // Normal PDF: pages, render, text.
        let d = PdfDocument::open(fixture("normal.pdf"), true, None).unwrap();
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
        let f = PdfDocument::open(fixture("acroform.pdf"), true, None).unwrap();
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
        let g = PdfDocument::open(out.display().to_string(), true, None).unwrap();
        g.tap(0, pw, ph, x, y);
        assert_eq!(g.focused_text(), "Hola Androi");
        let _ = std::fs::remove_file(out);
    }
}
