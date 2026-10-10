//! Ajustes del escritorio que se recuerdan entre sesiones, en
//! `$XDG_CONFIG_HOME/form-pdf-reader/settings.conf` (o `~/.config/…`), con
//! una línea `clave = valor` por ajuste.

use std::path::PathBuf;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Settings {
    /// Idioma elegido ("fr"…); None: el del sistema.
    pub language: Option<String>,
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("form-pdf-reader").join("settings.conf"))
}

impl Settings {
    pub fn load() -> Settings {
        let text = path().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        Settings::parse(&text)
    }

    fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                if k.trim() == "language" && !v.is_empty() {
                    s.language = Some(v.to_string());
                }
            }
        }
        s
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        let mut text = String::new();
        if let Some(l) = &self.language {
            text.push_str(&format!("language = {l}\n"));
        }
        let ok = p.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&p, text).is_ok();
        if !ok {
            log::warn!("no se pudieron guardar los ajustes en {}", p.display());
        }
    }

    /// Aplica el idioma: el elegido, o el del sistema. La variable
    /// FORM_PDF_READER_LANG manda sobre el ajuste.
    pub fn apply_language(&self) {
        if std::env::var_os("FORM_PDF_READER_LANG").is_some_and(|v| !v.is_empty()) {
            form_pdf_reader::i18n::use_system_language();
            return;
        }
        match &self.language {
            Some(l) => {
                form_pdf_reader::i18n::set_language(l);
            }
            None => form_pdf_reader::i18n::use_system_language(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leer_ajustes() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("# x\nlanguage = de\n").language.as_deref(), Some("de"));
        assert_eq!(Settings::parse("language =\n").language, None);
    }
}
