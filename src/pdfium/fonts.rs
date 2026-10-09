//! Sustitución de fuentes.
//!
//! Muchos formularios XFA (por ejemplo los del Banco de España) usan fuentes
//! corporativas que no van incrustadas en el PDF. PDFium busca un sustituto en
//! el sistema y, si elige mal, faltan glifos. Aquí envolvemos el
//! FPDF_SYSFONTINFO por defecto de PDFium y reescribimos los nombres de fuente
//! conocidos por equivalentes métricamente compatibles instalados en Linux.

use super::sys;
use std::ffi::{CStr, CString, c_char, c_int, c_uchar, c_uint, c_ulong, c_void};

#[repr(C)]
struct FontInfoWrapper {
    base: sys::FPDF_SYSFONTINFO,
    inner: *mut sys::FPDF_SYSFONTINFO,
    debug: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Sans,
    Serif,
    Mono,
}

/// Fuentes de Windows/Adobe habituales en formularios y que no existen en
/// Linux. Se registran como "instaladas" durante la enumeración para que el
/// motor XFA (que elige fuente por nombre entre las enumeradas) las encuentre,
/// y después se redirigen a una equivalente métrica real.
const ALIASES: &[(&str, Kind)] = &[
    ("BdE Neue Helvetica 45 Light", Kind::Sans),
    ("BdE Neue Helvetica 55 Roman", Kind::Sans),
    ("BdE Neue Helvetica 47 LightCn", Kind::Sans),
    ("BdE Neue Helvetica 65 Medium", Kind::Sans),
    ("BdE Neue Helvetica 75 Bold", Kind::Sans),
    ("Arial", Kind::Sans),
    ("Helvetica", Kind::Sans),
    ("Myriad Pro", Kind::Sans),
    ("Calibri", Kind::Sans),
    ("Segoe UI", Kind::Sans),
    ("Tahoma", Kind::Sans),
    ("Verdana", Kind::Sans),
    ("Times New Roman", Kind::Serif),
    ("Courier New", Kind::Mono),
];

const SANS: &[&str] = &["Liberation Sans", "Arimo", "Nimbus Sans", "DejaVu Sans"];
const SERIF: &[&str] = &["Liberation Serif", "Tinos", "Nimbus Roman", "DejaVu Serif"];
const MONO: &[&str] = &["Liberation Mono", "Cousine", "Nimbus Mono PS", "DejaVu Sans Mono"];

fn classify(face: &str) -> Option<Kind> {
    let f = face.to_ascii_lowercase();
    if let Some((_, k)) = ALIASES.iter().find(|(a, _)| a.eq_ignore_ascii_case(face)) {
        return Some(*k);
    }
    if f.contains("courier") {
        Some(Kind::Mono)
    } else if f.contains("times") || f.contains("georgia") || f.contains("garamond") {
        Some(Kind::Serif)
    } else if f.contains("helvetica")
        || f.contains("myriad")
        || f.contains("segoe")
        || f.contains("calibri")
        || f.starts_with("arial")
        || f.contains("frutiger")
        || f.contains("univers")
    {
        Some(Kind::Sans)
    } else {
        None
    }
}

/// Devuelve el nombre de fuente instalada que debe usarse en lugar de `face`.
pub fn substitute(face: &str) -> Option<&'static str> {
    let k = classify(face)?;
    let pick = installed(k)?;
    // No redirigir una fuente a sí misma.
    if pick.eq_ignore_ascii_case(face) { None } else { Some(pick) }
}

fn installed(k: Kind) -> Option<&'static str> {
    use std::sync::OnceLock;
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    let fams = FAMILIES.get_or_init(|| {
        std::process::Command::new("fc-list")
            .arg(":")
            .arg("family")
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .flat_map(|l| l.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
                    .collect()
            })
            .unwrap_or_default()
    });
    let list = match k {
        Kind::Sans => SANS,
        Kind::Serif => SERIF,
        Kind::Mono => MONO,
    };
    list.iter().copied().find(|name| fams.iter().any(|f| f == name))
}

fn alias_needed(alias: &str) -> bool {
    // Solo se registra el alias si la fuente real no está instalada.
    let k = classify(alias);
    k.is_some() && substitute(alias).is_some()
}

unsafe fn inner(p: *mut sys::FPDF_SYSFONTINFO) -> (&'static FontInfoWrapper, &'static sys::FPDF_SYSFONTINFO) {
    unsafe {
        let w = &*(p as *const FontInfoWrapper);
        (w, &*w.inner)
    }
}

unsafe extern "C" fn release(_p: *mut sys::FPDF_SYSFONTINFO) {}

unsafe extern "C" fn enum_fonts(p: *mut sys::FPDF_SYSFONTINFO, mapper: *mut c_void) {
    unsafe {
        let (w, i) = inner(p);
        if let Some(f) = i.EnumFonts {
            f(w.inner, mapper)
        }
        for (alias, _) in ALIASES {
            if alias_needed(alias) {
                let c = CString::new(*alias).unwrap();
                sys::FPDF_AddInstalledFont(mapper, c.as_ptr(), sys::FXFONT_ANSI_CHARSET as c_int);
                if w.debug {
                    eprintln!("[fuentes] alias registrado '{alias}' -> {:?}", substitute(alias));
                }
            }
        }
    }
}

unsafe extern "C" fn map_font(
    p: *mut sys::FPDF_SYSFONTINFO,
    weight: c_int,
    italic: sys::FPDF_BOOL,
    charset: c_int,
    pitch: c_int,
    face: *const c_char,
    exact: *mut sys::FPDF_BOOL,
) -> *mut c_void {
    unsafe {
        let (w, i) = inner(p);
        let Some(f) = i.MapFont else { return std::ptr::null_mut() };
        let name = if face.is_null() { String::new() } else { CStr::from_ptr(face).to_string_lossy().into_owned() };
        let sub = substitute(&name);
        let r = match sub {
            Some(s) => {
                let c = CString::new(s).unwrap();
                f(w.inner, weight, italic, charset, pitch, c.as_ptr(), exact)
            }
            None => f(w.inner, weight, italic, charset, pitch, face, exact),
        };
        if w.debug {
            eprintln!("[fuentes] MapFont '{name}' w={weight} it={italic} cs={charset} -> {:?} ({})", sub, if r.is_null() { "no" } else { "ok" });
        }
        r
    }
}

unsafe extern "C" fn get_font(p: *mut sys::FPDF_SYSFONTINFO, face: *const c_char) -> *mut c_void {
    unsafe {
        let (w, i) = inner(p);
        let Some(f) = i.GetFont else { return std::ptr::null_mut() };
        let name = if face.is_null() { String::new() } else { CStr::from_ptr(face).to_string_lossy().into_owned() };
        let r = match substitute(&name) {
            Some(s) => {
                let c = CString::new(s).unwrap();
                f(w.inner, c.as_ptr())
            }
            None => f(w.inner, face),
        };
        if w.debug {
            eprintln!("[fuentes] GetFont '{name}' -> {}", if r.is_null() { "no" } else { "ok" });
        }
        r
    }
}

unsafe extern "C" fn get_font_data(
    p: *mut sys::FPDF_SYSFONTINFO,
    font: *mut c_void,
    table: c_uint,
    buf: *mut c_uchar,
    size: c_ulong,
) -> c_ulong {
    unsafe {
        let (w, i) = inner(p);
        i.GetFontData.map(|f| f(w.inner, font, table, buf, size)).unwrap_or(0)
    }
}

unsafe extern "C" fn get_face_name(p: *mut sys::FPDF_SYSFONTINFO, font: *mut c_void, buf: *mut c_char, size: c_ulong) -> c_ulong {
    unsafe {
        let (w, i) = inner(p);
        i.GetFaceName.map(|f| f(w.inner, font, buf, size)).unwrap_or(0)
    }
}

unsafe extern "C" fn get_font_charset(p: *mut sys::FPDF_SYSFONTINFO, font: *mut c_void) -> c_int {
    unsafe {
        let (w, i) = inner(p);
        i.GetFontCharset.map(|f| f(w.inner, font)).unwrap_or(0)
    }
}

unsafe extern "C" fn delete_font(p: *mut sys::FPDF_SYSFONTINFO, font: *mut c_void) {
    unsafe {
        let (w, i) = inner(p);
        if let Some(f) = i.DeleteFont {
            f(w.inner, font)
        }
    }
}

/// Instala el envoltorio. Debe llamarse justo después de FPDF_InitLibrary.
pub fn install() {
    unsafe {
        let inner = sys::FPDF_GetDefaultSystemFontInfo();
        if inner.is_null() {
            log::warn!("PDFium no ofrece FPDF_SYSFONTINFO por defecto; sin sustitución de fuentes");
            return;
        }
        let base = sys::FPDF_SYSFONTINFO {
            version: 1,
            Release: Some(release),
            EnumFonts: Some(enum_fonts),
            MapFont: Some(map_font),
            GetFont: Some(get_font),
            GetFontData: Some(get_font_data),
            GetFaceName: Some(get_face_name),
            GetFontCharset: Some(get_font_charset),
            DeleteFont: Some(delete_font),
        };
        let wrapper = Box::new(FontInfoWrapper { base, inner, debug: std::env::var_os("PDF_FONT_DEBUG").is_some() });
        // Vive durante todo el proceso: PDFium guarda el puntero.
        let ptr = Box::into_raw(wrapper);
        sys::FPDF_SetSystemFontInfo(ptr as *mut sys::FPDF_SYSFONTINFO);
    }
}
