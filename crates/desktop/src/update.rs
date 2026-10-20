//! Actualizaciones: mira en GitHub si hay una versión nueva, descarga el
//! paquete que corresponde a cómo está instalado el programa, comprueba su
//! SHA-256 y lo instala.
//!
//! Las descargas van con `curl`, que traen Linux y Windows 10/11: así el
//! programa no necesita su propia pila TLS.
//!
//! - Windows con el instalador: el instalador nuevo, en silencio, y se vuelve
//!   a abrir el programa al terminar.
//! - Linux con el paquete .deb, .rpm o de Arch: el paquete, con `pkexec`
//!   (pide la contraseña de administrador).
//! - Linux con el .tar.gz (`instalar.sh`, en `~/.local`): se descomprime y se
//!   instala en el mismo sitio.
//! - Lo demás (versión portable, AUR, compilado a mano): solo se avisa y se
//!   abre la página de la versión.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use form_pdf_reader::t;
use sha2::{Digest, Sha256};

const REPO: &str = "blzkz/form-pdf-reader";

/// Un fichero de una versión publicada.
#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// SHA-256 que publica GitHub, en hexadecimal.
    pub sha256: Option<String>,
}

/// Una versión publicada.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub page: String,
    /// El texto de la versión en GitHub (ver `notes_for`).
    pub notes: String,
    pub assets: Vec<Asset>,
}

/// Cómo está instalado el programa.
#[derive(Debug, Clone, PartialEq)]
// Cada sistema solo construye las suyas.
#[allow(dead_code)]
pub enum Install {
    /// Windows, con el instalador (para todos los usuarios o solo este).
    WindowsSetup { all_users: bool },
    Deb,
    Rpm,
    Arch,
    /// El .tar.gz instalado con `instalar.sh` en este prefijo.
    Tarball { prefix: PathBuf },
    /// No se sabe actualizar solo: el motivo, para el usuario.
    Manual(String),
}

/// La versión de este programa (`FORM_PDF_READER_VERSION` la cambia, para
/// probar).
pub fn current_version() -> String {
    std::env::var("FORM_PDF_READER_VERSION").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn parse_version(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split(['.', '-']).map(|p| p.parse::<u64>().unwrap_or(0)).collect()
}

/// `a` es más nueva que `b`.
pub fn newer(a: &str, b: &str) -> bool {
    parse_version(a) > parse_version(b)
}

fn curl() -> Command {
    let mut c = Command::new("curl");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Sin ventana de consola.
        c.creation_flags(0x0800_0000);
    }
    c.args(["--silent", "--show-error", "--fail", "--location", "--max-redirs", "5", "--user-agent"]);
    c.arg(format!("form-pdf-reader/{}", env!("CARGO_PKG_VERSION")));
    c
}

/// La última versión publicada.
pub fn latest() -> Result<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let out = curl().args(["--max-time", "20", "-H", "Accept: application/vnd.github+json", &url]).output().context(t!("update.no_curl"))?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Asset {
                        name: x.get("name")?.as_str()?.to_string(),
                        url: x.get("browser_download_url")?.as_str()?.to_string(),
                        size: x.get("size").and_then(|n| n.as_u64()).unwrap_or(0),
                        sha256: x.get("digest").and_then(|d| d.as_str()).and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release { version: s("tag_name").trim_start_matches('v').to_string(), page: s("html_url"), notes: s("body"), assets })
}

/// Las novedades en texto llano: sin las marcas de Markdown y con los
/// enlaces a las PR de GitHub acortados ("… by @x in https://…/pull/7" → "… (#7)").
fn plain_notes(md: &str) -> String {
    let mut out = Vec::new();
    for line in md.lines() {
        let l = line.trim();
        // Lo que añade GitHub en inglés.
        if l.starts_with("**Full Changelog**") || l.trim_start_matches('#').trim() == "What's Changed" {
            continue;
        }
        let l = l.trim_start_matches('#').trim_start();
        let mut l = match l.strip_prefix("* ").or_else(|| l.strip_prefix("- ")) {
            Some(rest) => format!("•  {rest}"),
            None => l.to_string(),
        };
        if let Some(i) = l.find(" by @")
            && let Some(n) = l[i..].rfind("/pull/").map(|j| &l[i + j + 6..])
            && n.chars().all(|c| c.is_ascii_digit())
        {
            l = format!("{} (#{n})", &l[..i]);
        }
        out.push(l.replace("**", ""));
    }
    out.join("\n").trim().to_string()
}

/// Las novedades en el idioma `lang`. El texto de la versión trae una
/// sección por idioma (de `packaging/novedades/<versión>.md`), cada una tras una
/// marca `<!-- lang:xx -->` y hasta la siguiente marca (`<!-- end -->` al
/// final); sin la del idioma, la inglesa. Las versiones sin secciones solo
/// traen la lista de cambios de GitHub, con los títulos en español: solo se
/// enseña con la interfaz en español.
pub fn notes_for(body: &str, lang: &str) -> Option<String> {
    let mut sections: Vec<(&str, Vec<&str>)> = Vec::new();
    let mut current: Option<usize> = None;
    for line in body.lines() {
        let l = line.trim();
        if let Some(mark) = l.strip_prefix("<!--").and_then(|m| m.strip_suffix("-->")).map(str::trim) {
            current = mark.strip_prefix("lang:").map(|code| {
                sections.push((code.trim(), Vec::new()));
                sections.len() - 1
            });
            continue;
        }
        if let Some(i) = current {
            // El título de la sección ("## English") sobra en la ventana.
            if sections[i].1.is_empty() && (l.is_empty() || l.starts_with('#')) {
                continue;
            }
            sections[i].1.push(line);
        }
    }
    if sections.is_empty() {
        return (lang == "es").then(|| plain_notes(body)).filter(|n| !n.is_empty());
    }
    let pick = sections.iter().position(|(c, _)| *c == lang).or_else(|| sections.iter().position(|(c, _)| *c == "en"))?;
    let lines = &mut sections[pick].1;
    // Ni el título de la sección siguiente, si va antes de su marca.
    while lines.last().is_some_and(|l| l.trim().is_empty() || l.trim().starts_with('#')) {
        lines.pop();
    }
    Some(plain_notes(&lines.join("\n"))).filter(|n| !n.is_empty())
}

/// Hay una versión más nueva que esta.
pub fn check() -> Result<Option<Release>> {
    let r = latest()?;
    Ok(newer(&r.version, &current_version()).then_some(r))
}

/// Cómo está instalado este programa.
pub fn install_kind() -> Install {
    let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize()) else {
        return Install::Manual(t!("update.manual_unknown"));
    };
    platform_kind(&exe)
}

#[cfg(windows)]
fn platform_kind(exe: &Path) -> Install {
    let dir = exe.parent().unwrap_or(Path::new("."));
    // El desinstalador de Inno Setup: lo instaló el instalador.
    if dir.join("unins000.exe").exists() {
        let pf = [std::env::var_os("ProgramFiles"), std::env::var_os("ProgramW6432")];
        let all_users = pf.iter().flatten().any(|p| dir.starts_with(p));
        return Install::WindowsSetup { all_users };
    }
    Install::Manual(t!("update.manual_portable"))
}

#[cfg(not(windows))]
fn platform_kind(exe: &Path) -> Install {
    let ok = |cmd: &str, args: &[&str]| Command::new(cmd).args(args).output().ok().filter(|o| o.status.success());
    let path = exe.to_string_lossy();
    if exe.starts_with("/usr") {
        if ok("dpkg", &["-S", &path]).is_some() {
            return Install::Deb;
        }
        if let Some(o) = ok("pacman", &["-Qqo", &path]) {
            // El paquete de las versiones publicadas se llama así; los de AUR
            // (form-pdf-reader-bin...) se actualizan con su ayudante.
            return if String::from_utf8_lossy(&o.stdout).trim() == "form-pdf-reader" { Install::Arch } else { Install::Manual(t!("update.manual_aur")) };
        }
        if ok("rpm", &["-qf", &path]).is_some() {
            return Install::Rpm;
        }
        return Install::Manual(t!("update.manual_unknown"));
    }
    // instalar.sh: <prefijo>/lib/form-pdf-reader/form-pdf-reader.
    let dir = exe.parent();
    if dir.and_then(|d| d.file_name()).is_some_and(|n| n == "form-pdf-reader")
        && dir.and_then(|d| d.parent()).and_then(|l| l.file_name()).is_some_and(|n| n == "lib")
        && let Some(prefix) = dir.and_then(|d| d.parent()).and_then(|l| l.parent())
    {
        return Install::Tarball { prefix: prefix.to_path_buf() };
    }
    Install::Manual(t!("update.manual_build"))
}

/// El fichero de la versión que sirve para esta instalación.
pub fn asset_for<'a>(kind: &Install, r: &'a Release) -> Option<&'a Asset> {
    let ends = match kind {
        Install::WindowsSetup { .. } => "-windows-x64-setup.exe",
        Install::Deb => "_amd64.deb",
        Install::Rpm => ".x86_64.rpm",
        Install::Arch => "-x86_64.pkg.tar.zst",
        Install::Tarball { .. } => "-x86_64.tar.gz",
        Install::Manual(_) => return None,
    };
    r.assets.iter().find(|a| a.name.ends_with(ends))
}

/// Borra los paquetes que dejaron actualizaciones anteriores en la carpeta
/// temporal (el instalador de Windows no se puede borrar mientras se ejecuta).
pub fn clean_old_downloads() {
    let mine = format!("form-pdf-reader-update-{}", std::process::id());
    let Ok(dir) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("form-pdf-reader-update-") && name != mine {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Descarga `a` en una carpeta temporal y comprueba su SHA-256. `done`
/// cuenta los bytes descargados (para la barra de progreso).
pub fn download(a: &Asset, done: Arc<AtomicU64>) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("form-pdf-reader-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(&a.name);
    let mut child = curl().args(["--max-time", "900", "--output"]).arg(&path).arg(&a.url).spawn().context(t!("update.no_curl"))?;
    loop {
        if let Some(st) = child.try_wait()? {
            if !st.success() {
                bail!("{}", t!("update.download_failed", st));
            }
            break;
        }
        done.store(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    let data = std::fs::read(&path)?;
    done.store(data.len() as u64, Ordering::Relaxed);
    let sum: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    match &a.sha256 {
        Some(want) if *want != sum => {
            let _ = std::fs::remove_file(&path);
            bail!("{}", t!("update.bad_checksum"));
        }
        None => log::warn!("{}: GitHub no da su SHA-256; no se comprueba", a.name),
        _ => {}
    }
    Ok(path)
}

/// Instala el paquete descargado. En Windows lanza el instalador y el
/// programa debe cerrarse (devuelve true); en Linux espera a que termine.
pub fn install(kind: &Install, file: &Path) -> Result<bool> {
    match kind {
        #[cfg(windows)]
        Install::WindowsSetup { all_users } => {
            use std::os::windows::process::CommandExt;
            // /UPDATE=1: el instalador vuelve a abrir el programa al terminar.
            Command::new(file)
                .args(["/SILENT", "/SP-", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/UPDATE=1"])
                .arg(if *all_users { "/ALLUSERS" } else { "/CURRENTUSER" })
                .creation_flags(0x0000_0008) // DETACHED_PROCESS
                .spawn()?;
            Ok(true)
        }
        Install::Deb => run_admin(&["dpkg", "-i"], file),
        Install::Rpm => run_admin(&["rpm", "-U", "--replacepkgs"], file),
        Install::Arch => run_admin(&["pacman", "-U", "--noconfirm"], file),
        Install::Tarball { prefix } => {
            let dir = file.parent().ok_or_else(|| anyhow!("sin carpeta"))?.join("tar");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;
            let st = Command::new("tar").arg("-xzf").arg(file).arg("-C").arg(&dir).status()?;
            if !st.success() {
                bail!("tar: {st}");
            }
            let script = std::fs::read_dir(&dir)?.flatten().map(|e| e.path().join("instalar.sh")).find(|p| p.exists()).ok_or_else(|| anyhow!("instalar.sh no está en el paquete"))?;
            let st = Command::new("bash").arg(&script).env("PREFIX", prefix).env("UPDATE", "1").stdin(std::process::Stdio::null()).status()?;
            if !st.success() {
                bail!("instalar.sh: {st}");
            }
            Ok(false)
        }
        _ => bail!("{}", t!("update.manual_unknown")),
    }
}

/// Una orden de instalación como administrador, con pkexec.
#[cfg(not(windows))]
fn run_admin(cmd: &[&str], file: &Path) -> Result<bool> {
    let st = Command::new("pkexec").args(cmd).arg(file).status().context(t!("update.no_pkexec", format!("sudo {} {}", cmd.join(" "), file.display())))?;
    match st.code() {
        Some(0) => Ok(false),
        // 126: se canceló la autenticación.
        Some(126) | Some(127) => bail!("{}", t!("update.cancelled")),
        _ => bail!("{}", t!("update.install_failed", st)),
    }
}

#[cfg(windows)]
fn run_admin(_cmd: &[&str], _file: &Path) -> Result<bool> {
    bail!("no disponible en Windows")
}

/// Vuelve a abrir el programa (tras instalar la versión nueva en Linux),
/// con el documento que estaba a la vista.
pub fn restart(file: Option<&Path>) {
    if let Ok(exe) = std::env::current_exe() {
        // El ejecutable se ha reemplazado: el nombre sigue apuntando al nuevo.
        let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
        let mut c = Command::new(exe);
        c.args(file).env_remove("PDFRE_AUTOTEST");
        if let Err(e) = c.spawn() {
            log::warn!("no se pudo volver a abrir el programa: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// La ventana
// ---------------------------------------------------------------------------

use eframe::egui::{self, RichText};
use std::sync::mpsc::{Receiver, channel};

use crate::theme;

enum State {
    Checking(Receiver<Result<Option<Release>>>),
    UpToDate,
    Available(Release),
    Downloading { rx: Receiver<Result<PathBuf>>, done: Arc<AtomicU64>, total: u64 },
    Installing(Receiver<Result<bool>>),
    /// Instalada (Linux): falta volver a abrir el programa.
    Installed,
    Failed(String),
}

/// Lo que pide la ventana a la aplicación.
pub enum Outcome {
    Closed,
    /// No volver a avisar de esta versión.
    Skip(String),
    /// Volver a abrir el programa con la versión nueva.
    Restart,
}

pub struct UpdateDialog {
    state: State,
    kind: Install,
    release: Option<Release>,
}

fn spawn<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx
}

/// Busca en segundo plano (al abrir el programa, sin ventana).
pub fn check_in_background() -> Receiver<Result<Option<Release>>> {
    spawn(check)
}

fn mb(n: u64) -> String {
    format!("{:.1} MB", n as f64 / 1_048_576.0)
}

impl UpdateDialog {
    /// Buscar ahora (desde el menú o los ajustes).
    pub fn checking() -> UpdateDialog {
        UpdateDialog { state: State::Checking(spawn(check)), kind: install_kind(), release: None }
    }

    /// Ya se sabe que hay una versión nueva.
    pub fn available(r: Release) -> UpdateDialog {
        UpdateDialog { state: State::Available(r.clone()), kind: install_kind(), release: Some(r) }
    }

    /// `dirty`: hay documentos con cambios sin guardar (no se puede cerrar).
    pub fn show(&mut self, ctx: &egui::Context, dirty: bool) -> Option<Outcome> {
        self.poll(ctx);
        let mut out = None;
        theme::modal(ctx, "actualizar").show(ctx, |ui| {
            let p = theme::palette(ui);
            ui.set_width(440.0);
            match &self.state {
                State::Checking(_) => {
                    ui.heading(t!("update.title"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t!("update.checking"));
                    });
                    if theme::dialog_footer(ui, &[(t!("dialog.cancel"), false, true)]).is_some() {
                        out = Some(Outcome::Closed);
                    }
                }
                State::UpToDate => {
                    ui.heading(t!("update.title"));
                    ui.add_space(8.0);
                    ui.label(t!("update.up_to_date", current_version()));
                    if theme::dialog_footer(ui, &[(t!("dialog.close"), true, true)]).is_some() {
                        out = Some(Outcome::Closed);
                    }
                }
                State::Available(r) => {
                    let r = r.clone();
                    ui.heading(t!("update.available"));
                    ui.add_space(6.0);
                    ui.label(t!("update.versions", r.version, current_version()));
                    if let Some(notes) = notes_for(&r.notes, form_pdf_reader::i18n::lang()) {
                        ui.add_space(8.0);
                        egui::Frame::new().stroke(egui::Stroke::new(1.0, p.border)).corner_radius(6).inner_margin(10).show(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(notes).size(13.0));
                            });
                        });
                    }
                    if !r.page.is_empty() {
                        ui.add_space(4.0);
                        ui.hyperlink_to(t!("update.release_page"), &r.page);
                    }
                    let auto = asset_for(&self.kind, &r).is_some();
                    ui.add_space(6.0);
                    match &self.kind {
                        Install::Manual(why) => {
                            ui.label(RichText::new(why).size(12.0).color(p.text_weak));
                        }
                        _ if !auto => {
                            ui.label(RichText::new(t!("update.no_asset")).size(12.0).color(p.text_weak));
                        }
                        Install::Deb | Install::Rpm | Install::Arch => {
                            ui.label(RichText::new(t!("update.admin_note")).size(12.0).color(p.text_weak));
                        }
                        _ => {}
                    }
                    if auto && dirty {
                        ui.label(RichText::new(t!("update.save_first")).size(12.0).color(p.warn));
                    }
                    let buttons = if auto {
                        vec![(t!("update.install"), true, !dirty), (t!("update.later"), false, true), (t!("update.skip"), false, true)]
                    } else {
                        vec![(t!("update.download"), true, !r.page.is_empty()), (t!("update.later"), false, true), (t!("update.skip"), false, true)]
                    };
                    match theme::dialog_footer(ui, &buttons) {
                        Some(0) if auto => self.start_download(&r),
                        Some(0) => {
                            let _ = crate::platform::open_external(&r.page);
                            out = Some(Outcome::Closed);
                        }
                        Some(1) => out = Some(Outcome::Closed),
                        Some(_) => out = Some(Outcome::Skip(r.version.clone())),
                        None => {}
                    }
                }
                State::Downloading { done, total, .. } => {
                    ui.heading(t!("update.title"));
                    ui.add_space(8.0);
                    let n = done.load(Ordering::Relaxed);
                    let frac = if *total > 0 { n as f32 / *total as f32 } else { 0.0 };
                    ui.label(t!("update.downloading", mb(n), mb(*total)));
                    ui.add_space(4.0);
                    ui.add(egui::ProgressBar::new(frac.min(1.0)).show_percentage());
                    ui.add_space(14.0);
                }
                State::Installing(_) => {
                    ui.heading(t!("update.title"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t!("update.installing"));
                    });
                    ui.add_space(14.0);
                }
                State::Installed => {
                    ui.heading(t!("update.title"));
                    ui.add_space(8.0);
                    let v = self.release.as_ref().map(|r| r.version.clone()).unwrap_or_default();
                    ui.label(t!("update.installed", v));
                    if dirty {
                        ui.label(RichText::new(t!("update.save_first")).size(12.0).color(p.warn));
                    }
                    match theme::dialog_footer(ui, &[(t!("update.restart"), true, !dirty), (t!("update.later"), false, true)]) {
                        Some(0) => out = Some(Outcome::Restart),
                        Some(_) => out = Some(Outcome::Closed),
                        None => {}
                    }
                }
                State::Failed(e) => {
                    ui.heading(t!("update.title"));
                    ui.add_space(8.0);
                    ui.label(t!("update.failed", e));
                    let page = self.release.as_ref().map(|r| r.page.clone()).unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest"));
                    match theme::dialog_footer(ui, &[(t!("update.download"), false, true), (t!("dialog.close"), true, true)]) {
                        Some(0) => {
                            let _ = crate::platform::open_external(&page);
                            out = Some(Outcome::Closed);
                        }
                        Some(_) => out = Some(Outcome::Closed),
                        None => {}
                    }
                }
            }
        });
        out
    }

    fn start_download(&mut self, r: &Release) {
        let Some(a) = asset_for(&self.kind, r).cloned() else { return };
        let done = Arc::new(AtomicU64::new(0));
        let d = done.clone();
        let total = a.size;
        self.state = State::Downloading { rx: spawn(move || download(&a, d)), done, total };
    }

    /// Avanza cuando terminan las tareas en segundo plano.
    fn poll(&mut self, ctx: &egui::Context) {
        use std::sync::mpsc::TryRecvError;
        let next = match &self.state {
            State::Checking(rx) => match rx.try_recv() {
                Ok(Ok(Some(r))) => {
                    self.release = Some(r.clone());
                    Some(State::Available(r))
                }
                Ok(Ok(None)) => Some(State::UpToDate),
                Ok(Err(e)) => Some(State::Failed(e.to_string())),
                Err(TryRecvError::Disconnected) => Some(State::Failed(String::new())),
                Err(TryRecvError::Empty) => None,
            },
            State::Downloading { rx, .. } => match rx.try_recv() {
                Ok(Ok(file)) => {
                    let kind = self.kind.clone();
                    Some(State::Installing(spawn(move || install(&kind, &file))))
                }
                Ok(Err(e)) => Some(State::Failed(e.to_string())),
                Err(TryRecvError::Disconnected) => Some(State::Failed(String::new())),
                Err(TryRecvError::Empty) => None,
            },
            State::Installing(rx) => match rx.try_recv() {
                Ok(Ok(_)) => Some(State::Installed),
                Ok(Err(e)) => Some(State::Failed(e.to_string())),
                Err(TryRecvError::Disconnected) => Some(State::Failed(String::new())),
                Err(TryRecvError::Empty) => None,
            },
            _ => None,
        };
        if let Some(s) = next {
            self.state = s;
        }
        if matches!(self.state, State::Checking(_) | State::Downloading { .. } | State::Installing(_)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }

    /// El instalador de Windows ya está en marcha: hay que cerrar.
    pub fn must_quit(&self) -> bool {
        cfg!(windows) && matches!(self.state, State::Installed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versiones() {
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("v1.0.0", "0.9.9"));
        assert!(!newer("0.9.0", "0.9.0"));
        assert!(!newer("0.8.5", "0.9.0"));
    }

    #[test]
    fn novedades() {
        let md = "## What's Changed\n* Firma **digital** by @blzkz in https://github.com/blzkz/form-pdf-reader/pull/33\n\n**Full Changelog**: https://x";
        assert_eq!(plain_notes(md), "•  Firma digital (#33)");
        // Sin secciones por idioma: solo en español.
        assert_eq!(notes_for(md, "es").as_deref(), Some("•  Firma digital (#33)"));
        assert_eq!(notes_for(md, "en"), None);
        let md = "## Español\n<!-- lang:es -->\n- Uno\n\n## English\n<!-- lang:en -->\n## English\n- One\n<!-- end -->\n## What's Changed\n* x by @a in https://g/pull/1";
        assert_eq!(notes_for(md, "es").as_deref(), Some("•  Uno"));
        assert_eq!(notes_for(md, "en").as_deref(), Some("•  One"));
        assert_eq!(notes_for(md, "de").as_deref(), Some("•  One"));
    }
}
