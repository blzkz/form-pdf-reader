//! Aplicación de escritorio (egui/eframe).

use crate::viewer::{DocView, PageLayout, Tool};
use eframe::egui::{self, Align, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText};
use form_pdf_reader::pdfium::{AlertAnswer, AlertButtons, AlertIcon, AlertRequest, Document, FormKind, OpenOptions};
use form_pdf_reader::t;
use std::path::{Path, PathBuf};

/// Acción pendiente de confirmar cuando hay cambios sin guardar.
#[derive(Clone)]
enum Pending {
    Quit,
    /// Volver a abrir el documento de la pestaña activa (p. ej. al cambiar
    /// la vista continua).
    Reopen(PathBuf),
    Close,
}

/// Un documento abierto en una pestaña.
struct Tab {
    view: DocView,
    dirty: bool,
    search: String,
}

pub struct PdfApp {
    tabs: Vec<Tab>,
    active: usize,
    status: String,
    error: Option<String>,
    confirm: Option<Pending>,
    allow_quit: bool,
    search_focus: bool,
    show_about: bool,
    show_settings: bool,
    settings: crate::settings::Settings,
    show_xfa_info: bool,
    show_attachments: bool,
    continuous_xfa: bool,
    title: String,
    autotest: Option<crate::autotest::AutoTest>,
}

impl PdfApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> PdfApp {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        let mut app = PdfApp {
            tabs: Vec::new(),
            active: 0,
            status: t!("status.welcome"),
            error: None,
            confirm: None,
            allow_quit: false,
            search_focus: false,
            show_about: false,
            show_settings: false,
            settings: crate::settings::Settings::load(),
            show_xfa_info: false,
            show_attachments: false,
            continuous_xfa: true,
            title: String::new(),
            autotest: crate::autotest::AutoTest::from_env(),
        };
        if let Some(p) = initial {
            app.open(&p);
        }
        app
    }

    // -----------------------------------------------------------------------
    // Ficheros
    // -----------------------------------------------------------------------

    fn cur(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn cur_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    fn view(&self) -> Option<&DocView> {
        self.cur().map(|t| &t.view)
    }

    fn view_mut(&mut self) -> Option<&mut DocView> {
        self.cur_mut().map(|t| &mut t.view)
    }

    /// Abre `path` en una pestaña nueva (o activa la que ya lo tenga).
    fn open(&mut self, path: &Path) {
        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.tabs.iter().position(|t| std::fs::canonicalize(t.view.doc.path()).ok().as_deref() == Some(canon.as_path())) {
            self.active = i;
            self.status = t!("status.already_open", file_name(path));
            return;
        }
        match Document::open_with(path, OpenOptions { continuous_xfa: self.continuous_xfa }) {
            Ok(doc) => {
                install_handlers(&doc);
                let kind = describe(&doc);
                let is_xfa_dyn = doc.is_dynamic_xfa();
                self.tabs.push(Tab { view: DocView::new(doc), dirty: false, search: String::new() });
                self.active = self.tabs.len() - 1;
                self.error = None;
                self.status = t!("status.opened", file_name(path), kind);
                self.show_xfa_info = is_xfa_dyn;
            }
            Err(e) => {
                self.error = Some(t!("error.open", path.display(), format!("{e:#}")));
            }
        }
    }

    /// Vuelve a abrir el documento de la pestaña activa, en el mismo sitio.
    fn reopen(&mut self, path: &Path) {
        if self.tabs.is_empty() {
            return self.open(path);
        }
        let i = self.active;
        // Liberar antes el documento actual (entorno de PDFium).
        self.tabs.remove(i);
        let n = self.tabs.len();
        self.open(path);
        if self.tabs.len() > n {
            let t = self.tabs.pop().unwrap();
            self.tabs.insert(i, t);
            self.active = i;
        }
    }

    /// Abre un fichero adjunto: los PDF en una pestaña nueva, el resto con la
    /// aplicación del sistema.
    fn open_attachment(&mut self, path: &Path) {
        let is_pdf = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
        if is_pdf {
            self.open(path);
            return;
        }
        match std::process::Command::new("xdg-open").arg(path).spawn() {
            Ok(_) => self.status = t!("status.opening_attachment", file_name(path)),
            Err(e) => self.error = Some(t!("error.open_short", path.display(), e)),
        }
    }

    fn open_dialog(&mut self) {
        let mut d = rfd::FileDialog::new().add_filter(t!("dialog.pdf_filter"), &["pdf", "PDF"]).set_title(t!("dialog.open_title"));
        if let Some(dir) = self.view().and_then(|v| v.doc.path().parent().map(Path::to_path_buf)) {
            d = d.set_directory(dir);
        }
        if let Some(p) = d.pick_file() {
            self.open(&p);
        }
    }

    fn save(&mut self, path: Option<PathBuf>) -> bool {
        let Some(tab) = self.cur_mut() else { return false };
        let target = match path {
            Some(p) => p,
            None => tab.view.doc.path().to_path_buf(),
        };
        tab.view.commit_focus();
        match tab.view.doc.save(&target) {
            Ok(()) => {
                tab.dirty = false;
                self.status = t!("status.saved", target.display());
                true
            }
            Err(e) => {
                self.error = Some(t!("error.save", format!("{e:#}")));
                false
            }
        }
    }

    fn save_as_dialog(&mut self) -> bool {
        let Some(view) = self.view() else { return false };
        let p = view.doc.path();
        let mut d = rfd::FileDialog::new().add_filter(t!("dialog.pdf_filter"), &["pdf"]).set_title(t!("dialog.save_as_title"));
        if let Some(dir) = p.parent() {
            d = d.set_directory(dir);
        }
        d = d.set_file_name(file_name(p));
        match d.save_file() {
            Some(mut t) => {
                if t.extension().is_none() {
                    t.set_extension("pdf");
                }
                self.save(Some(t))
            }
            None => false,
        }
    }

    fn export_flat_dialog(&mut self) {
        let Some(view) = self.view_mut() else { return };
        let p = view.doc.path().to_path_buf();
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| t!("file.default_stem"));
        let mut d = rfd::FileDialog::new().add_filter(t!("dialog.pdf_filter"), &["pdf"]).set_title(t!("dialog.export_title"));
        if let Some(dir) = p.parent() {
            d = d.set_directory(dir);
        }
        d = d.set_file_name(format!("{stem}_{}.pdf", t!("file.flat_suffix")));
        if let Some(t) = d.save_file() {
            view.commit_focus();
            match view.doc.export_flat(&t, 150.0) {
                Ok(()) => self.status = t!("status.exported", t.display()),
                Err(e) => self.error = Some(t!("error.export", format!("{e:#}"))),
            }
        }
    }

    /// Imprime: genera un PDF plano temporal y lo abre en el visor del sistema,
    /// desde el que se imprime con el diálogo habitual.
    fn print(&mut self) {
        let Some(view) = self.view_mut() else { return };
        view.commit_focus();
        let dir = std::env::temp_dir().join("form-pdf-reader");
        let _ = std::fs::create_dir_all(&dir);
        let stem = view.doc.path().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| t!("file.default_stem"));
        let out = dir.join(format!("{stem}_{}.pdf", t!("file.print_suffix")));
        match view.doc.export_flat(&out, 200.0) {
            Ok(()) => match std::process::Command::new("xdg-open").arg(&out).spawn() {
                Ok(_) => self.status = t!("status.print_opened"),
                Err(e) => self.error = Some(t!("error.print_open", out.display(), e)),
            },
            Err(e) => self.error = Some(t!("error.print", format!("{e:#}"))),
        }
    }

    /// Ejecuta una acción, pidiendo confirmación si hay cambios sin guardar.
    fn request(&mut self, action: Pending) {
        // Al salir, cada pestaña con cambios se confirma por separado.
        if matches!(action, Pending::Quit)
            && let Some(i) = self.tabs.iter().position(|t| t.dirty)
        {
            self.active = i;
        }
        if self.cur().is_some_and(|t| t.dirty) {
            self.confirm = Some(action);
        } else {
            self.perform(action);
        }
    }

    fn perform(&mut self, action: Pending) {
        match action {
            Pending::Reopen(p) => self.reopen(&p),
            Pending::Close => self.close_tab(self.active),
            Pending::Quit => {
                if self.tabs.iter().any(|t| t.dirty) {
                    // Quedan otras pestañas con cambios: preguntar por ellas.
                    self.request(Pending::Quit);
                } else {
                    self.allow_quit = true;
                    self.tabs.clear();
                }
            }
        }
    }

    fn close_tab(&mut self, i: usize) {
        if i < self.tabs.len() {
            self.tabs.remove(i);
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            } else if self.active > i {
                self.active -= 1;
            }
            self.status = t!("status.closed");
        }
    }

    /// Barra de pestañas (solo con algún documento abierto).
    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut select = None;
        let mut close = None;
        ui.horizontal_wrapped(|ui| {
            for (i, t) in self.tabs.iter().enumerate() {
                let name = format!("{}{}", if t.dirty { "● " } else { "" }, file_name(t.view.doc.path()));
                let r = ui.selectable_label(i == self.active, name).on_hover_text(t.view.doc.path().display().to_string());
                if r.clicked() {
                    select = Some(i);
                }
                if r.middle_clicked() {
                    close = Some(i);
                }
                if ui.small_button("×").on_hover_text(t!("tab.close")).clicked() {
                    close = Some(i);
                }
                ui.separator();
            }
        });
        if let Some(i) = select {
            self.active = i;
        }
        if let Some(i) = close {
            self.active = i;
            self.request(Pending::Close);
        }
    }

    // -----------------------------------------------------------------------
    // Interfaz
    // -----------------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let cs = Modifiers::COMMAND | Modifiers::SHIFT;
        if ctx.input_mut(|i| i.consume_shortcut(&sc(cs, Key::S))) {
            self.save_as_dialog();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::S))) {
            self.save(None);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::O))) {
            self.open_dialog();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::P))) {
            self.print();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::W))) {
            self.request(Pending::Close);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Q))) {
            self.request(Pending::Quit);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::F))) {
            self.search_focus = true;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::CTRL, Key::Tab))) && !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
        if let Some(v) = self.view_mut() {
            if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Plus)) || i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Equals))) {
                v.set_zoom(v.zoom * 1.2);
            }
            if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Minus))) {
                v.set_zoom(v.zoom / 1.2);
            }
            if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Num0))) {
                v.set_zoom(1.0);
            }
        }
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(t!("menu.file"), |ui| {
                if ui.add(egui::Button::new(t!("menu.open")).shortcut_text("Ctrl+O")).clicked() {
                    ui.close();
                    self.open_dialog();
                }
                let has = !self.tabs.is_empty();
                if ui.add_enabled(has, egui::Button::new(t!("menu.save")).shortcut_text("Ctrl+S")).clicked() {
                    ui.close();
                    self.save(None);
                }
                if ui.add_enabled(has, egui::Button::new(t!("menu.save_as")).shortcut_text(t!("menu.save_as_shortcut"))).clicked() {
                    ui.close();
                    self.save_as_dialog();
                }
                if ui.add_enabled(has, egui::Button::new(t!("menu.attachments"))).clicked() {
                    ui.close();
                    self.show_attachments = true;
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new(t!("menu.export_flat"))).clicked() {
                    ui.close();
                    self.export_flat_dialog();
                }
                if ui.add_enabled(has, egui::Button::new(t!("menu.print")).shortcut_text("Ctrl+P")).clicked() {
                    ui.close();
                    self.print();
                }
                ui.separator();
                if ui.button(t!("menu.settings")).clicked() {
                    ui.close();
                    self.show_settings = true;
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new(t!("menu.close")).shortcut_text("Ctrl+W")).clicked() {
                    ui.close();
                    self.request(Pending::Close);
                }
                if ui.add(egui::Button::new(t!("menu.quit")).shortcut_text("Ctrl+Q")).clicked() {
                    ui.close();
                    self.request(Pending::Quit);
                }
            });
            ui.menu_button(t!("menu.view"), |ui| {
                let has = !self.tabs.is_empty();
                if let Some(v) = self.view_mut() {
                    if ui.button(t!("menu.zoom_in")).clicked() {
                        v.set_zoom(v.zoom * 1.2);
                    }
                    if ui.button(t!("menu.zoom_out")).clicked() {
                        v.set_zoom(v.zoom / 1.2);
                    }
                    if ui.button(t!("menu.actual_size")).clicked() {
                        v.set_zoom(1.0);
                    }
                    ui.checkbox(&mut v.fit_width, t!("menu.fit"));
                    ui.separator();
                    let continuous_xfa = v.doc.is_continuous();
                    let mut l = v.layout;
                    for opt in [PageLayout::Continuous, PageLayout::Single, PageLayout::TwoColumns] {
                        ui.add_enabled_ui(!continuous_xfa, |ui| {
                            ui.radio_value(&mut l, opt, layout_name(opt))
                                .on_disabled_hover_text(t!("menu.layout_disabled"));
                        });
                    }
                    v.set_layout(l);
                    ui.separator();
                    let mut t = v.tool;
                    ui.radio_value(&mut t, Tool::Form, t!("menu.tool_form"));
                    ui.radio_value(&mut t, Tool::Text, t!("menu.tool_text"));
                    ui.radio_value(&mut t, Tool::Hand, t!("menu.tool_hand"));
                    v.set_tool(t);
                }
                ui.separator();
                let mut c = self.continuous_xfa;
                if ui
                    .add_enabled(true, egui::Checkbox::new(&mut c, t!("menu.xfa_continuous")))
                    .on_hover_text(t!("menu.xfa_continuous_tip"))
                    .changed()
                {
                    self.continuous_xfa = c;
                    if has
                        && let Some(p) = self.view().map(|v| v.doc.path().to_path_buf()) {
                            self.request(Pending::Reopen(p));
                        }
                }
            });
            ui.menu_button(t!("menu.help"), |ui| {
                if ui.button(t!("menu.xfa_info")).clicked() {
                    ui.close();
                    self.show_xfa_info = true;
                }
                if ui.button(t!("menu.about")).clicked() {
                    ui.close();
                    self.show_about = true;
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(format!("📂 {}", t!("toolbar.open"))).clicked() {
                self.open_dialog();
            }
            let has = !self.tabs.is_empty();
            if ui.add_enabled(has, egui::Button::new(format!("💾 {}", t!("toolbar.save")))).clicked() {
                self.save(None);
            }
            if ui.add_enabled(has, egui::Button::new(format!("🖶 {}", t!("toolbar.print")))).clicked() {
                self.print();
            }
            ui.separator();
            let search_focus = &mut self.search_focus;
            let status = &mut self.status;
            if let Some(Tab { view: v, search, .. }) = self.tabs.get_mut(self.active) {
                // Herramienta del ratón.
                let mut t = v.tool;
                ui.selectable_value(&mut t, Tool::Form, format!("📝 {}", t!("toolbar.form"))).on_hover_text(t!("toolbar.form_tip"));
                ui.selectable_value(&mut t, Tool::Text, format!("🔤 {}", t!("toolbar.text"))).on_hover_text(t!("toolbar.text_tip"));
                ui.selectable_value(&mut t, Tool::Hand, format!("✋ {}", t!("toolbar.hand"))).on_hover_text(t!("toolbar.hand_tip"));
                v.set_tool(t);
                ui.separator();
                if ui.button("−").on_hover_text(t!("toolbar.zoom_out")).clicked() {
                    v.set_zoom(v.zoom / 1.2);
                }
                ui.label(format!("{:.0} %", v.zoom * 100.0));
                if ui.button("+").on_hover_text(t!("toolbar.zoom_in")).clicked() {
                    v.set_zoom(v.zoom * 1.2);
                }
                let fit_label = if v.effective_layout() == PageLayout::Single { format!("⛶ {}", t!("toolbar.fit_page")) } else { format!("↔ {}", t!("toolbar.fit_width")) };
                ui.toggle_value(&mut v.fit_width, fit_label);
                ui.separator();
                if !v.doc.is_continuous() {
                    let mut l = v.layout;
                    for opt in [PageLayout::Continuous, PageLayout::Single, PageLayout::TwoColumns] {
                        if layout_button(ui, l == opt, opt).on_hover_text(layout_name(opt)).clicked() {
                            l = opt;
                        }
                    }
                    v.set_layout(l);
                }
                let n = v.page_count();
                if v.doc.is_continuous() {
                    ui.label(t!("toolbar.continuous"));
                } else {
                    if ui.button("◀").clicked() && v.current_page > 0 {
                        let p = v.current_page - 1;
                        v.go_to_page(p);
                    }
                    ui.label(t!("toolbar.page_of", (v.current_page + 1).min(n), n));
                    if ui.button("▶").clicked() && v.current_page + 1 < n {
                        let p = v.current_page + 1;
                        v.go_to_page(p);
                    }
                }
                ui.separator();
                let resp = ui.add(egui::TextEdit::singleline(search).hint_text(t!("toolbar.search_hint")).desired_width(180.0));
                if *search_focus {
                    resp.request_focus();
                    *search_focus = false;
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    let q = search.clone();
                    let n = v.search(&q);
                    *status = if n == 0 {
                        if v.doc.is_dynamic_xfa() {
                            t!("search.none_xfa")
                        } else {
                            t!("search.none")
                        }
                    } else {
                        t!("search.results", n)
                    };
                }
                if !v.search_hits.is_empty() {
                    if ui.small_button("⏶").clicked() {
                        v.search_step(false);
                    }
                    if ui.small_button("⏷").clicked() {
                        v.search_step(true);
                    }
                    ui.label(format!("{}/{}", v.search_index + 1, v.search_hits.len()));
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(t) = self.cur() {
                ui.label(RichText::new(describe(&t.view.doc)).strong());
                ui.separator();
                if t.dirty {
                    ui.label(RichText::new(format!("● {}", t!("status.unsaved"))).color(Color32::from_rgb(200, 120, 0)));
                    ui.separator();
                }
            }
            ui.label(&self.status);
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(err) = self.error.clone() {
            egui::Modal::new(egui::Id::new("error")).show(ctx, |ui| {
                ui.set_max_width(480.0);
                ui.heading(t!("dialog.error"));
                ui.label(err);
                ui.add_space(8.0);
                if ui.button(t!("dialog.ok")).clicked() {
                    self.error = None;
                }
            });
        }
        if let Some(action) = self.confirm.clone() {
            let mut close = false;
            egui::Modal::new(egui::Id::new("confirmar")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(t!("status.unsaved"));
                let name = self.view().map(|v| file_name(v.doc.path())).unwrap_or_default();
                ui.label(t!("dialog.unsaved_question", name));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(t!("dialog.save")).clicked()
                        && self.save(None) {
                            close = true;
                            self.perform(action.clone());
                        }
                    if ui.button(t!("dialog.discard")).clicked() {
                        close = true;
                        if let Some(t) = self.cur_mut() {
                            t.dirty = false;
                        }
                        self.perform(action.clone());
                    }
                    if ui.button(t!("dialog.cancel")).clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.confirm = None;
            }
        }
        if self.show_settings {
            self.settings_window(ctx);
        }
        if self.show_about {
            egui::Modal::new(egui::Id::new("acerca")).show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.heading(format!("Form PDF Reader {}", env!("CARGO_PKG_VERSION")));
                ui.label(t!("about.desc"));
                ui.label(t!("about.engine"));
                ui.add_space(8.0);
                if ui.button(t!("dialog.close")).clicked() {
                    self.show_about = false;
                }
            });
        }
        if self.show_attachments {
            let mut close = false;
            let mut open: Option<form_pdf_reader::xfa::attachments::Attachment> = None;
            let mut save: Option<form_pdf_reader::xfa::attachments::Attachment> = None;
            let (items, bridge, dirty) = match self.view() {
                Some(v) => (v.doc.attachments(), v.doc.has_attachment_bridge(), v.doc.attachments_dirty()),
                None => (Vec::new(), false, false),
            };
            egui::Modal::new(egui::Id::new("adjuntos")).show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.heading(t!("attach.title"));
                if items.is_empty() {
                    ui.label(t!("attach.none"));
                }
                egui::Grid::new("tabla_adjuntos").striped(true).show(ui, |ui| {
                    for a in &items {
                        ui.label(&a.file_name);
                        ui.label(format!("{:.1} KB", a.data.len() as f64 / 1024.0));
                        if ui.button(t!("attach.open")).clicked() {
                            open = Some(a.clone());
                        }
                        if ui.button(t!("attach.save_as")).clicked() {
                            save = Some(a.clone());
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                if bridge {
                    ui.label(t!("attach.use_form"));
                }
                if dirty {
                    ui.label(RichText::new(t!("attach.unsaved")).color(Color32::from_rgb(200, 120, 0)));
                }
                ui.add_space(6.0);
                if ui.button(t!("dialog.close")).clicked() {
                    close = true;
                }
            });
            if let Some(a) = open {
                let dir = std::env::temp_dir().join("form-pdf-reader").join("adjuntos");
                let _ = std::fs::create_dir_all(&dir);
                let path = dir.join(a.file_name.replace('/', "_"));
                match std::fs::write(&path, &a.data) {
                    Ok(()) => {
                        close = true;
                        self.open_attachment(&path);
                    }
                    Err(e) => self.error = Some(t!("error.attachment_open", e)),
                }
            }
            if let Some(a) = save
                && let Some(path) = rfd::FileDialog::new().set_title(t!("attach.save_title")).set_file_name(&a.file_name).save_file() {
                    match std::fs::write(&path, &a.data) {
                        Ok(()) => self.status = t!("status.attachment_saved", path.display()),
                        Err(e) => self.error = Some(t!("error.attachment_save", e)),
                    }
                }
            if close {
                self.show_attachments = false;
            }
        }
        if self.show_xfa_info {
            egui::Modal::new(egui::Id::new("xfa")).show(ctx, |ui| {
                ui.set_max_width(560.0);
                ui.heading(t!("xfa.title"));
                ui.label(t!("xfa.intro"));
                ui.add_space(4.0);
                ui.label(format!("• {}", t!("xfa.continuous")));
                ui.label(format!("• {}", t!("xfa.saving")));
                ui.label(format!("• {}", t!("xfa.works")));
                ui.label(format!("• {}", t!("xfa.attachments")));
                ui.label(format!("• {}", t!("xfa.not_supported")));
                ui.label(format!("• {}", t!("xfa.print")));
                ui.add_space(8.0);
                if ui.button(t!("xfa.ok")).clicked() {
                    self.show_xfa_info = false;
                }
            });
        }
    }

    /// Ajustes: el idioma de la interfaz, que cambia en el acto y se recuerda.
    fn settings_window(&mut self, ctx: &egui::Context) {
        use form_pdf_reader::i18n;
        egui::Modal::new(egui::Id::new("ajustes")).show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.heading(t!("settings.title"));
            ui.add_space(8.0);
            let system = i18n::LANGUAGES.iter().find(|(c, _)| *c == i18n::system_language()).map_or("English", |(_, n)| *n);
            let system_label = t!("settings.language_system", system);
            let mut chosen = self.settings.language.clone();
            let shown = chosen
                .as_deref()
                .and_then(|c| i18n::LANGUAGES.iter().find(|(l, _)| *l == c))
                .map_or(system_label.clone(), |(_, n)| n.to_string());
            ui.horizontal(|ui| {
                ui.label(t!("settings.language"));
                egui::ComboBox::from_id_salt("idioma").selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, system_label);
                    for (code, name) in i18n::LANGUAGES {
                        ui.selectable_value(&mut chosen, Some(code.to_string()), *name);
                    }
                });
            });
            if chosen != self.settings.language {
                self.settings.language = chosen;
                self.settings.apply_language();
                self.settings.save();
                // El mensaje de bienvenida se calculó en el idioma anterior.
                if self.tabs.is_empty() {
                    self.status = t!("status.welcome");
                }
            }
            ui.add_space(12.0);
            if ui.button(t!("dialog.close")).clicked() {
                self.show_settings = false;
            }
        });
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let t = match self.cur() {
            Some(t) => format!("{}{} — Form PDF Reader", if t.dirty { "● " } else { "" }, file_name(t.view.doc.path())),
            None => "Form PDF Reader".into(),
        };
        if t != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t.clone()));
            self.title = t;
        }
    }
}

impl eframe::App for PdfApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(t) = self.autotest.as_mut() {
            t.raw_input(ctx, raw);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(t) = self.autotest.as_mut() {
            t.after_frame(&ctx);
        }

        // Cierre de ventana con cambios sin guardar.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_quit {
            if self.tabs.iter().any(|t| t.dirty) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if self.confirm.is_none() {
                    self.request(Pending::Quit);
                }
            } else {
                self.tabs.clear();
            }
        }
        if self.allow_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Ficheros arrastrados a la ventana.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for p in dropped {
            self.open(&p);
        }

        if self.confirm.is_none() && self.error.is_none() {
            self.shortcuts(&ctx);
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu(ui));
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        if !self.tabs.is_empty() {
            egui::Panel::top("pestanas").show(ui, |ui| self.tab_bar(ui));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        let mut ev = None;
        egui::CentralPanel::default_margins().frame(egui::Frame::NONE).show(ui, |ui| match self.view_mut() {
            Some(v) => ev = Some(v.ui(ui)),
            None => {
                ui.with_layout(Layout::centered_and_justified(egui::Direction::TopDown), |ui| {
                    ui.label(RichText::new(t!("empty.hint")).size(18.0).color(Color32::GRAY));
                });
            }
        });

        if let Some(ev) = ev {
            if ev.modified
                && let Some(t) = self.cur_mut()
            {
                t.dirty = true;
            }
            if ev.restructured
                && let Some(v) = self.view()
                    && v.doc.lost_focus_on_refresh() {
                        self.status = t!("status.restructured");
                    }
            if ev.print_requested {
                self.print();
            }
            if let Some(url) = ev.submit_requested {
                self.status = t!("status.submit", url);
            }
            for f in ev.open_files {
                if crate::autotest::active() {
                    eprintln!("[autotest] abrir adjunto {}", f.display());
                }
                let is_pdf = f.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
                if crate::autotest::active() && !is_pdf {
                    continue;
                }
                self.open_attachment(&f);
            }
            for u in ev.uris {
                if u.starts_with("http://") || u.starts_with("https://") || u.starts_with("mailto:") {
                    let _ = std::process::Command::new("xdg-open").arg(&u).spawn();
                    self.status = t!("status.opening_link", u);
                }
            }
        }
        if let Some(v) = self.view_mut() {
            v.trim_cache();
        }

        self.dialogs(&ctx);
        self.update_title(&ctx);
    }
}

/// Botón con icono (dibujado, sin texto) para elegir la disposición.
fn layout_button(ui: &mut egui::Ui, selected: bool, l: PageLayout) -> egui::Response {
    let size = egui::vec2(26.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let w = if selected { &v.widgets.active } else { v.widgets.style(&resp) };
        let fill = if selected { v.selection.bg_fill } else if resp.hovered() { w.weak_bg_fill } else { Color32::TRANSPARENT };
        let p = ui.painter();
        p.rect_filled(rect, 3.0, fill);
        let ink = if selected { v.selection.stroke.color } else { w.fg_stroke.color };
        let stroke = egui::Stroke::new(1.2, ink);
        let c = rect.center();
        let page = |r: egui::Rect| {
            p.rect_filled(r, 1.0, Color32::WHITE);
            p.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Inside);
        };
        match l {
            PageLayout::Continuous => {
                // Dos páginas una debajo de otra, cortadas por los bordes.
                page(egui::Rect::from_min_max(egui::pos2(c.x - 5.0, rect.top() + 2.0), egui::pos2(c.x + 5.0, c.y - 1.0)));
                page(egui::Rect::from_min_max(egui::pos2(c.x - 5.0, c.y + 1.0), egui::pos2(c.x + 5.0, rect.bottom() - 2.0)));
            }
            PageLayout::Single => {
                page(egui::Rect::from_center_size(c, egui::vec2(11.0, 14.0)));
            }
            PageLayout::TwoColumns => {
                page(egui::Rect::from_center_size(c - egui::vec2(6.0, 0.0), egui::vec2(10.0, 14.0)));
                page(egui::Rect::from_center_size(c + egui::vec2(6.0, 0.0), egui::vec2(10.0, 14.0)));
            }
        }
    }
    resp
}

fn layout_name(l: PageLayout) -> String {
    match l {
        PageLayout::Continuous => t!("layout.continuous"),
        PageLayout::Single => t!("layout.single"),
        PageLayout::TwoColumns => t!("layout.two"),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn describe(doc: &Document) -> String {
    match (doc.kind(), doc.is_xfa()) {
        (FormKind::XfaFull, true) if doc.is_continuous() => t!("kind.xfa_dynamic_continuous"),
        (FormKind::XfaFull, true) => t!("kind.xfa_dynamic"),
        (FormKind::XfaForeground, true) => t!("kind.xfa_static"),
        (FormKind::XfaFull | FormKind::XfaForeground, false) => t!("kind.xfa_failed"),
        (FormKind::AcroForm, _) => t!("kind.acroform"),
        (FormKind::None, _) => "PDF".into(),
    }
}

/// Diálogos nativos para app.alert() y la selección de ficheros de los formularios.
fn install_handlers(doc: &Document) {
    if crate::autotest::active() {
        // En pruebas automáticas las alertas no deben bloquear.
        doc.set_alert_handler(Box::new(|req: &AlertRequest| {
            eprintln!("[autotest] alerta: {}", req.message);
            AlertAnswer::Ok
        }));
        // Fichero a "elegir" en los diálogos de selección.
        doc.set_browse_handler(Box::new(|| std::env::var("PDFRE_AUTOTEST_FILE").ok()));
        return;
    }
    doc.set_alert_handler(Box::new(|req: &AlertRequest| {
        let level = match req.icon {
            AlertIcon::Error => rfd::MessageLevel::Error,
            AlertIcon::Warning => rfd::MessageLevel::Warning,
            _ => rfd::MessageLevel::Info,
        };
        let buttons = match req.buttons {
            AlertButtons::Ok => rfd::MessageButtons::Ok,
            AlertButtons::OkCancel => rfd::MessageButtons::OkCancel,
            AlertButtons::YesNo => rfd::MessageButtons::YesNo,
            AlertButtons::YesNoCancel => rfd::MessageButtons::YesNoCancel,
        };
        let title = if req.title.is_empty() || req.title == "Alert" { t!("dialog.form_title") } else { req.title.clone() };
        let r = rfd::MessageDialog::new().set_title(title).set_description(req.message.clone()).set_level(level).set_buttons(buttons).show();
        match r {
            rfd::MessageDialogResult::Ok => AlertAnswer::Ok,
            rfd::MessageDialogResult::Cancel => AlertAnswer::Cancel,
            rfd::MessageDialogResult::Yes => AlertAnswer::Yes,
            rfd::MessageDialogResult::No => AlertAnswer::No,
            rfd::MessageDialogResult::Custom(_) => AlertAnswer::Ok,
        }
    }));
    doc.set_browse_handler(Box::new(|| rfd::FileDialog::new().set_title(t!("dialog.pick_file")).pick_file().map(|p| p.to_string_lossy().into_owned())));
    // app.response(): pregunta de texto. Se usa zenity (los diálogos de rfd no
    // tienen campo de texto).
    doc.set_response_handler(Box::new(|question: &str, title: &str, default: &str| {
        let out = std::process::Command::new("zenity")
            .arg("--entry")
            .arg(format!("--title={}", if title.is_empty() { t!("dialog.form_title") } else { title.to_string() }))
            .arg(format!("--text={question}"))
            .arg(format!("--entry-text={default}"))
            .output()
            .ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())
    }));
}

#[allow(dead_code)]
fn _align() -> Align {
    Align::Center
}
