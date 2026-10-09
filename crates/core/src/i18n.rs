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
//! inglés y, si tampoco está, se muestra la propia clave.
//!
//! Para añadir un idioma: crear `locales/<código>.txt` y añadirlo a
//! `CATALOGS`.

use std::collections::HashMap;
use std::sync::OnceLock;

/// (código de idioma, contenido del catálogo). El primero es el de reserva.
const CATALOGS: &[(&str, &str)] = &[("en", include_str!("../locales/en.txt")), ("es", include_str!("../locales/es.txt"))];

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
    raw.split(['_', '.', '@', '-']).next().unwrap_or("").to_ascii_lowercase()
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let want = requested();
        let (lang, src) = CATALOGS.iter().find(|(c, _)| *c == want).copied().unwrap_or(CATALOGS[0]);
        Tables { lang, current: parse(src), fallback: parse(CATALOGS[0].1) }
    })
}

/// Idioma de la interfaz ("en", "es"…).
pub fn lang() -> &'static str {
    tables().lang
}

/// Texto de una clave.
pub fn tr(key: &str) -> String {
    let t = tables();
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
