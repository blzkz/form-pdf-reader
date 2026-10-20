//! Ajustes del escritorio que se recuerdan entre sesiones, en
//! `$XDG_CONFIG_HOME/form-pdf-reader/settings.conf` (o `~/.config/…`; en
//! Windows, `%APPDATA%\form-pdf-reader\settings.conf`), con una línea
//! `clave = valor` por ajuste.

use std::path::PathBuf;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Settings {
    /// Idioma elegido ("fr"…); None: el del sistema.
    pub language: Option<String>,
    /// Tema: "light", "dark"; None: el del sistema.
    pub theme: Option<String>,
    /// Nombre con que se crearon firmas (se propone para la siguiente).
    pub signer_name: Option<String>,
    /// La última impresora usada.
    pub printer: Option<String>,
    /// Firma digital: pedir sello de tiempo, y a qué servidor.
    pub timestamp: bool,
    pub timestamp_server: Option<String>,
    /// No buscar versiones nuevas al abrir.
    pub updates_off: bool,
    /// Cuándo se buscaron por última vez (segundos desde 1970).
    pub update_checked: u64,
    /// Versión nueva por la que no hay que volver a avisar.
    pub update_skipped: Option<String>,
    /// Ya se preguntó si abrir los PDF con esta aplicación.
    pub pdf_default_asked: bool,
}

fn path() -> Option<PathBuf> {
    Some(crate::platform::config_dir()?.join("settings.conf"))
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
                match k.trim() {
                    "language" if !v.is_empty() => s.language = Some(v.to_string()),
                    "theme" if v == "light" || v == "dark" => s.theme = Some(v.to_string()),
                    "signer_name" if !v.is_empty() => s.signer_name = Some(v.to_string()),
                    "printer" if !v.is_empty() => s.printer = Some(v.to_string()),
                    "timestamp" => s.timestamp = v == "true",
                    "timestamp_server" if !v.is_empty() => s.timestamp_server = Some(v.to_string()),
                    "update_check" => s.updates_off = v == "false",
                    "update_checked" => s.update_checked = v.parse().unwrap_or(0),
                    "update_skipped" if !v.is_empty() => s.update_skipped = Some(v.to_string()),
                    "pdf_default_asked" => s.pdf_default_asked = v == "true",
                    _ => {}
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
        if let Some(t) = &self.theme {
            text.push_str(&format!("theme = {t}\n"));
        }
        if let Some(n) = &self.signer_name {
            text.push_str(&format!("signer_name = {n}\n"));
        }
        if let Some(n) = &self.printer {
            text.push_str(&format!("printer = {n}\n"));
        }
        if self.timestamp {
            text.push_str("timestamp = true\n");
        }
        if let Some(n) = &self.timestamp_server {
            text.push_str(&format!("timestamp_server = {n}\n"));
        }
        if self.updates_off {
            text.push_str("update_check = false\n");
        }
        if self.update_checked > 0 {
            text.push_str(&format!("update_checked = {}\n", self.update_checked));
        }
        if let Some(v) = &self.update_skipped {
            text.push_str(&format!("update_skipped = {v}\n"));
        }
        if self.pdf_default_asked {
            text.push_str("pdf_default_asked = true\n");
        }
        let ok = p.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&p, text).is_ok();
        if !ok {
            log::warn!("no se pudieron guardar los ajustes en {}", p.display());
        }
    }

    /// Aplica el tema: el elegido, o el del sistema.
    pub fn apply_theme(&self, ctx: &eframe::egui::Context) {
        use eframe::egui::ThemePreference;
        ctx.set_theme(match self.theme.as_deref() {
            Some("light") => ThemePreference::Light,
            Some("dark") => ThemePreference::Dark,
            _ => ThemePreference::System,
        });
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
        assert_eq!(Settings::parse("theme = dark\n").theme.as_deref(), Some("dark"));
        assert_eq!(Settings::parse("theme = azul\n").theme, None);
        assert!(Settings::parse("update_check = false\n").updates_off);
        assert!(!Settings::parse("").updates_off);
        assert_eq!(Settings::parse("update_checked = 1760000000\n").update_checked, 1760000000);
    }
}
