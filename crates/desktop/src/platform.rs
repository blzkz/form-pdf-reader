//! Lo que cambia entre Linux y Windows: abrir ficheros y enlaces con la
//! aplicación del sistema, preguntar un texto (app.response de los
//! formularios) y la carpeta de los ajustes.

use std::path::PathBuf;

/// Abre un fichero o un enlace (http, https, mailto) con la aplicación del
/// sistema.
pub fn open_external(target: &str) -> std::io::Result<()> {
    imp::open_external(target)
}

/// Pregunta un texto al usuario con un diálogo del sistema. None si cancela
/// o no se puede mostrar.
pub fn ask_text(title: &str, question: &str, default: &str) -> Option<String> {
    imp::ask_text(title, question, default)
}

/// Carpeta de los ajustes de la aplicación.
pub fn config_dir() -> Option<PathBuf> {
    imp::config_dir().map(|d| d.join("form-pdf-reader"))
}

/// Si esta aplicación es la que abre los PDF (None: no se puede saber, p. ej.
/// porque no está instalada).
pub fn is_pdf_default() -> Option<bool> {
    imp::is_pdf_default()
}

/// Hace que esta aplicación abra los PDF. En Windows no se puede sin que
/// el usuario lo confirme: se abre la configuración de aplicaciones
/// predeterminadas (devuelve false: queda en manos del usuario).
pub fn make_pdf_default() -> std::io::Result<bool> {
    imp::make_pdf_default()
}

/// false si solo hay adaptadores gráficos por software (máquinas virtuales y
/// escritorios remotos sin tarjeta gráfica, con el "Microsoft Basic Render
/// Driver" de Direct3D). En Linux se deja a OpenGL (Mesa).
pub fn has_gpu() -> bool {
    #[cfg(windows)]
    {
        use eframe::wgpu;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::DX12, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
        for a in &adapters {
            let info = a.get_info();
            log::info!("Adaptador gráfico: {} ({:?})", info.name, info.device_type);
        }
        adapters.iter().any(|a| a.get_info().device_type != wgpu::DeviceType::Cpu)
    }
    #[cfg(not(windows))]
    true
}

/// En Windows el ejecutable no abre consola; las órdenes de línea de
/// comandos escriben en la de quien lo lanzó.
pub fn attach_console() {
    #[cfg(windows)]
    imp::attach_console();
}

#[cfg(not(windows))]
mod imp {
    use std::path::PathBuf;

    pub fn open_external(target: &str) -> std::io::Result<()> {
        std::process::Command::new("xdg-open").arg(target).spawn().map(|_| ())
    }

    // zenity: los diálogos de rfd no tienen campo de texto.
    pub fn ask_text(title: &str, question: &str, default: &str) -> Option<String> {
        let out = std::process::Command::new("zenity")
            .arg("--entry")
            .arg(format!("--title={title}"))
            .arg(format!("--text={question}"))
            .arg(format!("--entry-text={default}"))
            .output()
            .ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())
    }

    const DESKTOP: &str = "form-pdf-reader.desktop";

    /// El lanzador está instalado (en alguna carpeta de aplicaciones).
    fn desktop_installed() -> bool {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let data_home = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/share")));
        let dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
        data_home.into_iter().chain(dirs.split(':').map(PathBuf::from)).any(|d| d.join("applications").join(DESKTOP).exists())
    }

    pub fn is_pdf_default() -> Option<bool> {
        if !desktop_installed() {
            return None;
        }
        let out = std::process::Command::new("xdg-mime").args(["query", "default", "application/pdf"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim() == DESKTOP)
    }

    pub fn make_pdf_default() -> std::io::Result<bool> {
        let st = std::process::Command::new("xdg-mime").args(["default", DESKTOP, "application/pdf"]).status()?;
        if st.success() { Ok(true) } else { Err(std::io::Error::other(format!("xdg-mime: {st}"))) }
    }

    pub fn config_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(hwnd: *mut c_void, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> *mut c_void;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(pid: u32) -> i32;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(key: isize, subkey: *const u16, value: *const u16, flags: u32, kind: *mut u32, data: *mut c_void, len: *mut u32) -> i32;
    }

    const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002_u32 as i32 as isize;

    /// Un valor de texto del registro.
    fn reg_string(root: isize, key: &str, value: &str) -> Option<String> {
        const RRF_RT_REG_SZ: u32 = 0x2;
        let (k, v) = (wide(key), wide(value));
        let mut buf = vec![0u16; 512];
        let mut len = (buf.len() * 2) as u32;
        let r = unsafe { RegGetValueW(root, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
        if r != 0 {
            return None;
        }
        buf.truncate((len as usize / 2).saturating_sub(1));
        Some(String::from_utf16_lossy(&buf))
    }

    /// El nombre con que lo registra el instalador en RegisteredApplications.
    const REGISTERED: &str = "FormPdfReader";

    pub fn is_pdf_default() -> Option<bool> {
        let id = reg_string(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf\UserChoice", "ProgId");
        Some(matches!(id.as_deref(), Some("FormPdfReader.pdf") | Some(r"Applications\form-pdf-reader.exe")))
    }

    pub fn make_pdf_default() -> std::io::Result<bool> {
        // La página de esta aplicación en "Aplicaciones predeterminadas"
        // (Windows 11), si el instalador la registró; si no, la general.
        let key = r"Software\RegisteredApplications";
        let uri = if reg_string(HKEY_CURRENT_USER, key, REGISTERED).is_some() {
            format!("ms-settings:defaultapps?registeredAppUser={REGISTERED}")
        } else if reg_string(HKEY_LOCAL_MACHINE, key, REGISTERED).is_some() {
            format!("ms-settings:defaultapps?registeredAppMachine={REGISTERED}")
        } else {
            "ms-settings:defaultapps".to_string()
        };
        open_external(&uri).map(|_| false)
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn open_external(target: &str) -> std::io::Result<()> {
        let (op, file) = (wide("open"), wide(target));
        const SW_SHOWNORMAL: i32 = 1;
        let r = unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
        // Devuelve un valor mayor que 32 si funcionó.
        if r as usize > 32 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }

    // El cuadro InputBox de Visual Basic, desde PowerShell. Los textos van en
    // variables de entorno para no tener que escaparlos.
    pub fn ask_text(title: &str, question: &str, default: &str) -> Option<String> {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Add-Type -AssemblyName Microsoft.VisualBasic; \
                      [Microsoft.VisualBasic.Interaction]::InputBox($env:PDFRE_Q, $env:PDFRE_T, $env:PDFRE_D)";
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("PDFRE_Q", question)
            .env("PDFRE_T", title)
            .env("PDFRE_D", default)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']).to_string())
    }

    pub fn config_dir() -> Option<PathBuf> {
        std::env::var_os("APPDATA").filter(|v| !v.is_empty()).map(PathBuf::from)
    }

    pub fn attach_console() {
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}
