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
