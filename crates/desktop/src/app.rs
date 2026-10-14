//! Aplicación de escritorio (egui/eframe).

use crate::chrome::{self, TabAction, TabInfo};
use crate::theme::{self, icon};
use crate::viewer::{DocView, PageLayout, Tool};
use eframe::egui::{self, Align, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText};
use form_pdf_reader::pdfium::{AlertAnswer, AlertButtons, AlertIcon, AlertRequest, Document, FormKind, OpenOptions, PasswordRequired};
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
/// Un PDF que pide contraseña para abrirlo.
struct PasswordPrompt {
    path: PathBuf,
    text: String,
    /// La contraseña anterior no era correcta.
    wrong: bool,
    focus: bool,
}

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
    password_prompt: Option<PasswordPrompt>,
    settings: crate::settings::Settings,
    show_xfa_info: bool,
    show_attachments: bool,
    continuous_xfa: bool,
    title: String,
    autotest: Option<crate::autotest::AutoTest>,
    signatures: crate::signatures::Signatures,
}

impl PdfApp {
    pub fn new(ctx: &egui::Context, initial: Option<PathBuf>) -> PdfApp {
        theme::install(ctx);
        let settings = crate::settings::Settings::load();
        settings.apply_theme(ctx);
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
            password_prompt: None,
            settings,
            show_xfa_info: false,
            show_attachments: false,
            continuous_xfa: true,
            title: String::new(),
            autotest: crate::autotest::AutoTest::from_env(),
            signatures: crate::signatures::Signatures::new(),
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
        self.open_with_password(path, None);
    }

    /// Como `open`, con la contraseña del PDF si la pide. Si falta (o no es
    /// correcta), se pide.
    fn open_with_password(&mut self, path: &Path, password: Option<String>) {
        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.tabs.iter().position(|t| std::fs::canonicalize(t.view.doc.path()).ok().as_deref() == Some(canon.as_path())) {
            self.active = i;
            self.status = t!("status.already_open", file_name(path));
            return;
        }
        match Document::open_with(path, OpenOptions { continuous_xfa: self.continuous_xfa, password }) {
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
                if let Some(p) = e.downcast_ref::<PasswordRequired>() {
                    self.password_prompt = Some(PasswordPrompt { path: path.to_path_buf(), text: String::new(), wrong: p.wrong, focus: true });
                    return;
                }
                self.error = Some(t!("error.open", path.display(), format!("{e:#}")));
            }
        }
    }

    fn password_window(&mut self, ctx: &egui::Context) {
        let Some(p) = self.password_prompt.as_mut() else { return };
        let mut submit = false;
        let mut cancel = false;
        let r = egui::Modal::new(egui::Id::new("contraseña")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(t!("dialog.password_title"));
            ui.add_space(4.0);
            ui.label(t!("dialog.password_prompt", file_name(&p.path)));
            if p.wrong {
                ui.colored_label(ui.visuals().error_fg_color, t!("dialog.password_wrong"));
            }
            ui.add_space(4.0);
            let field = ui.add(egui::TextEdit::singleline(&mut p.text).password(true).desired_width(f32::INFINITY));
            if std::mem::take(&mut p.focus) {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                submit |= theme::primary_button(ui, t!("toolbar.open")).clicked();
                cancel |= ui.button(t!("dialog.cancel")).clicked();
            });
        });
        if submit {
            let p = self.password_prompt.take().unwrap();
            self.open_with_password(&p.path, Some(p.text));
        } else if cancel || r.should_close() {
            self.password_prompt = None;
        }
    }

    /// Vuelve a abrir el documento de la pestaña activa, en el mismo sitio.
    fn reopen(&mut self, path: &Path) {
        if self.tabs.is_empty() {
            return self.open(path);
        }
        let i = self.active;
        let password = self.tabs[i].view.doc.password().map(str::to_string);
        // Liberar antes el documento actual (entorno de PDFium).
        self.tabs.remove(i);
        let n = self.tabs.len();
        self.open_with_password(path, password);
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
        match crate::platform::open_external(&path.to_string_lossy()) {
            Ok(()) => self.status = t!("status.opening_attachment", file_name(path)),
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
            Ok(()) => match crate::platform::open_external(&out.to_string_lossy()) {
                Ok(()) => self.status = t!("status.print_opened"),
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
    /// Pestañas de la barra de título (chrome.rs).
    fn title_bar(&mut self, ui: &mut egui::Ui) {
        let infos: Vec<TabInfo> = self
            .tabs
            .iter()
            .map(|t| TabInfo { name: file_name(t.view.doc.path()), tooltip: t.view.doc.path().display().to_string(), dirty: t.dirty })
            .collect();
        match chrome::title_bar(ui, &infos, self.active) {
            Some(TabAction::Select(i)) => self.active = i,
            Some(TabAction::Close(i)) => {
                self.active = i;
                self.request(Pending::Close);
            }
            Some(TabAction::Open) => self.open_dialog(),
            None => {}
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

    /// Menú principal (botón ☰): lo que antes eran Archivo, Ver y Ayuda.
    fn main_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(240.0);
        {
            {
                if ui.add(egui::Button::new(mi(icon::FOLDER_OPEN, t!("menu.open"))).shortcut_text("Ctrl+O")).clicked() {
                    ui.close();
                    self.open_dialog();
                }
                let has = !self.tabs.is_empty();
                if ui.add_enabled(has, egui::Button::new(mi(icon::FLOPPY_DISK, t!("menu.save"))).shortcut_text("Ctrl+S")).clicked() {
                    ui.close();
                    self.save(None);
                }
                if ui.add_enabled(has, egui::Button::new(mi(icon::FLOPPY_DISK_BACK, t!("menu.save_as"))).shortcut_text(t!("menu.save_as_shortcut"))).clicked() {
                    ui.close();
                    self.save_as_dialog();
                }
                if ui.add_enabled(has, egui::Button::new(mi(icon::PAPERCLIP, t!("menu.attachments")))).clicked() {
                    ui.close();
                    self.show_attachments = true;
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new(mi(icon::EXPORT, t!("menu.export_flat")))).clicked() {
                    ui.close();
                    self.export_flat_dialog();
                }
                if ui.add_enabled(has, egui::Button::new(mi(icon::PRINTER, t!("menu.print"))).shortcut_text("Ctrl+P")).clicked() {
                    ui.close();
                    self.print();
                }
                ui.separator();
                if ui.add(egui::Button::new(mi(icon::GEAR, t!("menu.settings")))).clicked() {
                    ui.close();
                    self.show_settings = true;
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new(mi(icon::X, t!("menu.close"))).shortcut_text("Ctrl+W")).clicked() {
                    ui.close();
                    self.request(Pending::Close);
                }
            }
            ui.separator();
            ui.menu_button(mi(icon::EYE, t!("menu.view")), |ui| {
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
            ui.menu_button(mi(icon::QUESTION, t!("menu.help")), |ui| {
                if ui.add(egui::Button::new(mi(icon::FILE_TEXT, t!("menu.xfa_info")))).clicked() {
                    ui.close();
                    self.show_xfa_info = true;
                }
                if ui.add(egui::Button::new(mi(icon::INFO, t!("menu.about")))).clicked() {
                    ui.close();
                    self.show_about = true;
                }
            });
            ui.separator();
            if ui.add(egui::Button::new(mi(icon::SIGN_OUT, t!("menu.quit"))).shortcut_text("Ctrl+Q")).clicked() {
                ui.close();
                self.request(Pending::Quit);
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let menu = egui::Button::new(RichText::new(icon::LIST).size(18.0)).frame_when_inactive(false).min_size(egui::Vec2::splat(32.0));
            egui::containers::menu::MenuButton::from_button(menu).ui(ui, |ui| self.main_menu(ui)).0.on_hover_text(t!("menu.main"));
            if theme::icon_button(ui, icon::FOLDER_OPEN, format!("{} (Ctrl+O)", t!("toolbar.open"))).clicked() {
                self.open_dialog();
            }
            let has = !self.tabs.is_empty();
            ui.add_enabled_ui(has, |ui| {
                if theme::icon_button(ui, icon::FLOPPY_DISK, format!("{} (Ctrl+S)", t!("toolbar.save"))).clicked() {
                    self.save(None);
                }
                if theme::icon_button(ui, icon::PRINTER, format!("{} (Ctrl+P)", t!("toolbar.print"))).clicked() {
                    self.print();
                }
            });
            let search_focus = &mut self.search_focus;
            let status = &mut self.status;
            let Some(Tab { view: v, search, .. }) = self.tabs.get_mut(self.active) else { return };
            separator(ui);
            let n = v.page_count();
            let cur = (v.current_page + 1).min(n);
            let text_w = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), egui::FontId::proportional(14.0), Color32::WHITE).size().x);
            let tools = [(icon::CURSOR_CLICK, t!("toolbar.form")), (icon::CURSOR_TEXT, t!("toolbar.text")), (icon::HAND, t!("toolbar.hand"))];
            let widths = ToolbarWidths {
                tools_labels: 4.0 + tools.iter().map(|(g, l)| 22.0 + text_w(&format!("{g}  {l}"))).sum::<f32>(),
                tools_icons: 4.0 + tools.iter().map(|(g, _)| 22.0 + text_w(g)).sum::<f32>(),
                page_long: 4.0 + text_w(&t!("toolbar.page_of", cur, n)),
                page_short: 4.0 + text_w(&format!("{cur} / {n}")),
                search_results: if v.search_hits.is_empty() { 0.0 } else { 76.0 + text_w(&format!("{}/{}", v.search_index + 1, v.search_hits.len())) },
            };
            let fit = ToolbarFit::new(ui.available_width(), !v.doc.is_continuous(), &widths);

            // Herramienta del ratón.
            let mut t = v.tool;
            theme::segmented(
                ui,
                &mut t,
                &[
                    (Tool::Form, icon::CURSOR_CLICK, t!("toolbar.form"), t!("toolbar.form_tip")),
                    (Tool::Text, icon::CURSOR_TEXT, t!("toolbar.text"), t!("toolbar.text_tip")),
                    (Tool::Hand, icon::HAND, t!("toolbar.hand"), t!("toolbar.hand_tip")),
                ],
                fit.tool_labels,
            );
            v.set_tool(t);

            // Firmar: las firmas guardadas en este equipo, o crear una.
            let can_sign = v.doc.can_sign();
            let sign_button = egui::Button::new(RichText::new(icon::SIGNATURE).size(18.0)).frame_when_inactive(v.is_placing()).min_size(egui::Vec2::splat(32.0));
            let mut action = None;
            ui.add_enabled_ui(can_sign, |ui| {
                let (r, inner) = egui::containers::menu::MenuButton::from_button(sign_button).ui(ui, |ui| self.signatures.menu(ui));
                r.on_hover_text(t!("sign.button_tip")).on_disabled_hover_text(t!("sign.cannot"));
                action = inner.and_then(|i| i.inner);
            });
            match action {
                Some(crate::signatures::Action::Place(sig)) => {
                    v.start_placing(ui.ctx(), sig);
                    *status = t!("sign.placing_status");
                }
                Some(crate::signatures::Action::Create) => {
                    self.signatures.open_create(self.settings.signer_name.as_deref().unwrap_or(""));
                }
                None => {}
            }

            // Disposición y páginas (las XFA continuas son una sola página).
            if !v.doc.is_continuous() && fit.page_nav {
                separator(ui);
                if fit.layouts {
                    let mut l = v.layout;
                    for (opt, glyph) in [(PageLayout::Continuous, icon::ROWS), (PageLayout::Single, icon::FILE), (PageLayout::TwoColumns, icon::BOOK_OPEN)] {
                        if theme::icon_toggle(ui, l == opt, glyph, layout_name(opt)).clicked() {
                            l = opt;
                        }
                    }
                    v.set_layout(l);
                }
                if theme::icon_button(ui, icon::CARET_LEFT, t!("toolbar.prev_page")).clicked() && v.current_page > 0 {
                    let p = v.current_page - 1;
                    v.go_to_page(p);
                }
                let page = if fit.page_long { t!("toolbar.page_of", cur, n) } else { format!("{cur} / {n}") };
                ui.label(RichText::new(page).color(p.text_weak));
                if theme::icon_button(ui, icon::CARET_RIGHT, t!("toolbar.next_page")).clicked() && v.current_page + 1 < n {
                    let p = v.current_page + 1;
                    v.go_to_page(p);
                }
            }

            // A la derecha: búsqueda y zoom.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if !v.search_hits.is_empty() {
                    ui.label(RichText::new(format!("{}/{}", v.search_index + 1, v.search_hits.len())).color(p.text_weak));
                    if theme::icon_button(ui, icon::CARET_DOWN, t!("toolbar.next_result")).clicked() {
                        v.search_step(true);
                    }
                    if theme::icon_button(ui, icon::CARET_UP, t!("toolbar.prev_result")).clicked() {
                        v.search_step(false);
                    }
                }
                let field = egui::Frame::new()
                    .fill(ui.visuals().text_edit_bg_color())
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(8, 3));
                let resp = field
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(p.text_weak));
                            ui.add(egui::TextEdit::singleline(search).hint_text(t!("toolbar.search_hint")).frame(egui::Frame::NONE).desired_width(fit.search_width))
                        })
                        .inner
                    })
                    .inner;
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
                separator(ui);
                let (fit_icon, fit_tip) = if v.effective_layout() == PageLayout::Single {
                    (icon::CORNERS_OUT, t!("toolbar.fit_page"))
                } else {
                    (icon::ARROWS_OUT_LINE_HORIZONTAL, t!("toolbar.fit_width"))
                };
                if theme::icon_toggle(ui, v.fit_width, fit_icon, fit_tip).clicked() {
                    v.fit_width = !v.fit_width;
                }
                if fit.zoom {
                    if theme::icon_button(ui, icon::MAGNIFYING_GLASS_PLUS, format!("{} (Ctrl++)", t!("toolbar.zoom_in"))).clicked() {
                        v.set_zoom(v.zoom * 1.2);
                    }
                    ui.add_sized([44.0, 20.0], egui::Label::new(RichText::new(format!("{:.0} %", v.zoom * 100.0)).color(p.text_weak)));
                    if theme::icon_button(ui, icon::MAGNIFYING_GLASS_MINUS, format!("{} (Ctrl+−)", t!("toolbar.zoom_out"))).clicked() {
                        v.set_zoom(v.zoom / 1.2);
                    }
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.horizontal_centered(|ui| {
            let small = |s: String| RichText::new(s).size(12.0).color(p.text_weak);
            if let Some(t) = self.cur() {
                ui.label(small(describe(&t.view.doc)));
                if t.dirty {
                    ui.label(RichText::new(format!("●  {}", t!("status.unsaved"))).size(12.0).color(p.warn));
                }
                ui.label(small("·".into()));
            }
            ui.label(small(self.status.clone()));
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
                    if theme::primary_button(ui, t!("dialog.save")).clicked()
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
        if let Some((sig, name)) = self.signatures.create_window(ctx) {
            if !name.is_empty() && self.settings.signer_name.as_deref() != Some(&name) {
                self.settings.signer_name = Some(name);
                self.settings.save();
            }
            if let Some(v) = self.view_mut()
                && v.doc.can_sign()
            {
                v.start_placing(ctx, sig);
                self.status = t!("sign.placing_status");
            }
        }
        self.password_window(ctx);
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
            ui.add_space(6.0);
            let mut theme = self.settings.theme.clone();
            ui.horizontal(|ui| {
                ui.label(t!("settings.theme"));
                ui.selectable_value(&mut theme, None, t!("settings.theme_system"));
                ui.selectable_value(&mut theme, Some("light".to_string()), t!("settings.theme_light"));
                ui.selectable_value(&mut theme, Some("dark".to_string()), t!("settings.theme_dark"));
            });
            if theme != self.settings.theme {
                self.settings.theme = theme;
                self.settings.apply_theme(ui.ctx());
                self.settings.save();
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
        self.before_frame(ctx, raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}

/// Lo que hace cada fotograma, con eframe o con `soft::run`.
impl PdfApp {
    pub fn before_frame(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(t) = self.autotest.as_mut() {
            t.raw_input(ctx, raw);
        }
    }

    pub fn frame(&mut self, ui: &mut egui::Ui) {
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

        let p = theme::palette(ui);
        self.title_bar(ui);
        egui::Panel::top("toolbar")
            .exact_size(46.0)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(8, 0)).stroke(egui::Stroke::new(1.0, p.border)))
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| self.status_bar(ui));

        let mut ev = None;
        let mut open_requested = false;
        egui::CentralPanel::default_margins().frame(egui::Frame::NONE).show(ui, |ui| match self.view_mut() {
            Some(v) => ev = Some(v.ui(ui)),
            None => {
                if self.empty_state(ui) {
                    open_requested = true;
                }
            }
        });
        if open_requested {
            self.open_dialog();
        }

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
            if let Some(e) = ev.sign_error {
                self.error = Some(t!("sign.error", e));
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
                    let _ = crate::platform::open_external(&u);
                    self.status = t!("status.opening_link", u);
                }
            }
        }
        if let Some(v) = self.view_mut() {
            v.trim_cache();
        }

        self.dialogs(&ctx);
        self.update_title(&ctx);
        chrome::window_border(&ctx);
        chrome::resize_edges(&ctx);
    }

    /// Ventana sin documentos: una invitación a abrir uno. true si se pulsa.
    fn empty_state(&self, ui: &mut egui::Ui) -> bool {
        let p = theme::palette(ui);
        ui.painter().rect_filled(ui.max_rect(), 0.0, p.canvas);
        let mut open = false;
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            ui.add_space((ui.available_height() / 2.0 - 110.0).max(0.0));
            ui.label(RichText::new(icon::FILE_PDF).size(64.0).color(p.text_weak));
            ui.add_space(8.0);
            ui.label(RichText::new(t!("empty.title")).size(20.0).color(p.text));
            ui.label(RichText::new(t!("empty.hint")).color(p.text_weak));
            ui.add_space(14.0);
            open = theme::primary_button(ui, format!("{}  {}", icon::FOLDER_OPEN, t!("toolbar.open"))).clicked();
        });
        open
    }
}

/// Qué cabe en la barra de herramientas: con poco ancho se compactan
/// cosas, en este orden, para que nada se monte encima de otra cosa. Lo que
/// se oculta sigue en el menú ☰ (Ver) o con el teclado.
struct ToolbarFit {
    tool_labels: bool,
    page_long: bool,
    search_width: f32,
    layouts: bool,
    page_nav: bool,
    zoom: bool,
}

/// Anchos de las partes de la barra que dependen del texto.
struct ToolbarWidths {
    tools_labels: f32,
    tools_icons: f32,
    page_long: f32,
    page_short: f32,
    search_results: f32,
}

impl ToolbarFit {
    /// `avail`: ancho tras ☰, abrir, guardar e imprimir; `paged`: el
    /// documento tiene páginas (no es una XFA continua).
    fn new(avail: f32, paged: bool, w: &ToolbarWidths) -> Self {
        let mut f = ToolbarFit { tool_labels: true, page_long: true, search_width: 170.0, layouts: true, page_nav: true, zoom: true };
        const BUTTON: f32 = 36.0;
        const SEP: f32 = 13.0;
        let need = |f: &ToolbarFit| {
            // Herramientas y el botón de firmar.
            let mut n = SEP + BUTTON + if f.tool_labels { w.tools_labels } else { w.tools_icons };
            if paged && f.page_nav {
                n += SEP + 2.0 * BUTTON + if f.page_long { w.page_long } else { w.page_short };
                if f.layouts {
                    n += 3.0 * BUTTON;
                }
            }
            // Ajustar, separador y, si caben, alejar, porcentaje y acercar.
            n += BUTTON + SEP + if f.zoom { 2.0 * BUTTON + 48.0 } else { 0.0 };
            // Búsqueda: marco, lupa y resultados.
            // Y un hueco entre la parte izquierda y la derecha.
            n + f.search_width + 50.0 + w.search_results + 24.0
        };
        let steps: [fn(&mut ToolbarFit); 7] = [
            |f| f.tool_labels = false,
            |f| f.page_long = false,
            |f| f.search_width = 120.0,
            |f| f.layouts = false,
            |f| f.page_nav = false,
            |f| f.zoom = false,
            |f| f.search_width = 70.0,
        ];
        for step in steps {
            if need(&f) <= avail {
                break;
            }
            step(&mut f);
        }
        f
    }
}

/// Entrada del menú principal con su icono.
fn mi(glyph: &str, text: String) -> String {
    format!("{glyph}   {text}")
}

/// Separador vertical de la barra de herramientas.
fn separator(ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    let (r, _) = ui.allocate_exact_size(egui::vec2(9.0, 22.0), egui::Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), egui::Stroke::new(1.0, p.border));
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
    // app.response(): pregunta de texto, con un diálogo del sistema (los de
    // rfd no tienen campo de texto).
    doc.set_response_handler(Box::new(|question: &str, title: &str, default: &str| {
        let title = if title.is_empty() { t!("dialog.form_title") } else { title.to_string() };
        crate::platform::ask_text(&title, question, default)
    }));
}

#[allow(dead_code)]
fn _align() -> Align {
    Align::Center
}
