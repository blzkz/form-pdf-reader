//! Impresión en Linux con CUPS, con sus órdenes (`lpstat`, `lpoptions`,
//! `lp`): las tiene cualquier sistema con CUPS y no hace falta enlazar con
//! libcups. Las hojas se juntan en un PDF plano que se manda con `lp`; los
//! filtros de CUPS lo pasan al idioma de la impresora.

use std::path::PathBuf;
use std::process::Command;

use form_pdf_reader::pdfium::{Document, FlatPdf, Sheet};
use form_pdf_reader::t;

use super::{Duplex, Options, PRINT_DPI, Printer};

/// CUPS no tiene un diálogo de propiedades propio.
pub const HAS_PROPERTIES: bool = false;

/// Bits de `printer-type` (cups.h).
const CUPS_PRINTER_COLOR: u32 = 0x0004;
const CUPS_PRINTER_DUPLEX: u32 = 0x0008;

/// Una orden de CUPS en inglés (para leer su salida).
fn cups(cmd: &str, args: &[&str]) -> std::io::Result<String> {
    let out = Command::new(cmd).args(args).env("LC_ALL", "C").env("LANG", "C").output()?;
    if !out.status.success() && out.stdout.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(std::io::Error::other(if err.is_empty() { format!("{cmd}: {}", out.status) } else { err }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn list_printers() -> Result<Vec<Printer>, String> {
    let names = match cups("lpstat", &["-e"]) {
        Ok(s) => s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect::<Vec<_>>(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(t!("print.no_cups")),
        // Sin impresoras, lpstat -e acaba con error y sin salida.
        Err(_) => Vec::new(),
    };
    let default = default_printer();
    let mut list: Vec<Printer> = names
        .into_iter()
        .map(|name| {
            let attrs = cups("lpoptions", &["-p", &name]).map(|s| parse_options(&s)).unwrap_or_default();
            let get = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
            let ty = get("printer-type").and_then(|v| v.parse::<u32>().ok());
            let ppd = cups("lpoptions", &["-p", &name, "-l"]).unwrap_or_default();
            // Doble cara: que CUPS lo diga y que el controlador tenga la opción.
            let duplex_option = ppd.lines().any(|l| l.starts_with("Duplex/") || l.starts_with("sides/"));
            let duplex = ty.is_none_or(|t| t & CUPS_PRINTER_DUPLEX != 0) && duplex_option;
            let color = ty.is_none_or(|t| t & CUPS_PRINTER_COLOR != 0);
            let label = get("printer-info").filter(|s| !s.trim().is_empty()).map(str::to_string).unwrap_or_else(|| name.replace('_', " "));
            Printer { is_default: default.as_deref() == Some(name.as_str()), label, name, color, duplex }
        })
        .collect();
    list.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.label.to_lowercase().cmp(&b.label.to_lowercase())));
    Ok(list)
}

/// La impresora predeterminada: la del usuario ($PRINTER, $LPDEST) o la del
/// sistema.
fn default_printer() -> Option<String> {
    for var in ["LPDEST", "PRINTER"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            return Some(v.trim().to_string());
        }
    }
    let out = cups("lpstat", &["-d"]).ok()?;
    out.lines().find_map(|l| l.split_once("default destination:").map(|(_, n)| n.trim().to_string())).filter(|n| !n.is_empty())
}

/// Lee `clave=valor clave='valor con espacios' …` (la salida de lpoptions).
fn parse_options(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            key.push(c);
            chars.next();
        }
        if key.is_empty() {
            break;
        }
        let mut value = String::new();
        if chars.peek() == Some(&'=') {
            chars.next();
            let mut quote = None;
            while let Some(&c) = chars.peek() {
                match (c, quote) {
                    ('\\', _) => {
                        chars.next();
                        if let Some(n) = chars.next() {
                            value.push(n);
                        }
                        continue;
                    }
                    ('\'' | '"', None) => quote = Some(c),
                    (c, Some(q)) if c == q => quote = None,
                    (c, None) if c.is_whitespace() => break,
                    (c, _) => value.push(c),
                }
                chars.next();
            }
        }
        out.push((key, value));
    }
    out
}

/// Ajustes del controlador: en CUPS, ninguno aparte de los del diálogo.
#[derive(Default)]
pub struct DriverSettings {}

impl DriverSettings {
    pub fn properties(&mut self, _printer: &str) -> Option<DriverChoice> {
        None
    }
}

/// Lo elegido en el diálogo del controlador (no lo hay en CUPS).
#[allow(dead_code)]
pub struct DriverChoice {
    pub copies: u32,
    pub color: bool,
    pub duplex: Duplex,
}

/// Trabajo: las hojas se van juntando en un PDF que se manda al final.
pub struct Spool {
    pdf: FlatPdf,
    opts: Options,
}

impl Spool {
    pub fn start(opts: &Options, _driver: &mut DriverSettings) -> Result<Spool, String> {
        let pdf = FlatPdf::new().map_err(|e| format!("{e:#}"))?;
        Ok(Spool { pdf, opts: opts.clone() })
    }

    /// CUPS hace las copias (`lp -n`).
    pub fn handles_copies(&self) -> bool {
        true
    }

    pub fn add(&mut self, doc: &Document, sheet: &Sheet, gray: bool) -> Result<(), String> {
        let n = self.pdf.page_count() + 1;
        let (w, h, px) = super::render(doc, sheet, PRINT_DPI, gray).ok_or_else(|| t!("err.rasterize", n))?;
        self.pdf.add(sheet.width, sheet.height, w, h, &px).map_err(|e| format!("{e:#}"))
    }

    pub fn finish(self) -> Result<(), String> {
        let path = temp_file();
        self.pdf.save(&path).map_err(|e| format!("{e:#}"))?;
        let o = &self.opts;
        let sides = match o.duplex {
            Duplex::Off => "sides=one-sided",
            Duplex::LongEdge => "sides=two-sided-long-edge",
            Duplex::ShortEdge => "sides=two-sided-short-edge",
        };
        let color = if o.color { "print-color-mode=color" } else { "print-color-mode=monochrome" };
        let copies = o.copies.to_string();
        let file = path.to_string_lossy().into_owned();
        let args = ["-d", &o.printer, "-n", &copies, "-t", &o.title, "-o", "fit-to-page", "-o", "collate=true", "-o", sides, "-o", color, "--", &file];
        let result = Command::new("lp").args(args).env("LC_ALL", "C").output();
        let _ = std::fs::remove_file(&path);
        match result {
            Ok(out) if out.status.success() => {
                log::info!("lp: {}", String::from_utf8_lossy(&out.stdout).trim());
                Ok(())
            }
            Ok(out) => Err(t!("print.lp_failed", String::from_utf8_lossy(&out.stderr).trim())),
            Err(e) => Err(t!("print.lp_failed", e)),
        }
    }

    pub fn abort(self) {}
}

fn temp_file() -> PathBuf {
    let dir = std::env::temp_dir().join("form-pdf-reader");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    dir.join(format!("imprimir-{}-{stamp}.pdf", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::parse_options;

    #[test]
    fn opciones_de_lpoptions() {
        let o = parse_options("copies=1 printer-info='HP Deskjet 3700' printer-location printer-make-and-model='HP Deskjet 3700 Series, hpcups 3.26.4' printer-type=4108 a=b\\ c");
        let get = |k: &str| o.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        assert_eq!(get("copies"), Some("1"));
        assert_eq!(get("printer-info"), Some("HP Deskjet 3700"));
        assert_eq!(get("printer-location"), Some(""));
        assert_eq!(get("printer-make-and-model"), Some("HP Deskjet 3700 Series, hpcups 3.26.4"));
        assert_eq!(get("printer-type"), Some("4108"));
        assert_eq!(get("a"), Some("b c"));
    }
}
