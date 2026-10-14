//! Imprimir: el diálogo (impresora, páginas, copias, color, doble cara y una
//! vista previa) y el trabajo de impresión, que va hoja a hoja para enseñar
//! el progreso sin bloquear la ventana.
//!
//! Lo que se imprime son las hojas del documento tal como se ven (con los
//! datos de los formularios y las firmas), igual que "Exportar PDF plano".
//! En Linux se mandan a CUPS como un PDF (`lp`); en Windows se dibujan en la
//! impresora con GDI.

use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Color32, RichText, TextureHandle, TextureOptions, Vec2};
use form_pdf_reader::pdfium::{Document, Sheet, parse_page_ranges};
use form_pdf_reader::t;

use crate::theme::{self, icon};

#[cfg(not(windows))]
#[path = "print/cups.rs"]
mod backend;
#[cfg(windows)]
#[path = "print/win.rs"]
mod backend;

/// Una impresora instalada.
#[derive(Debug, Clone, PartialEq)]
pub struct Printer {
    /// Nombre para el sistema.
    pub name: String,
    /// Nombre para enseñar.
    pub label: String,
    pub is_default: bool,
    pub color: bool,
    pub duplex: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplex {
    Off,
    /// Pasar página por el borde largo (lo normal en vertical).
    LongEdge,
    ShortEdge,
}

/// Lo elegido en el diálogo.
#[derive(Debug, Clone)]
pub struct Options {
    pub printer: String,
    pub copies: u32,
    pub color: bool,
    pub duplex: Duplex,
    /// Nombre del trabajo (el del documento).
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pages {
    All,
    Current,
    Range,
}

/// Resolución con que se mandan las hojas a CUPS, y la máxima en Windows.
pub const PRINT_DPI: f32 = 300.0;

/// La hoja a `dpi`, en RGBA, en gris si `gray`.
pub fn render(doc: &Document, sheet: &Sheet, dpi: f32, gray: bool) -> Option<(i32, i32, Vec<u8>)> {
    let (w, h, mut px) = doc.render_sheet(sheet, dpi)?;
    if gray {
        for p in px.as_chunks_mut::<4>().0 {
            let y = ((p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000) as u8;
            p[0] = y;
            p[1] = y;
            p[2] = y;
        }
    }
    Some((w, h, px))
}

/// Cómo acabó el diálogo.
pub enum Outcome {
    /// Se cerró sin imprimir.
    Closed,
    /// Trabajo enviado: el mensaje para la barra de estado.
    Sent(String),
    Failed(String),
    /// Abrir el PDF plano en el visor del sistema (lo de antes).
    OpenExternal,
}

/// Trabajo en marcha: las hojas que faltan por mandar.
struct Job {
    spool: backend::Spool,
    sheets: Vec<Sheet>,
    /// Veces que hay que mandar las hojas (si la impresora no hace las copias).
    rounds: u32,
    done: usize,
    gray: bool,
    copies: u32,
    printer: String,
}

pub struct PrintDialog {
    loading: Option<Receiver<Result<Vec<Printer>, String>>>,
    printers: Result<Vec<Printer>, String>,
    selected: usize,
    copies: u32,
    pages: Pages,
    range: String,
    color: bool,
    duplex: Duplex,
    /// Las hojas del documento y la que se ve ahora.
    layout: Vec<Sheet>,
    current: usize,
    continuous: bool,
    title: String,
    preview_at: usize,
    preview: Option<(usize, bool, TextureHandle)>,
    /// Ajustes propios del controlador (Windows), por impresora.
    driver: backend::DriverSettings,
    last_printer: Option<String>,
    job: Option<Job>,
    first_frame: bool,
}

impl PrintDialog {
    pub fn new(doc: &Document, current_page: usize, last_printer: Option<&str>) -> PrintDialog {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(backend::list_printers());
        });
        let layout = doc.print_layout();
        let continuous = doc.is_continuous();
        let title = doc.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "PDF".into());
        PrintDialog {
            loading: Some(rx),
            printers: Ok(Vec::new()),
            selected: 0,
            copies: 1,
            pages: Pages::All,
            range: String::new(),
            color: true,
            duplex: Duplex::Off,
            current: if continuous { 0 } else { current_page.min(layout.len().saturating_sub(1)) },
            layout,
            continuous,
            title,
            preview_at: 0,
            preview: None,
            driver: backend::DriverSettings::default(),
            last_printer: last_printer.map(str::to_string),
            job: None,
            first_frame: true,
        }
    }

    /// La impresora elegida (para recordarla).
    pub fn printer_name(&self) -> Option<String> {
        self.printers.as_ref().ok()?.get(self.selected).map(|p| p.name.clone())
    }

    fn printer(&self) -> Option<&Printer> {
        self.printers.as_ref().ok()?.get(self.selected)
    }

    /// Las hojas elegidas (índices en `layout`), o None si el rango no vale.
    fn chosen(&self) -> Option<Vec<usize>> {
        match self.pages {
            Pages::All => Some((0..self.layout.len()).collect()),
            Pages::Current => Some(vec![self.current]),
            Pages::Range => parse_page_ranges(&self.range, self.layout.len()),
        }
    }

    fn on_printers(&mut self, list: Result<Vec<Printer>, String>) {
        if let Ok(l) = &list {
            let remembered = self.last_printer.as_ref().and_then(|n| l.iter().position(|p| &p.name == n));
            self.selected = remembered.or_else(|| l.iter().position(|p| p.is_default)).unwrap_or(0);
        }
        self.printers = list;
        self.on_printer_changed();
    }

    fn on_printer_changed(&mut self) {
        let Some(p) = self.printer().cloned() else { return };
        if !p.color {
            self.color = false;
        }
        if !p.duplex {
            self.duplex = Duplex::Off;
        }
    }

    /// El diálogo, cada fotograma. Devuelve algo cuando termina.
    pub fn show(&mut self, ctx: &egui::Context, doc: &mut Document) -> Option<Outcome> {
        if let Some(rx) = &self.loading {
            match rx.try_recv() {
                Ok(list) => {
                    self.loading = None;
                    self.on_printers(list);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.loading = None;
                    self.on_printers(Err(t!("print.list_failed")));
                }
                Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        if self.job.is_some() {
            // Una hoja por fotograma: la ventana sigue viva y se ve el progreso.
            if let Some(out) = self.step(doc) {
                return Some(out);
            }
            ctx.request_repaint();
        }

        let mut outcome = None;
        let r = theme::modal(ctx, "imprimir").show(ctx, |ui| {
            let p = theme::palette(ui);
            ui.set_width(620.0);
            ui.heading(t!("print.title"));
            ui.add_space(12.0);
            ui.horizontal_top(|ui| {
                self.preview_ui(ui, doc);
                ui.add_space(18.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    self.form_ui(ui, &p);
                });
            });
            outcome = self.footer_ui(ui, doc, &p);
        });
        if self.job.is_none() && outcome.is_none() && r.should_close() {
            outcome = Some(Outcome::Closed);
        }
        self.first_frame = false;
        outcome
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui, doc: &Document) {
        let p = theme::palette(ui);
        let chosen = self.chosen().unwrap_or_default();
        const BOX: Vec2 = Vec2::new(190.0, 268.0);
        ui.vertical(|ui| {
            let (rect, _) = ui.allocate_exact_size(BOX, egui::Sense::hover());
            ui.painter().rect_filled(rect, 8.0, p.canvas);
            if let Some(&sheet_i) = chosen.get(self.preview_at.min(chosen.len().saturating_sub(1))) {
                let gray = !self.color;
                let stale = self.preview.as_ref().is_none_or(|(i, g, _)| *i != sheet_i || *g != gray);
                if stale && let Some(s) = self.layout.get(sheet_i) {
                    // A la resolución de la pantalla, para que se vea nítida.
                    let ppp = ui.ctx().pixels_per_point();
                    let fit = ((BOX.x - 24.0) / s.width).min((BOX.y - 24.0) / s.height);
                    let dpi = fit * 72.0 * ppp;
                    if let Some((w, h, px)) = render(doc, s, dpi, gray) {
                        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
                        self.preview = Some((sheet_i, gray, ui.ctx().load_texture("vista-impresion", img, TextureOptions::LINEAR)));
                    }
                }
                if let Some((_, _, tex)) = &self.preview {
                    let size = tex.size_vec2() / ui.ctx().pixels_per_point();
                    let r = egui::Rect::from_center_size(rect.center(), size);
                    ui.painter().rect_filled(r.translate(Vec2::new(0.0, 2.0)).expand(1.0), 2.0, Color32::from_black_alpha(40));
                    ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                }
            }
            // Pasar las hojas elegidas.
            ui.add_space(4.0);
            ui.allocate_ui_with_layout(Vec2::new(BOX.x, 30.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let n = chosen.len();
                self.preview_at = self.preview_at.min(n.saturating_sub(1));
                ui.add_enabled_ui(self.preview_at > 0, |ui| {
                    if theme::icon_button(ui, icon::CARET_LEFT, t!("print.preview_prev")).clicked() {
                        self.preview_at -= 1;
                    }
                });
                let text = if n == 0 { "–".to_string() } else { t!("print.preview_of", self.preview_at + 1, n) };
                ui.add_sized([BOX.x - 80.0, 20.0], egui::Label::new(RichText::new(text).color(p.text_weak)));
                ui.add_enabled_ui(self.preview_at + 1 < n, |ui| {
                    if theme::icon_button(ui, icon::CARET_RIGHT, t!("print.preview_next")).clicked() {
                        self.preview_at += 1;
                    }
                });
            });
        });
    }

    fn form_ui(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        let busy = self.job.is_some();
        let label_w = 110.0;
        let row = |ui: &mut egui::Ui, label: String, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(label_w, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_size(Vec2::new(label_w, 28.0));
                    ui.label(label);
                });
                add(ui);
            });
        };

        // Impresora.
        let loading = self.loading.is_some();
        let mut changed = false;
        row(ui, t!("print.printer"), &mut |ui| {
            if loading {
                ui.spinner();
                ui.label(RichText::new(t!("print.searching")).color(p.text_weak));
                return;
            }
            match &self.printers {
                Err(e) => {
                    ui.label(RichText::new(e).color(p.danger));
                }
                Ok(list) if list.is_empty() => {
                    ui.label(RichText::new(t!("print.no_printers")).color(p.text_weak));
                }
                Ok(list) => {
                    ui.add_enabled_ui(!busy, |ui| {
                        let shown = list.get(self.selected).map(label_of).unwrap_or_default();
                        egui::ComboBox::from_id_salt("impresora").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                            for (i, pr) in list.iter().enumerate() {
                                if ui.selectable_value(&mut self.selected, i, label_of(pr)).changed() {
                                    changed = true;
                                }
                            }
                        });
                        if let Some(pr) = list.get(self.selected)
                            && backend::HAS_PROPERTIES
                            && theme::icon_button(ui, icon::SLIDERS_HORIZONTAL, t!("print.properties")).clicked()
                            && let Some(o) = self.driver.properties(&pr.name)
                        {
                            // Lo elegido en el diálogo del controlador.
                            self.copies = o.copies.clamp(1, 999);
                            self.color = o.color && pr.color;
                            self.duplex = if pr.duplex { o.duplex } else { Duplex::Off };
                        }
                    });
                }
            }
        });
        if changed {
            self.on_printer_changed();
        }
        let printer = self.printer().cloned();

        ui.add_enabled_ui(!busy, |ui| {
            // Páginas.
            let n = self.layout.len();
            row(ui, if self.continuous { t!("print.sheets_label") } else { t!("print.pages") }, &mut |ui| {
                let mut opts = vec![(Pages::All, icon::FILES, t!("print.pages_all", n), t!("print.pages_all_tip"))];
                if !self.continuous {
                    opts.push((Pages::Current, icon::FILE, t!("print.pages_current"), t!("print.pages_current_tip", self.current + 1)));
                }
                opts.push((Pages::Range, icon::LIST_NUMBERS, t!("print.pages_range"), t!("print.pages_range_tip")));
                let before = self.pages;
                theme::segmented(ui, &mut self.pages, &opts, true);
                if self.pages != before {
                    self.preview_at = 0;
                }
            });
            if self.pages == Pages::Range {
                let valid = self.chosen().is_some();
                row(ui, String::new(), &mut |ui| {
                    let edit = egui::TextEdit::singleline(&mut self.range).hint_text(t!("print.range_hint")).desired_width(170.0).margin(egui::Margin::symmetric(8, 5));
                    let r = ui.add(edit);
                    if self.first_frame || r.changed() {
                        self.preview_at = 0;
                    }
                    if !valid && !self.range.trim().is_empty() {
                        ui.label(RichText::new(t!("print.range_invalid", n)).color(p.danger).small());
                    }
                });
            }

            // Copias.
            row(ui, t!("print.copies"), &mut |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_enabled_ui(self.copies > 1, |ui| {
                    if theme::icon_button(ui, icon::MINUS, t!("print.copies_less")).clicked() {
                        self.copies -= 1;
                    }
                });
                ui.add_sized([44.0, 28.0], egui::DragValue::new(&mut self.copies).range(1..=999));
                if theme::icon_button(ui, icon::PLUS, t!("print.copies_more")).clicked() && self.copies < 999 {
                    self.copies += 1;
                }
            });

            // Color.
            let can_color = printer.as_ref().is_none_or(|p| p.color);
            row(ui, t!("print.color"), &mut |ui| {
                ui.add_enabled_ui(can_color, |ui| {
                    theme::segmented(
                        ui,
                        &mut self.color,
                        &[(true, icon::PALETTE, t!("print.color_color"), t!("print.color_color_tip")), (false, icon::DROP_HALF, t!("print.color_gray"), t!("print.color_gray_tip"))],
                        true,
                    );
                });
            });

            // Doble cara.
            let can_duplex = printer.as_ref().is_some_and(|p| p.duplex);
            row(ui, t!("print.duplex"), &mut |ui| {
                if !can_duplex {
                    let text = if printer.is_some() { t!("print.duplex_unavailable") } else { "—".to_string() };
                    ui.label(RichText::new(text).color(p.text_weak));
                    return;
                }
                let name = |d: Duplex| match d {
                    Duplex::Off => t!("print.duplex_off"),
                    Duplex::LongEdge => t!("print.duplex_long"),
                    Duplex::ShortEdge => t!("print.duplex_short"),
                };
                egui::ComboBox::from_id_salt("doble_cara").width(300.0).selected_text(name(self.duplex)).show_ui(ui, |ui| {
                    for d in [Duplex::Off, Duplex::LongEdge, Duplex::ShortEdge] {
                        ui.selectable_value(&mut self.duplex, d, name(d));
                    }
                });
            });
        });
    }

    fn footer_ui(&mut self, ui: &mut egui::Ui, doc: &mut Document, p: &theme::Palette) -> Option<Outcome> {
        ui.add_space(16.0);
        if let Some(job) = &self.job {
            // Imprimiendo: progreso y cancelar.
            let total = job.sheets.len() * job.rounds as usize;
            let mut cancel = false;
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(t!("print.progress", (job.done + 1).min(total), total));
                    ui.add(egui::ProgressBar::new(job.done as f32 / total.max(1) as f32).desired_width(400.0).fill(p.accent));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    cancel = theme::dialog_button(ui, t!("dialog.cancel"), false, true).clicked();
                });
            });
            if cancel && let Some(job) = self.job.take() {
                job.spool.abort();
                return Some(Outcome::Closed);
            }
            return None;
        }

        let chosen = self.chosen();
        let ready = chosen.is_some() && self.printer().is_some();
        let mut out = None;
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new(format!("{}  {}", icon::ARROW_SQUARE_OUT, t!("print.open_external"))).color(p.text_weak)).frame(false))
                .on_hover_text(t!("print.open_external_tip"))
                .clicked()
            {
                out = Some(Outcome::OpenExternal);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if theme::dialog_button(ui, t!("dialog.cancel"), false, true).clicked() {
                    out = Some(Outcome::Closed);
                }
                let label = format!("{}  {}", icon::PRINTER, t!("print.print"));
                if theme::dialog_button(ui, label, true, ready).clicked()
                    && let Some(sel) = &chosen
                {
                    out = self.start(doc, sel);
                }
                if let Some(sel) = &chosen {
                    let sheets = sel.len() * self.copies as usize;
                    ui.label(RichText::new(t!("print.summary", sheets)).color(p.text_weak));
                }
            });
        });
        out
    }

    /// Empieza a imprimir las hojas `sel`.
    fn start(&mut self, doc: &mut Document, sel: &[usize]) -> Option<Outcome> {
        let printer = self.printer()?.clone();
        // Escribe las firmas puestas en el PDF (como al guardar).
        let sheets = match doc.print_sheets() {
            Ok(all) => sel.iter().filter_map(|&i| all.get(i).copied()).collect::<Vec<_>>(),
            Err(e) => return Some(Outcome::Failed(format!("{e:#}"))),
        };
        let opts = Options { printer: printer.name.clone(), copies: self.copies, color: self.color, duplex: self.duplex, title: self.title.clone() };
        match backend::Spool::start(&opts, &mut self.driver) {
            Ok(spool) => {
                let rounds = if spool.handles_copies() { 1 } else { self.copies };
                self.job = Some(Job { spool, sheets, rounds, done: 0, gray: !self.color, copies: self.copies, printer: printer.label.clone() });
                None
            }
            // Cancelado en un diálogo del sistema (p. ej. el "Guardar como"
            // de una impresora PDF): no es un error.
            Err(e) if e == t!("print.cancelled") => Some(Outcome::Closed),
            Err(e) => Some(Outcome::Failed(e)),
        }
    }

    /// Manda la hoja siguiente; al acabar, cierra el trabajo.
    fn step(&mut self, doc: &Document) -> Option<Outcome> {
        let job = self.job.as_mut()?;
        let total = job.sheets.len() * job.rounds as usize;
        if job.done < total {
            let sheet = job.sheets[job.done % job.sheets.len()];
            if let Err(e) = job.spool.add(doc, &sheet, job.gray) {
                let job = self.job.take()?;
                job.spool.abort();
                return Some(Outcome::Failed(e));
            }
            job.done += 1;
            return None;
        }
        let job = self.job.take()?;
        let n = job.sheets.len() * job.copies as usize;
        Some(match job.spool.finish() {
            Ok(()) => Outcome::Sent(t!("print.sent", n, job.printer)),
            Err(e) => Outcome::Failed(e),
        })
    }
}

fn label_of(p: &Printer) -> String {
    if p.is_default { format!("{}  ({})", p.label, t!("print.default")) } else { p.label.clone() }
}
