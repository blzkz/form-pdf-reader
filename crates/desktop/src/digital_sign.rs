//! El diálogo de "Firmar con certificado": elegir el certificado (del
//! sistema o un fichero), el PIN o la contraseña, si la firma se ve en la
//! página (y dónde, y con la firma manuscrita) o no, y el motivo y el lugar.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, RichText, Stroke, Vec2};
use form_pdf_reader::pades::SignatureField;
use form_pdf_reader::sign::{SavedSignature, SignatureKind};
use form_pdf_reader::t;

use crate::certs::{self, CertEntry, Secret};
use crate::theme::{self, icon};

/// Lo elegido, para firmar.
pub struct SignPlan {
    pub cert: CertEntry,
    pub secret: String,
    pub visible: bool,
    /// Campo de firma del documento; None: elegir el sitio en la página.
    pub field: Option<String>,
    pub handwritten: Option<SavedSignature>,
    pub reason: String,
    pub location: String,
    /// Servidor de sellado de tiempo, si se pide sello.
    pub timestamp: Option<String>,
    /// Lo elegido para el sello (para recordarlo).
    pub timestamp_on: bool,
    pub timestamp_server: String,
}

pub enum Outcome {
    Closed,
    Sign(Box<SignPlan>),
}

pub struct DigitalSignDialog {
    loading: Option<Receiver<Vec<CertEntry>>>,
    certs: Vec<CertEntry>,
    selected: Option<usize>,
    secret: String,
    /// Fichero elegido que aún pide su contraseña.
    pending_file: Option<PathBuf>,
    file_password: String,
    file_error: Option<String>,
    /// Poner el foco en la contraseña del fichero.
    focus_password: bool,
    /// Contraseñas de los ficheros ya abiertos.
    file_passwords: std::collections::HashMap<PathBuf, String>,
    visible: bool,
    /// Campos de firma vacíos del documento.
    fields: Vec<SignatureField>,
    field: Option<String>,
    saved: Vec<SavedSignature>,
    handwritten: Option<usize>,
    reason: String,
    location: String,
    dynamic_xfa: bool,
    timestamp: bool,
    timestamp_server: String,
}

impl DigitalSignDialog {
    pub fn new(fields: Vec<SignatureField>, saved: Vec<SavedSignature>, dynamic_xfa: bool, timestamp: bool, timestamp_server: Option<String>) -> DigitalSignDialog {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(certs::system_certificates());
        });
        let fields: Vec<SignatureField> = fields.into_iter().filter(|f| !f.signed).collect();
        DigitalSignDialog {
            loading: Some(rx),
            certs: Vec::new(),
            selected: None,
            secret: String::new(),
            pending_file: None,
            file_password: String::new(),
            file_error: None,
            focus_password: false,
            file_passwords: Default::default(),
            // En XFA dinámico Adobe no enseña anotaciones: invisible.
            visible: !dynamic_xfa,
            field: fields.first().map(|f| f.name.clone()),
            fields,
            saved,
            handwritten: None,
            reason: String::new(),
            location: String::new(),
            dynamic_xfa,
            timestamp,
            timestamp_server: timestamp_server.unwrap_or_else(|| form_pdf_reader::pades::tsa::DEFAULT_TSA.to_string()),
        }
    }

    /// Añade y elige un certificado leído de un fichero.
    pub fn add_file(&mut self, entry: CertEntry, password: String) {
        if let certs::Source::File(p) = &entry.source {
            self.file_passwords.insert(p.clone(), password);
        }
        self.certs.push(entry);
        self.selected = Some(self.certs.len() - 1);
        self.secret.clear();
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(list) => {
                    self.loading = None;
                    let files = std::mem::take(&mut self.certs);
                    self.certs = list;
                    self.certs.extend(files);
                    if self.selected.is_none() {
                        self.selected = self.certs.iter().position(|c| !c.info.expired);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.loading = None,
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        let mut out = None;
        let r = theme::modal(ctx, "firma_digital").show(ctx, |ui| {
            let p = theme::palette(ui);
            ui.set_width(600.0);
            ui.heading(t!("dsig.title"));
            ui.add_space(12.0);
            ui.label(RichText::new(t!("dsig.certificate")).strong());
            ui.add_space(4.0);
            self.cert_list(ui, &p);
            self.file_row(ui, &p);
            self.secret_row(ui, &p);
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            self.options(ui, &p);
            out = self.footer(ui);
        });
        if out.is_none() && r.should_close() {
            out = Some(Outcome::Closed);
        }
        out
    }

    fn cert_list(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        if self.loading.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(t!("dsig.searching")).color(p.text_weak));
            });
        } else if self.certs.is_empty() {
            ui.label(RichText::new(t!("dsig.none")).color(p.text_weak));
        }
        egui::ScrollArea::vertical().max_height(230.0).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            for (i, c) in self.certs.iter().enumerate() {
                let selected = self.selected == Some(i);
                let stroke = if selected { Stroke::new(2.0, p.accent) } else { Stroke::new(1.0, p.border) };
                let frame = egui::Frame::new().stroke(stroke).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 8));
                let resp = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let glyph = if matches!(c.source, certs::Source::File(_)) { icon::FILE_LOCK } else { icon::CERTIFICATE };
                            ui.label(RichText::new(glyph).size(22.0).color(if c.info.expired { p.text_weak } else { p.accent_text }));
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(RichText::new(&c.info.subject).strong().color(if c.info.expired { p.text_weak } else { p.text }));
                                let valid = if c.info.expired { RichText::new(t!("dsig.expired", &c.info.valid_to)).color(p.danger) } else { RichText::new(t!("dsig.valid_to", &c.info.valid_to)).color(p.text_weak) };
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    ui.label(RichText::new(format!("{} · {} ·", c.origin, t!("dsig.issuer", &c.info.issuer))).small().color(p.text_weak));
                                    ui.label(valid.small());
                                });
                            });
                        });
                    })
                    .response;
                if ui.interact(resp.rect, egui::Id::new(("certificado", i)), egui::Sense::click()).clicked() && !c.info.expired && self.selected != Some(i) {
                    self.selected = Some(i);
                    self.secret.clear();
                }
            }
        });
    }

    fn file_row(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        ui.add_space(4.0);
        if let Some(path) = self.pending_file.clone() {
            // Contraseña del fichero recién elegido.
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(t!("dsig.file_password", name));
            let mut open = false;
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.file_password).password(true).desired_width(260.0).margin(egui::Margin::symmetric(8, 5)));
                if std::mem::take(&mut self.focus_password) {
                    r.request_focus();
                }
                open = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                open |= theme::dialog_button(ui, t!("dsig.open_file"), false, true).clicked();
                if ui.button(t!("dialog.cancel")).clicked() {
                    self.pending_file = None;
                    self.file_error = None;
                }
            });
            if open {
                match certs::open_file(&path, &self.file_password) {
                    Ok(e) => {
                        let pw = std::mem::take(&mut self.file_password);
                        self.add_file(e, pw);
                        self.pending_file = None;
                        self.file_error = None;
                    }
                    Err(e) => {
                        self.file_error = Some(format!("{e:#}"));
                        self.file_password.clear();
                        self.focus_password = true;
                    }
                }
            }
            if let Some(e) = &self.file_error {
                ui.label(RichText::new(e).color(p.danger).small());
            }
        } else if ui.add(egui::Button::new(format!("{}  {}", icon::FOLDER_OPEN, t!("dsig.use_file")))).clicked()
            && let Some(f) = pick_p12()
        {
            self.pending_file = Some(f);
            self.file_password.clear();
            self.file_error = None;
            self.focus_password = true;
        }
    }

    fn secret_row(&mut self, ui: &mut egui::Ui, _p: &theme::Palette) {
        let Some(c) = self.selected.and_then(|i| self.certs.get(i)) else { return };
        let label = match (&c.secret, &c.source) {
            (Secret::Pin, _) => t!("dsig.pin"),
            (Secret::Password, certs::Source::System(_)) => t!("dsig.master_password"),
            _ => return,
        };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(label);
            ui.add(egui::TextEdit::singleline(&mut self.secret).password(true).desired_width(200.0).margin(egui::Margin::symmetric(8, 5)));
        });
    }

    fn options(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        let label_w = 165.0;
        let row = |ui: &mut egui::Ui, label: String, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(label_w, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_size(Vec2::new(label_w, 28.0));
                    ui.label(label);
                });
                add(ui);
            });
        };
        ui.spacing_mut().item_spacing.y = 8.0;
        row(ui, t!("dsig.appearance"), &mut |ui| {
            ui.add_enabled_ui(!self.dynamic_xfa, |ui| {
                theme::segmented(ui, &mut self.visible, &[(true, icon::EYE, t!("dsig.visible"), t!("dsig.visible_tip")), (false, icon::EYE_SLASH, t!("dsig.invisible"), t!("dsig.invisible_tip"))], true);
            });
        });
        if self.visible {
            row(ui, t!("dsig.where"), &mut |ui| {
                let shown = match &self.field {
                    None => t!("dsig.where_place"),
                    Some(n) => field_label(&self.fields, n),
                };
                egui::ComboBox::from_id_salt("donde_firma").width(330.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.field, None, t!("dsig.where_place"));
                    for f in &self.fields {
                        ui.selectable_value(&mut self.field, Some(f.name.clone()), field_label(&self.fields, &f.name));
                    }
                });
            });
            row(ui, t!("dsig.handwritten"), &mut |ui| {
                let label = |s: &SavedSignature| match &s.kind {
                    SignatureKind::Typed { text, .. } | SignatureKind::Initials { text, .. } => text.clone(),
                    SignatureKind::Drawn { .. } => t!("dsig.drawn"),
                };
                let shown = self.handwritten.and_then(|i| self.saved.get(i)).map(label).unwrap_or_else(|| t!("dsig.handwritten_none"));
                egui::ComboBox::from_id_salt("manuscrita").width(330.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.handwritten, None, t!("dsig.handwritten_none"));
                    for (i, s) in self.saved.iter().enumerate() {
                        ui.selectable_value(&mut self.handwritten, Some(i), label(s));
                    }
                });
            });
        }
        row(ui, t!("dsig.reason"), &mut |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text(t!("dsig.optional")).desired_width(330.0).margin(egui::Margin::symmetric(8, 5)));
        });
        row(ui, t!("dsig.location"), &mut |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.location).hint_text(t!("dsig.optional")).desired_width(330.0).margin(egui::Margin::symmetric(8, 5)));
        });
        // La casilla con su texto, en la columna de las etiquetas.
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(Vec2::new(label_w, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_min_size(Vec2::new(label_w, 28.0));
                // Esquinas menos redondeadas: con las de los botones, la
                // casilla parecía un círculo.
                ui.visuals_mut().widgets.inactive.corner_radius = egui::CornerRadius::same(3);
                ui.visuals_mut().widgets.hovered.corner_radius = egui::CornerRadius::same(3);
                ui.visuals_mut().widgets.active.corner_radius = egui::CornerRadius::same(3);
                ui.checkbox(&mut self.timestamp, t!("dsig.timestamp")).on_hover_text(t!("dsig.timestamp_tip"));
            });
            ui.add_enabled(self.timestamp, egui::TextEdit::singleline(&mut self.timestamp_server).desired_width(330.0).margin(egui::Margin::symmetric(8, 5)));
        });
        if self.timestamp {
            ui.label(RichText::new(format!("{}  {}", icon::INFO, t!("dsig.timestamp_note"))).small().color(p.text_weak));
        }
        if self.dynamic_xfa {
            ui.label(RichText::new(format!("{}  {}", icon::INFO, t!("dsig.note_xfa"))).small().color(p.text_weak));
        }
        ui.label(RichText::new(format!("{}  {}", icon::INFO, t!("dsig.note_copy"))).small().color(p.text_weak));
    }

    fn footer(&mut self, ui: &mut egui::Ui) -> Option<Outcome> {
        let cert = self.selected.and_then(|i| self.certs.get(i));
        let is_file = |c: &CertEntry| matches!(c.source, certs::Source::File(_));
        let ready = cert.is_some_and(|c| !c.info.expired && (c.secret == Secret::None || is_file(c) || !self.secret.is_empty()));
        match theme::dialog_footer(ui, &[(format!("{}  {}", icon::SEAL_CHECK, t!("dsig.sign")), true, ready), (t!("dialog.cancel"), false, true)]) {
            Some(0) => {
                let cert = cert?.clone();
                let secret = match &cert.source {
                    certs::Source::File(p) => self.file_passwords.get(p).cloned().unwrap_or_default(),
                    certs::Source::System(_) => self.secret.clone(),
                };
                Some(Outcome::Sign(Box::new(SignPlan {
                    cert,
                    secret,
                    visible: self.visible,
                    field: if self.visible { self.field.clone() } else { None },
                    handwritten: if self.visible { self.handwritten.and_then(|i| self.saved.get(i).cloned()) } else { None },
                    reason: self.reason.trim().to_string(),
                    location: self.location.trim().to_string(),
                    timestamp: (self.timestamp && !self.timestamp_server.trim().is_empty()).then(|| self.timestamp_server.trim().to_string()),
                    timestamp_on: self.timestamp,
                    timestamp_server: self.timestamp_server.trim().to_string(),
                })))
            }
            Some(_) => Some(Outcome::Closed),
            None => None,
        }
    }
}

fn field_label(fields: &[SignatureField], name: &str) -> String {
    match fields.iter().find(|f| f.name == name).and_then(|f| f.page) {
        Some(p) => t!("dsig.field_page", name, p + 1),
        None => t!("dsig.field", name),
    }
}

/// Elegir un fichero de certificado.
fn pick_p12() -> Option<PathBuf> {
    if crate::autotest::active()
        && let Ok(p) = std::env::var("PDFRE_AUTOTEST_P12")
    {
        return Some(PathBuf::from(p));
    }
    rfd::FileDialog::new().add_filter(t!("dsig.p12_filter"), &["p12", "pfx", "P12", "PFX"]).set_title(t!("dsig.use_file")).pick_file()
}
