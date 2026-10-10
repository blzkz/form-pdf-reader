//! Textos de la interfaz en varios idiomas.
//!
//! Los catálogos están en `locales/<idioma>.txt`, con una línea por texto:
//! `clave = texto`. `{0}`, `{1}`… se sustituyen por los argumentos, y `\n`
//! es un salto de línea. Las líneas vacías y las que empiezan por `#` se
//! ignoran.
//!
//! El idioma sale de `FORM_PDF_READER_LANG` o, si no está, del idioma del
//! sistema (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`, como gettext). Si no hay catálogo para ese
//! idioma se usa el inglés. Una clave que falte en un catálogo se busca en el
//! inglés y, si tampoco está, se muestra la propia clave. La aplicación puede
//! cambiarlo en cualquier momento con [`set_language`] (el ajuste de idioma
//! del escritorio, el idioma de Android).
//!
//! Para añadir un idioma: crear `locales/<código>.txt` y añadirlo a
//! `CATALOGS` y a [`LANGUAGES`].

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

/// (código de idioma, contenido del catálogo). El primero es el de reserva.
const CATALOGS: &[(&str, &str)] = &[
    ("en", include_str!("../locales/en.txt")),
    ("es", include_str!("../locales/es.txt")),
    ("fr", include_str!("../locales/fr.txt")),
    ("it", include_str!("../locales/it.txt")),
    ("pt", include_str!("../locales/pt.txt")),
    ("de", include_str!("../locales/de.txt")),
];

/// Idiomas disponibles: (código, nombre en ese idioma), para elegirlos.
pub const LANGUAGES: &[(&str, &str)] =
    &[("en", "English"), ("es", "Español"), ("fr", "Français"), ("it", "Italiano"), ("pt", "Português"), ("de", "Deutsch")];

struct Tables {
    lang: &'static str,
    current: HashMap<&'static str, String>,
    fallback: HashMap<&'static str, String>,
}

fn parse(src: &'static str) -> HashMap<&'static str, String> {
    let mut m = HashMap::new();
    for line in src.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim(), v.trim().replace("\\n", "\n"));
        }
    }
    m
}

/// Código de idioma de "es_ES.UTF-8", "pt-BR", "de"…
fn base(code: &str) -> String {
    code.split(['_', '.', '@', '-']).next().unwrap_or("").to_ascii_lowercase()
}

static T: LazyLock<RwLock<Tables>> = LazyLock::new(|| RwLock::new(load(&requested())));

fn load(want: &str) -> Tables {
    let (lang, src) = CATALOGS.iter().find(|(c, _)| *c == want).copied().unwrap_or(CATALOGS[0]);
    Tables { lang, current: parse(src), fallback: parse(CATALOGS[0].1) }
}

/// Cambia el idioma de la interfaz (desde los ajustes, o en Android, que no
/// usa las variables de entorno). Un idioma sin catálogo usa el inglés.
/// Devuelve si el idioma tiene catálogo.
pub fn set_language(code: &str) -> bool {
    let want = base(code);
    *T.write().unwrap_or_else(|e| e.into_inner()) = load(&want);
    CATALOGS.iter().any(|(c, _)| *c == want)
}

/// Vuelve al idioma del sistema (el del entorno).
pub fn use_system_language() {
    *T.write().unwrap_or_else(|e| e.into_inner()) = load(&requested());
}

/// Idioma del sistema con catálogo, o "en".
pub fn system_language() -> &'static str {
    load(&requested()).lang
}

/// Código de idioma pedido por el entorno (p. ej. "es" de "es_ES.UTF-8").
fn requested() -> String {
    // Mismo orden que gettext: LANGUAGE (lista "es:en"), LC_ALL,
    // LC_MESSAGES, LANG. FORM_PDF_READER_LANG manda sobre todos.
    let raw = ["FORM_PDF_READER_LANG", "LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .map(|v| v.split(':').next().unwrap_or("").to_string())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .unwrap_or_default();
    base(&raw)
}

/// Idioma de la interfaz ("en", "es"…).
pub fn lang() -> &'static str {
    T.read().unwrap_or_else(|e| e.into_inner()).lang
}

/// Texto de una clave.
pub fn tr(key: &str) -> String {
    let t = T.read().unwrap_or_else(|e| e.into_inner());
    t.current.get(key).or_else(|| t.fallback.get(key)).cloned().unwrap_or_else(|| key.to_string())
}

/// Texto de una clave con argumentos `{0}`, `{1}`…
pub fn trf(key: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut s = tr(key);
    for (i, a) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), &a.to_string());
    }
    s
}

/// `t!("clave")` o `t!("clave", arg0, arg1…)`.
#[macro_export]
macro_rules! t {
    ($key:expr) => {
        $crate::i18n::tr($key)
    };
    ($key:expr, $($arg:expr),+ $(,)?) => {
        $crate::i18n::trf($key, &[$(&$arg as &dyn std::fmt::Display),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cambiar_idioma() {
        assert!(set_language("fr_FR.UTF-8"));
        assert_eq!(lang(), "fr");
        assert_eq!(tr("dialog.cancel"), "Annuler");
        assert!(set_language("pt-BR"));
        assert_eq!(lang(), "pt");
        // Sin catálogo: inglés.
        assert!(!set_language("ja"));
        assert_eq!(lang(), "en");
        assert_eq!(tr("dialog.cancel"), "Cancel");
        use_system_language();
    }

    #[test]
    fn catalogos_completos() {
        // Todos los idiomas tienen las mismas claves que el inglés.
        let en = parse(CATALOGS[0].1);
        for (code, src) in CATALOGS {
            let m = parse(src);
            let mut faltan: Vec<_> = en.keys().filter(|k| !m.contains_key(*k)).collect();
            let mut sobran: Vec<_> = m.keys().filter(|k| !en.contains_key(*k)).collect();
            faltan.sort();
            sobran.sort();
            assert!(faltan.is_empty() && sobran.is_empty(), "{code}: faltan {faltan:?}, sobran {sobran:?}");
            assert!(LANGUAGES.iter().any(|(c, _)| c == code), "{code} no está en LANGUAGES");
            // Mismos marcadores {n} en cada texto.
            for (k, v) in &m {
                for i in 0..5 {
                    let p = format!("{{{i}}}");
                    assert_eq!(v.contains(&p), en[k].contains(&p), "{code}: {k} difiere en {p}");
                }
            }
        }
    }
}
