//! Aplicación de escritorio (egui/eframe).

use crate::chrome::{self, TabAction, TabInfo};
use crate::theme::{self, icon};
use crate::viewer::{AnnotRequest, AnnotTool, DocView, PageLayout, Tool};
use eframe::egui::{self, Align, Color32, Key, KeyboardShortcut, Layout, Modifiers, RichText};
use form_pdf_reader::pdfium::{AlertAnswer, AlertButtons, AlertIcon, AlertRequest, DigitalSignature, Document, FormKind, OpenOptions, PasswordRequired, SignedState};
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

/// El texto de una nota o de un texto libre, nuevo o para cambiarlo.
struct AnnotDialog {
    req: AnnotRequest,
    text: String,
    /// Fotogramas en los que dar el foco al campo de texto.
    focus: u8,
}

/// Colores para anotar.
const ANNOT_COLORS: [[u8; 3]; 6] = [[255, 214, 0], [76, 175, 80], [33, 150, 243], [229, 57, 53], [156, 39, 176], [20, 20, 20]];

struct Tab {
    view: DocView,
    dirty: bool,
    search: String,
    /// Firmas digitales del documento y derechos de uso de Adobe (se leen
    /// al abrir y al guardar).
    digital: Vec<DigitalSignature>,
    usage_rights: bool,
    /// Bloqueado por una firma: no se puede modificar, guardar ni firmar.
    locked: bool,
}

impl Tab {
    fn new(view: DocView) -> Tab {
        let mut t = Tab { view, dirty: false, search: String::new(), digital: Vec::new(), usage_rights: false, locked: false };
        t.read_signatures();
        t
    }

    fn read_signatures(&mut self) {
        self.digital = self.view.doc.digital_signatures();
        self.usage_rights = self.view.doc.has_usage_rights();
        self.locked = self.view.doc.locked_by_signature();
        self.view.set_read_only(self.locked);
    }
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
    show_digital: bool,
    /// Fichero por abrir en cuanto se haya dibujado la ventana.
    pending_open: Option<PathBuf>,
    frames: u32,
    print_dialog: Option<crate::print::PrintDialog>,
    digital_dialog: Option<crate::digital_sign::DigitalSignDialog>,
    /// Pedido abrir el diálogo de firma digital (desde la barra).
    digital_requested: bool,
    /// Firma digital esperando a que se elija el sitio en la página.
    digital_pending: Option<Box<crate::digital_sign::SignPlan>>,
    update_dialog: Option<crate::update::UpdateDialog>,
    annot_dialog: Option<AnnotDialog>,
    pages_dialog: Option<crate::pages::PagesDialog>,
    /// La búsqueda de versiones nuevas al abrir el programa.
    update_check: Option<std::sync::mpsc::Receiver<anyhow::Result<Option<crate::update::Release>>>>,
    /// Preguntar si abrir los PDF con esta aplicación (Linux, la primera vez).
    ask_pdf_default: bool,
    /// Si abre los PDF por defecto (se mira al abrir los ajustes).
    pdf_default: Option<Option<bool>>,
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
            show_digital: false,
            pending_open: None,
            frames: 0,
            print_dialog: None,
            digital_dialog: None,
            digital_requested: false,
            digital_pending: None,
            update_dialog: None,
            annot_dialog: None,
            pages_dialog: None,
            update_check: None,
            ask_pdf_default: false,
            pdf_default: None,
        };
        if app.autotest.is_none() || std::env::var_os("PDFRE_AUTOTEST_STARTUP").is_some() {
            app.startup_checks();
        }
        // El fichero de la línea de órdenes se abre tras el primer fotograma:
        // así la ventana sale enseguida aunque abrirlo tarde (formularios
        // XFA grandes en equipos lentos).
        if let Some(p) = initial {
            app.status = t!("status.opening", file_name(&p));
            app.pending_open = Some(p);
        }
        app
    }

    /// Al abrir: buscar versiones nuevas (una vez al día) y, en Linux, si
    /// abrir los PDF con esta aplicación (una sola vez).
    fn startup_checks(&mut self) {
        std::thread::spawn(crate::update::clean_old_downloads);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if !self.settings.updates_off && now.saturating_sub(self.settings.update_checked) >= 24 * 3600 {
            self.update_check = Some(crate::update::check_in_background());
            self.settings.update_checked = now;
            self.settings.save();
        }
        if cfg!(not(windows)) && !self.settings.pdf_default_asked && crate::platform::is_pdf_default() == Some(false) {
            self.ask_pdf_default = true;
        }
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
                self.tabs.push(Tab::new(DocView::new(doc)));
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
        let r = theme::modal(ctx, "contraseña").show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(t!("dialog.password_title"));
            ui.add_space(4.0);
            ui.label(t!("dialog.password_prompt", file_name(&p.path)));
            if p.wrong {
                ui.colored_label(ui.visuals().error_fg_color, t!("dialog.password_wrong"));
            }
            ui.add_space(4.0);
            let field = ui.add(egui::TextEdit::singleline(&mut p.text).password(true).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(8, 5)));
            if std::mem::take(&mut p.focus) {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            match theme::dialog_footer(ui, &[(t!("toolbar.open"), true, true), (t!("dialog.cancel"), false, true)]) {
                Some(0) => submit = true,
                Some(_) => cancel = true,
                None => {}
            }
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
        if tab.locked {
            self.error = Some(t!("dsig.locked_save"));
            return false;
        }
        let target = match path {
            Some(p) => p,
            None => tab.view.doc.path().to_path_buf(),
        };
        tab.view.commit_focus();
        if let Err(e) = tab.view.commit_ink() {
            log::warn!("no se pudo escribir el dibujo: {e}");
        }
        match tab.view.doc.save(&target) {
            Ok(()) => {
                tab.dirty = false;
                tab.read_signatures();
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
    /// Abre el diálogo de imprimir.
    fn print(&mut self) {
        let last = self.settings.printer.clone();
        let Some(view) = self.view_mut() else { return };
        view.commit_focus();
        let dialog = crate::print::PrintDialog::new(&view.doc, view.current_page, last.as_deref());
        self.print_dialog = Some(dialog);
    }

    /// Imprimir con el visor del sistema: el PDF plano, abierto fuera.
    fn print_external(&mut self) {
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
                if ui.add_enabled(has, egui::Button::new(mi(icon::SEAL_CHECK, t!("digital.menu")))).clicked() {
                    ui.close();
                    self.show_digital = true;
                }
                ui.separator();
                let pages_ok = self.cur().is_some_and(|t| t.view.doc.can_edit_pages() && !t.locked);
                let r = ui.add_enabled(pages_ok, egui::Button::new(mi(icon::SQUARES_FOUR, t!("menu.pages"))));
                if r.on_disabled_hover_text(t!("pages.unavailable")).clicked() {
                    ui.close();
                    self.open_pages();
                }
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
                    self.pdf_default = None;
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
                if ui.add(egui::Button::new(mi(icon::CLOUD_ARROW_DOWN, t!("menu.check_updates")))).clicked() {
                    ui.close();
                    self.update_dialog = Some(crate::update::UpdateDialog::checking());
                }
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
            let menu = egui::Button::new(RichText::new(icon::LIST).size(18.0)).frame_when_inactive(true).min_size(egui::Vec2::splat(32.0));
            theme::flat(ui, |ui| egui::containers::menu::MenuButton::from_button(menu).ui(ui, |ui| self.main_menu(ui)).0.on_hover_text(t!("menu.main")));
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
            let digital_requested = &mut self.digital_requested;
            let status = &mut self.status;
            let Some(Tab { view: v, search, dirty, .. }) = self.tabs.get_mut(self.active) else { return };
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

            if !fit.search {
                *search_focus = false;
            }

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
            let mut sign_button = egui::Button::new(RichText::new(icon::SIGNATURE).size(18.0)).frame_when_inactive(true).min_size(egui::Vec2::splat(32.0));
            if v.is_placing() {
                sign_button = sign_button.fill(theme::palette(ui).accent.gamma_multiply(0.16));
            }
            // Bloqueado por una firma: ni manuscritas ni digitales.
            let locked = v.is_read_only();
            let mut action = None;
            ui.add_enabled_ui(!locked, |ui| {
                let (r, inner) = theme::flat(ui, |ui| egui::containers::menu::MenuButton::from_button(sign_button).ui(ui, |ui| self.signatures.menu(ui, can_sign)));
                r.on_hover_text(t!("sign.button_tip")).on_disabled_hover_text(t!("dsig.locked"));
                action = inner.and_then(|i| i.inner);
            });
            // Anotar: abre la barra de anotación.
            let annotating = v.is_annotating();
            let can = v.can_annotate();
            let r = ui.add_enabled_ui(can || annotating, |ui| theme::icon_toggle(ui, annotating, icon::HIGHLIGHTER, t!("annot.button_tip"))).inner;
            let r = r.on_disabled_hover_text(if v.is_read_only() { t!("dsig.locked") } else { t!("annot.unavailable") });
            if r.clicked() {
                if annotating {
                    match v.stop_annotating() {
                        Ok(changed) => *dirty |= changed,
                        Err(e) => *status = t!("annot.error", e),
                    }
                } else {
                    v.start_annotating();
                }
            }
            match action {
                Some(crate::signatures::Action::Digital) => *digital_requested = true,
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
                if fit.search {
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
                }
                let (fit_icon, fit_tip) = if v.effective_layout() == PageLayout::Single {
                    (icon::CORNERS_OUT, t!("toolbar.fit_page"))
                } else {
                    (icon::ARROWS_OUT_LINE_HORIZONTAL, t!("toolbar.fit_width"))
                };
                if fit.fit && theme::icon_toggle(ui, v.fit_width, fit_icon, fit_tip).clicked() {
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
            let mut open_digital = false;
            if let Some(t) = self.cur()
                && !t.digital.is_empty()
            {
                let (glyph, color, text) = digital_summary(&t.digital, &p);
                let r = ui.add(egui::Label::new(RichText::new(format!("{glyph} {text}")).size(12.0).color(color)).sense(egui::Sense::click()));
                open_digital = r.on_hover_text(t!("digital.menu")).clicked();
                ui.label(small("·".into()));
                if t.locked {
                    ui.label(RichText::new(format!("{} {}", icon::LOCK, t!("dsig.locked_status"))).size(12.0).color(p.text_weak)).on_hover_text(t!("dsig.locked"));
                    ui.label(small("·".into()));
                }
            }
            if open_digital {
                self.show_digital = true;
            }
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
            theme::modal(ctx, "error").show(ctx, |ui| {
                ui.set_max_width(480.0);
                ui.heading(t!("dialog.error"));
                ui.label(err);
                if theme::dialog_footer(ui, &[(t!("dialog.ok"), true, true)]).is_some() {
                    self.error = None;
                }
            });
        }
        if let Some(action) = self.confirm.clone() {
            let mut close = false;
            theme::modal(ctx, "confirmar").show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(t!("status.unsaved"));
                let name = self.view().map(|v| file_name(v.doc.path())).unwrap_or_default();
                ui.label(t!("dialog.unsaved_question", name));
                let buttons = [(t!("dialog.save"), true, true), (t!("dialog.discard"), false, true), (t!("dialog.cancel"), false, true)];
                match theme::dialog_footer(ui, &buttons) {
                    Some(0) => {
                        if self.save(None) {
                            close = true;
                            self.perform(action.clone());
                        }
                    }
                    Some(1) => {
                        close = true;
                        if let Some(t) = self.cur_mut() {
                            t.dirty = false;
                        }
                        self.perform(action.clone());
                    }
                    Some(_) => close = true,
                    None => {}
                }
            });
            if close {
                self.confirm = None;
            }
        }
        if self.show_settings {
            self.settings_window(ctx);
        }
        if self.show_digital {
            self.digital_window(ctx);
        }
        self.print_window(ctx);
        self.update_window(ctx);
        self.pdf_default_window(ctx);
        self.sign_digital_window(ctx);
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
        self.annot_window(ctx);
        self.pages_window(ctx);
        if self.show_about {
            theme::modal(ctx, "acerca").show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.heading(format!("Form PDF Reader {}", env!("CARGO_PKG_VERSION")));
                ui.label(t!("about.desc"));
                ui.label(t!("about.engine"));
                if theme::dialog_footer(ui, &[(t!("dialog.close"), false, true)]).is_some() {
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
            theme::modal(ctx, "adjuntos").show(ctx, |ui| {
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
                if theme::dialog_footer(ui, &[(t!("dialog.close"), false, true)]).is_some() {
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
            theme::modal(ctx, "xfa").show(ctx, |ui| {
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
                if theme::dialog_footer(ui, &[(t!("xfa.ok"), true, true)]).is_some() {
                    self.show_xfa_info = false;
                }
            });
        }
    }

    /// Las firmas digitales del documento.
    /// El diálogo de firma digital, y lo que sigue al pulsar "Firmar".
    fn sign_digital_window(&mut self, ctx: &egui::Context) {
        use crate::digital_sign::{DigitalSignDialog, Outcome};
        use form_pdf_reader::pades::Placement;
        if std::mem::take(&mut self.digital_requested)
            && let Some(tab) = self.tabs.get_mut(self.active)
        {
            tab.view.commit_focus();
            let doc = &tab.view.doc;
            let fields = doc.signature_fields();
            let s = &self.settings;
            self.digital_dialog = Some(DigitalSignDialog::new(fields, self.signatures.saved().to_vec(), !doc.can_sign(), s.timestamp, s.timestamp_server.clone()));
        }
        let Some(d) = self.digital_dialog.as_mut() else { return };
        let Some(out) = d.show(ctx) else { return };
        self.digital_dialog = None;
        let Outcome::Sign(plan) = out else { return };
        // Recordar lo elegido para el sello de tiempo.
        let server = Some(plan.timestamp_server.clone()).filter(|s| !s.is_empty() && s != form_pdf_reader::pades::tsa::DEFAULT_TSA);
        if plan.timestamp_on != self.settings.timestamp || server != self.settings.timestamp_server {
            self.settings.timestamp = plan.timestamp_on;
            self.settings.timestamp_server = server;
            self.settings.save();
        }
        match (&plan.field, plan.visible) {
            (None, true) => {
                // Primero el sitio en la página.
                let (w, h) = if plan.handwritten.is_some() { (240.0, 66.0) } else { (200.0, 54.0) };
                if let Some(v) = self.view_mut() {
                    v.start_placing_digital(w, h);
                }
                self.status = t!("dsig.place_status");
                self.digital_pending = Some(plan);
            }
            (Some(f), true) => {
                let f = f.clone();
                self.digital_sign(*plan, Placement::Field(f));
            }
            _ => self.digital_sign(*plan, Placement::Invisible),
        }
    }

    /// Firma el documento con lo elegido y lo guarda como copia firmada,
    /// que se abre en la misma pestaña.
    fn digital_sign(&mut self, plan: crate::digital_sign::SignPlan, placement: form_pdf_reader::pades::Placement) {
        let Some(view) = self.view() else { return };
        let p = view.doc.path().to_path_buf();
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| t!("file.default_stem"));
        let name = format!("{stem}_{}.pdf", t!("file.signed_suffix"));
        let target = match std::env::var("PDFRE_AUTOTEST_SAVE").ok().filter(|_| crate::autotest::active()) {
            Some(t) => Some(PathBuf::from(t)),
            None => {
                let mut d = rfd::FileDialog::new().add_filter(t!("dialog.pdf_filter"), &["pdf"]).set_title(t!("dsig.save_title")).set_file_name(&name);
                if let Some(dir) = p.parent() {
                    d = d.set_directory(dir);
                }
                d.save_file()
            }
        };
        let Some(mut target) = target else { return };
        if target.extension().is_none() {
            target.set_extension("pdf");
        }
        let mut signer = match crate::certs::signer(&plan.cert, &plan.secret) {
            Ok(s) => s,
            Err(e) => {
                self.error = Some(t!("dsig.failed", format!("{e:#}")));
                return;
            }
        };
        let req = form_pdf_reader::pades::Request {
            placement,
            reason: Some(plan.reason).filter(|s| !s.is_empty()),
            location: Some(plan.location).filter(|s| !s.is_empty()),
            handwritten: plan.handwritten,
            timestamp: plan.timestamp,
            lock: plan.lock,
        };
        let Some(view) = self.view_mut() else { return };
        view.commit_focus();
        let result = view.doc.sign_digital(&req, signer.as_mut()).and_then(|bytes| form_pdf_reader::pdfium::write_atomic(&target, &bytes));
        drop(signer);
        match result {
            Ok(()) => {
                self.reopen(&target);
                self.status = t!("dsig.done", target.display());
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg != t!("print.cancelled") {
                    self.error = Some(t!("dsig.failed", msg));
                }
            }
        }
    }

    fn update_window(&mut self, ctx: &egui::Context) {
        use crate::update::{Outcome, UpdateDialog};
        if let Some(rx) = &self.update_check {
            match rx.try_recv() {
                Ok(r) => {
                    self.update_check = None;
                    match r {
                        Ok(Some(r)) if self.settings.update_skipped.as_deref() != Some(&r.version) => {
                            if self.update_dialog.is_none() {
                                self.update_dialog = Some(UpdateDialog::available(r));
                            }
                        }
                        Ok(_) => {}
                        Err(e) => log::info!("no se pudo buscar actualizaciones: {e}"),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(500)),
                Err(_) => self.update_check = None,
            }
        }
        let dirty = self.tabs.iter().any(|t| t.dirty);
        let Some(d) = self.update_dialog.as_mut() else { return };
        let out = d.show(ctx, dirty);
        if d.must_quit() {
            // El instalador de Windows está en marcha y abrirá la versión nueva.
            self.update_dialog = None;
            self.allow_quit = true;
            return;
        }
        let Some(out) = out else { return };
        self.update_dialog = None;
        match out {
            Outcome::Closed => {}
            Outcome::Skip(v) => {
                self.settings.update_skipped = Some(v);
                self.settings.save();
            }
            Outcome::Restart => {
                crate::update::restart(self.view().map(|v| v.doc.path()));
                self.allow_quit = true;
            }
        }
    }

    /// Linux, la primera vez: abrir los PDF con esta aplicación.
    fn pdf_default_window(&mut self, ctx: &egui::Context) {
        if !self.ask_pdf_default || self.update_dialog.is_some() {
            return;
        }
        theme::modal(ctx, "pdf_predeterminada").show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(t!("assoc.title"));
            ui.add_space(6.0);
            ui.label(t!("assoc.question"));
            let answer = theme::dialog_footer(ui, &[(t!("assoc.yes"), true, true), (t!("assoc.no"), false, true)]);
            if let Some(i) = answer {
                self.ask_pdf_default = false;
                self.settings.pdf_default_asked = true;
                self.settings.save();
                if i == 0 {
                    self.make_pdf_default();
                }
            }
        });
    }

    fn make_pdf_default(&mut self) {
        self.pdf_default = None;
        match crate::platform::make_pdf_default() {
            Ok(true) => self.status = t!("assoc.done"),
            Ok(false) => self.status = t!("assoc.confirm_windows"),
            Err(e) => self.error = Some(t!("assoc.failed", e)),
        }
    }

    /// Barra de anotar: herramienta, color, grosor o tamaño y terminar.
    fn annot_bar(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let mut error = None;
        let mut edit = None;
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let v = &mut tab.view;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let Some(tool) = v.annot_state().map(|a| a.tool) else { return };
            let mut t = tool;
            theme::segmented(
                ui,
                &mut t,
                &[
                    (AnnotTool::Select, icon::CURSOR, t!("annot.select"), t!("annot.select_tip")),
                    (AnnotTool::Highlight, icon::HIGHLIGHTER, t!("annot.highlight"), t!("annot.markup_tip")),
                    (AnnotTool::Underline, icon::TEXT_UNDERLINE, t!("annot.underline"), t!("annot.markup_tip")),
                    (AnnotTool::StrikeOut, icon::TEXT_STRIKETHROUGH, t!("annot.strikeout"), t!("annot.markup_tip")),
                    (AnnotTool::Note, icon::NOTE, t!("annot.note"), t!("annot.note_tip")),
                    (AnnotTool::FreeText, icon::TEXT_T, t!("annot.text"), t!("annot.text_tip")),
                    (AnnotTool::Ink, icon::SCRIBBLE, t!("annot.ink"), t!("annot.ink_tip")),
                ],
                false,
            );
            match v.set_annot_tool(t) {
                Ok(changed) => tab.dirty |= changed,
                Err(e) => error = Some(e),
            }
            separator(ui);
            let Some(a) = v.annot_state() else { return };
            if t != AnnotTool::Select {
                // Colores.
                let cur = a.color();
                for c in ANNOT_COLORS {
                    let (rect, r) = ui.allocate_exact_size(egui::Vec2::splat(22.0), egui::Sense::click());
                    let col = Color32::from_rgb(c[0], c[1], c[2]);
                    ui.painter().circle_filled(rect.center(), 8.0, col);
                    if c == cur {
                        ui.painter().circle_stroke(rect.center(), 10.5, egui::Stroke::new(2.0, p.accent_text));
                    } else if r.hovered() {
                        ui.painter().circle_stroke(rect.center(), 10.5, egui::Stroke::new(1.0, p.border));
                    }
                    if r.clicked() {
                        a.set_color(c);
                    }
                }
            }
            match t {
                AnnotTool::Ink => {
                    separator(ui);
                    let mut w = a.ink_width;
                    for (val, label) in [(1.0, t!("annot.thin")), (2.0, t!("annot.medium")), (4.0, t!("annot.thick"))] {
                        if ui.selectable_label(w == val, label).clicked() {
                            w = val;
                        }
                    }
                    a.ink_width = w;
                    if a.has_strokes() {
                        separator(ui);
                        if theme::dialog_button(ui, format!("{}  {}", icon::CHECK, t!("annot.ink_done")), true, true).clicked() {
                            match v.commit_ink() {
                                Ok(changed) => tab.dirty |= changed,
                                Err(e) => error = Some(e),
                            }
                        }
                        if ui.button(t!("annot.ink_discard")).clicked() {
                            v.discard_ink();
                        }
                    }
                }
                AnnotTool::FreeText => {
                    separator(ui);
                    ui.label(RichText::new(t!("annot.size")).color(p.text_weak));
                    let mut size = a.font_size;
                    egui::ComboBox::from_id_salt("tamaño_texto").width(60.0).selected_text(format!("{size}")).show_ui(ui, |ui| {
                        for s in [8.0, 10.0, 12.0, 14.0, 18.0, 24.0, 36.0] {
                            ui.selectable_value(&mut size, s, format!("{s}"));
                        }
                    });
                    a.font_size = size;
                }
                AnnotTool::Select => {
                    if let Some(sel) = a.selected.clone() {
                        if matches!(sel.kind, form_pdf_reader::pdfium::AnnotKind::Note | form_pdf_reader::pdfium::AnnotKind::FreeText)
                            && ui.button(format!("{}  {}", icon::PENCIL_SIMPLE, t!("annot.edit"))).clicked()
                        {
                            edit = Some(sel.clone());
                        }
                        if ui.button(format!("{}  {}", icon::TRASH, t!("annot.delete"))).on_hover_text(t!("annot.delete_tip")).clicked() {
                            match v.delete_selected_annotation() {
                                Ok(changed) => tab.dirty |= changed,
                                Err(e) => error = Some(e),
                            }
                        }
                    } else {
                        ui.label(RichText::new(t!("annot.select_hint")).color(p.text_weak));
                    }
                }
                _ => {}
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::dialog_button(ui, t!("annot.finish"), false, true).on_hover_text(t!("annot.finish_tip")).clicked() {
                    match v.stop_annotating() {
                        Ok(changed) => tab.dirty |= changed,
                        Err(e) => error = Some(e),
                    }
                }
            });
        });
        if let Some(a) = edit {
            self.annot_dialog = Some(AnnotDialog { text: a.contents.clone(), req: AnnotRequest::Edit(a), focus: 4 });
        }
        if let Some(e) = error {
            self.error = Some(t!("annot.error", e));
        }
    }

    /// El texto de una nota o de un texto libre.
    fn annot_window(&mut self, ctx: &egui::Context) {
        let Some(d) = self.annot_dialog.as_mut() else { return };
        let mut answer = None;
        theme::modal(ctx, "anotacion").show(ctx, |ui| {
            ui.set_width(380.0);
            let title = match &d.req {
                AnnotRequest::Note { .. } => t!("annot.new_note"),
                AnnotRequest::FreeText { .. } => t!("annot.new_text"),
                AnnotRequest::Edit(a) if a.kind == form_pdf_reader::pdfium::AnnotKind::Note => t!("annot.edit_note"),
                AnnotRequest::Edit(_) => t!("annot.edit_text"),
            };
            ui.heading(title);
            ui.add_space(8.0);
            let r = ui.add(egui::TextEdit::multiline(&mut d.text).desired_rows(4).desired_width(f32::INFINITY));
            // Unos fotogramas: al soltar el clic que abrió la ventana, el
            // visor pide el foco para sí.
            if d.focus > 0 {
                r.request_focus();
                d.focus -= 1;
            }
            let ok = !d.text.trim().is_empty() || matches!(d.req, AnnotRequest::Note { .. });
            answer = theme::dialog_footer(ui, &[(t!("dialog.ok"), true, ok), (t!("dialog.cancel"), false, true)]);
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                answer = Some(1);
            }
        });
        let Some(answer) = answer else { return };
        let d = self.annot_dialog.take().expect("diálogo abierto");
        if answer != 0 {
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let r = match &d.req {
            AnnotRequest::Note { page, x, y } => tab.view.add_note(*page, *x, *y, &d.text),
            AnnotRequest::FreeText { page, x, y } => tab.view.add_free_text(*page, *x, *y, &d.text),
            AnnotRequest::Edit(a) => tab.view.set_annotation_text(a, &d.text),
        };
        match r {
            Ok(()) => tab.dirty = true,
            Err(e) => self.error = Some(t!("annot.error", e)),
        }
    }

    fn open_pages(&mut self) {
        let Some(t) = self.cur_mut() else { return };
        if let Err(e) = t.view.stop_annotating() {
            log::warn!("no se pudo escribir el dibujo: {e}");
        }
        let signed = !t.digital.is_empty();
        let mut d = crate::pages::PagesDialog::new(signed);
        d.reset(t.view.page_count(), [t.view.current_page]);
        self.pages_dialog = Some(d);
    }

    fn pages_window(&mut self, ctx: &egui::Context) {
        use crate::pages::Outcome;
        let Some(d) = self.pages_dialog.as_mut() else { return };
        let Some(tab) = self.tabs.get(self.active) else {
            self.pages_dialog = None;
            return;
        };
        let Some(out) = d.show(ctx, &tab.view.doc) else { return };
        match out {
            Outcome::Close => self.pages_dialog = None,
            Outcome::Edit(edits) => self.apply_page_edits(edits),
            Outcome::Extract(pages) => self.extract_pages(&pages),
            Outcome::Insert(at) => self.insert_pdf(at),
        }
    }

    /// Cambia las páginas del documento de la pestaña activa y lo vuelve a
    /// abrir (con los cambios sin guardar).
    fn apply_page_edits(&mut self, edits: Vec<form_pdf_reader::pdfium::pages::PageEdit>) {
        use form_pdf_reader::pdfium::pages::{self, PageEdit};
        let continuous_xfa = self.continuous_xfa;
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let r = (|| -> anyhow::Result<Document> {
            tab.view.commit_focus();
            let bytes = tab.view.doc.bytes_for_save()?;
            let password = tab.view.doc.password().map(str::to_string);
            let new = pages::edit_pages(&bytes, password.as_deref(), &edits)?;
            Document::from_bytes(new, tab.view.doc.path().to_path_buf(), OpenOptions { continuous_xfa, password })
        })();
        let doc = match r {
            Ok(d) => d,
            Err(e) => {
                if let Some(p) = e.downcast_ref::<PasswordRequired>()
                    && let Some(PageEdit::Insert { pdf, at, .. }) = edits.into_iter().find(|e| matches!(e, PageEdit::Insert { .. }))
                    && let Some(d) = self.pages_dialog.as_mut()
                {
                    d.ask_password(pdf, at, p.wrong);
                    return;
                }
                self.error = Some(t!("pages.error", format!("{e:#}")));
                return;
            }
        };
        install_handlers(&doc);
        let n = doc.page_count();
        // Las páginas que quedan elegidas tras el cambio.
        let select: Vec<usize> = match edits.last() {
            Some(PageEdit::Rotate { pages, .. }) => pages.clone(),
            Some(PageEdit::Move { pages, to }) => (*to..*to + pages.len()).collect(),
            Some(PageEdit::Insert { pdf, password, at }) => {
                let k = pages::page_count(pdf, password.as_deref()).unwrap_or(0);
                (*at..*at + k).collect()
            }
            Some(PageEdit::Delete { pages }) => vec![pages.iter().min().copied().unwrap_or(0).min(n.saturating_sub(1))],
            None => Vec::new(),
        };
        let old = std::mem::replace(&mut tab.view, DocView::new(doc));
        tab.view.zoom = old.zoom;
        tab.view.fit_width = old.fit_width;
        tab.view.layout = old.layout;
        tab.view.set_tool(old.tool);
        tab.view.go_to_page(select.first().copied().unwrap_or(old.current_page).min(n.saturating_sub(1)));
        drop(old);
        tab.dirty = true;
        tab.read_signatures();
        self.status = t!("pages.changed");
        if let Some(d) = self.pages_dialog.as_mut() {
            d.reset(n, select);
        }
    }

    /// Elige un PDF e inserta sus páginas delante de la página `at`.
    fn insert_pdf(&mut self, at: usize) {
        use form_pdf_reader::pdfium::pages::PageEdit;
        let Some(path) = rfd::FileDialog::new().add_filter(t!("dialog.pdf_filter"), &["pdf"]).set_title(t!("pages.insert_title")).pick_file() else { return };
        if let Some(d) = self.pages_dialog.as_mut() {
            d.set_insert_name(file_name(&path));
        }
        match std::fs::read(&path) {
            // Si pide contraseña, la ventana de páginas la pregunta.
            Ok(pdf) => self.apply_page_edits(vec![PageEdit::Insert { pdf, password: None, at }]),
            Err(e) => self.error = Some(t!("error.open", path.display(), e)),
        }
    }

    /// Guarda unas páginas como un PDF nuevo.
    fn extract_pages(&mut self, pages: &[usize]) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let stem = tab.view.doc.path().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| t!("file.default_stem"));
        let mut d = rfd::FileDialog::new().add_filter(t!("dialog.pdf_filter"), &["pdf"]).set_title(t!("pages.extract_title")).set_file_name(format!("{stem}-{}.pdf", t!("pages.extract_suffix")));
        if let Some(dir) = tab.view.doc.path().parent() {
            d = d.set_directory(dir);
        }
        let Some(target) = d.save_file() else { return };
        let r = (|| -> anyhow::Result<()> {
            tab.view.commit_focus();
            let bytes = tab.view.doc.bytes_for_save()?;
            let out = form_pdf_reader::pdfium::pages::extract_pages(&bytes, tab.view.doc.password(), pages)?;
            form_pdf_reader::pdfium::write_atomic(&target, &out)
        })();
        match r {
            Ok(()) => self.status = t!("pages.extracted", pages.len(), target.display()),
            Err(e) => self.error = Some(t!("error.save", format!("{e:#}"))),
        }
    }

    fn print_window(&mut self, ctx: &egui::Context) {
        use crate::print::Outcome;
        let Some(d) = self.print_dialog.as_mut() else { return };
        let out = match self.tabs.get_mut(self.active) {
            Some(tab) => d.show(ctx, &mut tab.view.doc),
            None => Some(Outcome::Closed),
        };
        let Some(out) = out else { return };
        let printer = d.printer_name();
        self.print_dialog = None;
        match out {
            Outcome::Closed => {}
            Outcome::Sent(msg) => {
                self.status = msg;
                if printer.is_some() && printer != self.settings.printer {
                    self.settings.printer = printer;
                    self.settings.save();
                }
            }
            Outcome::Failed(e) => self.error = Some(t!("print.failed", e)),
            Outcome::OpenExternal => self.print_external(),
        }
    }

    fn digital_window(&mut self, ctx: &egui::Context) {
        let (sigs, rights) = match self.cur() {
            Some(t) => (t.digital.clone(), t.usage_rights),
            None => (Vec::new(), false),
        };
        let r = theme::modal(ctx, "firmas_digitales").show(ctx, |ui| {
            let p = theme::palette(ui);
            ui.set_width(480.0);
            ui.heading(t!("digital.title"));
            ui.add_space(6.0);
            if sigs.is_empty() {
                ui.label(RichText::new(t!("digital.none")).color(p.text_weak));
            }
            for s in &sigs {
                egui::Frame::new().stroke(egui::Stroke::new(1.0, p.border)).corner_radius(8).inner_margin(12).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let (glyph, color, state) = state_text(s.state, &p);
                    ui.label(RichText::new(s.signer.clone().unwrap_or_else(|| t!("digital.unknown_signer"))).size(16.0).strong());
                    ui.label(RichText::new(format!("{glyph}  {state}")).color(color));
                    ui.add_space(4.0);
                    let row = |ui: &mut egui::Ui, k: String, v: &Option<String>| {
                        if let Some(v) = v {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(k).color(p.text_weak));
                                ui.label(v);
                            });
                        }
                    };
                    let kind = match s.certification {
                        Some(1) => t!("digital.cert_none"),
                        Some(2) => t!("digital.cert_fill"),
                        Some(_) => t!("digital.cert_annots"),
                        None => t!("digital.approval"),
                    };
                    row(ui, t!("digital.kind"), &Some(kind));
                    row(ui, t!("digital.date"), &s.time);
                    row(ui, t!("digital.reason"), &s.reason);
                    row(ui, t!("digital.issuer"), &s.issuer);
                    let validity = match (&s.valid_from, &s.valid_to) {
                        (Some(a), Some(b)) => Some(format!("{a} – {b}")),
                        _ => None,
                    };
                    row(ui, t!("digital.validity"), &validity);
                });
                ui.add_space(6.0);
            }
            if rights {
                ui.label(RichText::new(format!("{}  {}", icon::SEAL, t!("digital.usage_rights"))).color(p.text_weak));
                ui.add_space(4.0);
            }
            if !sigs.is_empty() {
                ui.label(RichText::new(t!("digital.trust_note")).size(12.0).color(p.text_weak));
            }
            theme::dialog_footer(ui, &[(t!("dialog.close"), false, true)]).is_some()
        });
        if r.inner || r.should_close() {
            self.show_digital = false;
        }
    }

    /// Ajustes: el idioma de la interfaz, que cambia en el acto y se recuerda.
    fn settings_window(&mut self, ctx: &egui::Context) {
        use form_pdf_reader::i18n;
        theme::modal(ctx, "ajustes").show(ctx, |ui| {
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
                let mut choice = match theme.as_deref() {
                    Some("light") => 1,
                    Some("dark") => 2,
                    _ => 0,
                };
                theme::segmented(
                    ui,
                    &mut choice,
                    &[
                        (0, icon::MONITOR, t!("settings.theme_system"), t!("settings.theme_system")),
                        (1, icon::SUN, t!("settings.theme_light"), t!("settings.theme_light")),
                        (2, icon::MOON, t!("settings.theme_dark"), t!("settings.theme_dark")),
                    ],
                    true,
                );
                theme = match choice {
                    1 => Some("light".to_string()),
                    2 => Some("dark".to_string()),
                    _ => None,
                };
            });
            if theme != self.settings.theme {
                self.settings.theme = theme;
                self.settings.apply_theme(ui.ctx());
                self.settings.save();
            }
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            let mut check = !self.settings.updates_off;
            ui.horizontal(|ui| {
                ui.checkbox(&mut check, t!("settings.update_check"));
                ui.add_space(8.0);
                if ui.button(t!("settings.update_now")).clicked() {
                    self.update_dialog = Some(crate::update::UpdateDialog::checking());
                }
            });
            if check == self.settings.updates_off {
                self.settings.updates_off = !check;
                self.settings.save();
            }
            ui.label(RichText::new(t!("settings.version", crate::update::current_version())).size(12.0).color(theme::palette(ui).text_weak));
            ui.add_space(6.0);
            match *self.pdf_default.get_or_insert_with(crate::platform::is_pdf_default) {
                Some(true) => {
                    ui.label(format!("{}  {}", icon::CHECK, t!("assoc.is_default")));
                }
                _ => {
                    if ui.button(mi(icon::FILE_PDF, t!("assoc.make_default"))).clicked() {
                        self.make_pdf_default();
                    }
                }
            }
            if theme::dialog_footer(ui, &[(t!("dialog.close"), false, true)]).is_some() {
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
        if self.view().is_some_and(|v| v.is_annotating()) {
            egui::Panel::top("annot_bar")
                .exact_size(42.0)
                .frame(egui::Frame::new().fill(p.surface).inner_margin(egui::Margin::symmetric(8, 0)).stroke(egui::Stroke::new(1.0, p.border)))
                .show(ui, |ui| self.annot_bar(ui));
        }
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
            if let Some((page, rect)) = ev.digital_placed
                && let Some(plan) = self.digital_pending.take()
            {
                self.digital_sign(*plan, form_pdf_reader::pades::Placement::New { page, rect });
            }
            if ev.digital_cancelled {
                self.digital_pending = None;
                self.status.clear();
            }
            if ev.print_requested {
                self.print();
            }
            if let Some(e) = ev.sign_error {
                self.error = Some(t!("sign.error", e));
            }
            if let Some(e) = ev.annot_error {
                self.error = Some(t!("annot.error", e));
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
        // Lo que pide la herramienta de anotar (el texto de una nota...).
        if self.annot_dialog.is_none()
            && let Some(req) = self.view_mut().and_then(|v| v.take_annot_request())
        {
            let text = if let AnnotRequest::Edit(a) = &req { a.contents.clone() } else { String::new() };
            self.annot_dialog = Some(AnnotDialog { req, text, focus: 4 });
        }
        if let Some(v) = self.view_mut() {
            v.trim_cache();
        }

        self.dialogs(&ctx);
        self.update_title(&ctx);
        self.frames = self.frames.saturating_add(1);
        if self.frames > 1
            && let Some(p) = self.pending_open.take()
        {
            self.open(&p);
        } else if self.pending_open.is_some() {
            ctx.request_repaint();
        }
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
    /// Ajustar al ancho (también en ☰ > Ver).
    fit: bool,
    search: bool,
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
        let mut f = ToolbarFit { tool_labels: true, page_long: true, search_width: 170.0, layouts: true, page_nav: true, zoom: true, fit: true, search: true };
        const BUTTON: f32 = 36.0;
        const SEP: f32 = 13.0;
        let need = |f: &ToolbarFit| {
            // Herramientas y los botones de firmar y anotar.
            let mut n = SEP + 2.0 * BUTTON + if f.tool_labels { w.tools_labels } else { w.tools_icons };
            if paged && f.page_nav {
                n += SEP + 2.0 * BUTTON + if f.page_long { w.page_long } else { w.page_short };
                if f.layouts {
                    n += 3.0 * BUTTON;
                }
            }
            // Ajustar, alejar, porcentaje y acercar, si caben.
            n += if f.fit { BUTTON } else { 0.0 } + if f.zoom { 2.0 * BUTTON + 48.0 } else { 0.0 };
            // Búsqueda: separador, marco, lupa y resultados.
            if f.search {
                n += SEP + f.search_width + 50.0 + w.search_results;
            }
            // Y un hueco entre la parte izquierda y la derecha.
            n + 24.0
        };
        let steps: [fn(&mut ToolbarFit); 9] = [
            |f| f.tool_labels = false,
            |f| f.page_long = false,
            |f| f.search_width = 120.0,
            |f| f.layouts = false,
            |f| f.page_nav = false,
            |f| f.zoom = false,
            |f| f.search_width = 70.0,
            |f| f.fit = false,
            |f| f.search = false,
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

/// Icono, color y texto de cómo está una firma.
fn state_text(s: SignedState, p: &theme::Palette) -> (&'static str, Color32, String) {
    match s {
        SignedState::Intact => (icon::SEAL_CHECK, p.ok, t!("digital.intact")),
        SignedState::ChangedAfter => (icon::SEAL_WARNING, p.warn, t!("digital.changed_after")),
        SignedState::Broken => (icon::SEAL_WARNING, p.danger, t!("digital.broken")),
        SignedState::Unknown => (icon::SEAL, p.text_weak, t!("digital.unknown")),
    }
}

/// Resumen para la barra de estado: el peor estado de todas las firmas.
fn digital_summary(sigs: &[DigitalSignature], p: &theme::Palette) -> (&'static str, Color32, String) {
    let worst = |s: SignedState| match s {
        SignedState::Broken => 3,
        SignedState::ChangedAfter => 2,
        SignedState::Unknown => 1,
        SignedState::Intact => 0,
    };
    let state = sigs.iter().map(|s| s.state).max_by_key(|s| worst(*s)).unwrap_or(SignedState::Unknown);
    let (glyph, color, _) = state_text(state, p);
    let text = if sigs.len() == 1 { t!("digital.status_one") } else { t!("digital.status_many", sigs.len()) };
    (glyph, color, text)
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
