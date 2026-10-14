//! Firmas guardadas en el dispositivo: un fichero de texto por firma en la
//! carpeta que diga la plataforma (la de ajustes en escritorio, la privada
//! de la app en Android).
//!
//! Formato (UTF-8, una clave por línea):
//!
//! ```text
//! form-pdf-reader-signature 1
//! kind = typed            (typed, initials o drawn)
//! style = great-vibes     (letra, si es escrita)
//! color = 1b2a4a          (tinta, RGB en hexadecimal)
//! text = Juan Pérez       (si es escrita)
//! stroke = 2.5 10,20 11,22 …   (si es dibujada: grosor y puntos x,y)
//! ```

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

const MAGIC: &str = "form-pdf-reader-signature 1";

/// Letras para las firmas escritas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeStyle {
    GreatVibes,
    Allura,
    HomemadeApple,
}

impl TypeStyle {
    pub const ALL: [TypeStyle; 3] = [TypeStyle::GreatVibes, TypeStyle::Allura, TypeStyle::HomemadeApple];

    pub fn key(self) -> &'static str {
        match self {
            TypeStyle::GreatVibes => "great-vibes",
            TypeStyle::Allura => "allura",
            TypeStyle::HomemadeApple => "homemade-apple",
        }
    }

    pub fn from_key(k: &str) -> Option<TypeStyle> {
        TypeStyle::ALL.into_iter().find(|s| s.key() == k)
    }

    /// La fuente (TrueType) incluida en el programa.
    pub fn font(self) -> &'static [u8] {
        match self {
            TypeStyle::GreatVibes => include_bytes!("../../assets/signature-fonts/GreatVibes-Regular.ttf"),
            TypeStyle::Allura => include_bytes!("../../assets/signature-fonts/Allura-Regular.ttf"),
            TypeStyle::HomemadeApple => include_bytes!("../../assets/signature-fonts/HomemadeApple-Regular.ttf"),
        }
    }
}

/// Un trazo dibujado: grosor y puntos, en las unidades del lienzo donde se
/// dibujó (y hacia abajo).
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub width: f32,
    pub points: Vec<(f32, f32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SignatureKind {
    /// Nombre y apellidos escritos con una letra de mano.
    Typed { text: String, style: TypeStyle },
    /// Iniciales, igual.
    Initials { text: String, style: TypeStyle },
    /// Firma dibujada con el dedo, el ratón o un lápiz.
    Drawn { strokes: Vec<Stroke> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedSignature {
    /// Nombre del fichero sin extensión; también ordena por antigüedad.
    pub id: String,
    pub kind: SignatureKind,
    /// Tinta, RGB.
    pub color: [u8; 3],
}

/// Tinta por defecto: azul oscuro, como un bolígrafo.
pub const DEFAULT_INK: [u8; 3] = [0x1b, 0x2a, 0x4a];

impl SavedSignature {
    pub fn is_initials(&self) -> bool {
        matches!(self.kind, SignatureKind::Initials { .. })
    }

    fn to_text(&self) -> String {
        let mut s = format!("{MAGIC}\n");
        let [r, g, b] = self.color;
        match &self.kind {
            SignatureKind::Typed { text, style } | SignatureKind::Initials { text, style } => {
                let kind = if self.is_initials() { "initials" } else { "typed" };
                s += &format!("kind = {kind}\nstyle = {}\ncolor = {r:02x}{g:02x}{b:02x}\ntext = {}\n", style.key(), one_line(text));
            }
            SignatureKind::Drawn { strokes } => {
                s += &format!("kind = drawn\ncolor = {r:02x}{g:02x}{b:02x}\n");
                for st in strokes {
                    s += &format!("stroke = {}", st.width);
                    for (x, y) in &st.points {
                        s += &format!(" {x:.1},{y:.1}");
                    }
                    s.push('\n');
                }
            }
        }
        s
    }

    fn parse(id: &str, text: &str) -> Result<SavedSignature> {
        let mut lines = text.lines();
        if lines.next().map(str::trim) != Some(MAGIC) {
            bail!("no es una firma guardada");
        }
        let (mut kind, mut style, mut color, mut txt, mut strokes) = (None, TypeStyle::GreatVibes, DEFAULT_INK, None, Vec::new());
        for line in lines {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "kind" => kind = Some(v.to_string()),
                "style" => style = TypeStyle::from_key(v).unwrap_or(style),
                "color" => color = parse_color(v).unwrap_or(color),
                "text" => txt = Some(v.to_string()),
                "stroke" => {
                    let mut parts = v.split_whitespace();
                    let width = parts.next().and_then(|w| w.parse().ok()).unwrap_or(2.0);
                    let points = parts
                        .filter_map(|p| {
                            let (x, y) = p.split_once(',')?;
                            Some((x.parse().ok()?, y.parse().ok()?))
                        })
                        .collect();
                    strokes.push(Stroke { width, points });
                }
                _ => {}
            }
        }
        let kind = match (kind.as_deref(), txt) {
            (Some("typed"), Some(text)) => SignatureKind::Typed { text, style },
            (Some("initials"), Some(text)) => SignatureKind::Initials { text, style },
            (Some("drawn"), _) if !strokes.is_empty() => SignatureKind::Drawn { strokes },
            _ => bail!("firma incompleta"),
        };
        Ok(SavedSignature { id: id.to_string(), kind, color })
    }
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ").trim().to_string()
}

fn parse_color(v: &str) -> Option<[u8; 3]> {
    if v.len() != 6 {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&v[i..i + 2], 16).ok();
    Some([c(0)?, c(2)?, c(4)?])
}

/// Las firmas de un dispositivo.
pub struct Library {
    dir: PathBuf,
}

impl Library {
    pub fn new(dir: impl Into<PathBuf>) -> Library {
        Library { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Todas, de la más antigua a la más nueva. Los ficheros que no se
    /// entienden se ignoran.
    pub fn list(&self) -> Vec<SavedSignature> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut v: Vec<SavedSignature> = rd
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                if path.extension()? != "sig" {
                    return None;
                }
                let id = path.file_stem()?.to_str()?.to_string();
                let text = std::fs::read_to_string(&path).ok()?;
                SavedSignature::parse(&id, &text).map_err(|err| log::warn!("firma {}: {err}", path.display())).ok()
            })
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// Guarda una firma nueva y la devuelve con su identificador.
    pub fn add(&self, kind: SignatureKind, color: [u8; 3]) -> Result<SavedSignature> {
        if let SignatureKind::Typed { text, .. } | SignatureKind::Initials { text, .. } = &kind
            && one_line(text).is_empty()
        {
            bail!("firma vacía");
        }
        if let SignatureKind::Drawn { strokes } = &kind
            && strokes.iter().all(|s| s.points.is_empty())
        {
            bail!("firma vacía");
        }
        std::fs::create_dir_all(&self.dir).with_context(|| format!("no se pudo crear {}", self.dir.display()))?;
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let mut n = 0;
        let id = loop {
            let id = format!("{ms:015}-{n}");
            if !self.dir.join(format!("{id}.sig")).exists() {
                break id;
            }
            n += 1;
        };
        let sig = SavedSignature { id, kind, color };
        std::fs::write(self.dir.join(format!("{}.sig", sig.id)), sig.to_text())?;
        Ok(sig)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        if id.contains(['/', '\\']) || id.contains("..") {
            bail!("identificador no válido");
        }
        std::fs::remove_file(self.dir.join(format!("{id}.sig")))?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<SavedSignature> {
        self.list().into_iter().find(|s| s.id == id)
    }
}

/// Iniciales de un nombre: "Juan Pérez García" → "JPG".
pub fn initials_of(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guardar_y_leer() {
        let dir = std::env::temp_dir().join(format!("pdfre-firmas-{}", std::process::id()));
        let lib = Library::new(&dir);
        assert!(lib.list().is_empty());
        let a = lib.add(SignatureKind::Typed { text: "Juan Pérez".into(), style: TypeStyle::Allura }, DEFAULT_INK).unwrap();
        let b = lib.add(SignatureKind::Drawn { strokes: vec![Stroke { width: 2.5, points: vec![(1.0, 2.0), (3.5, 4.0)] }] }, [0, 0, 0]).unwrap();
        let c = lib.add(SignatureKind::Initials { text: "JP".into(), style: TypeStyle::GreatVibes }, DEFAULT_INK).unwrap();
        assert_eq!(lib.list(), vec![a.clone(), b.clone(), c.clone()]);
        lib.remove(&b.id).unwrap();
        assert_eq!(lib.list(), vec![a, c]);
        assert!(lib.add(SignatureKind::Typed { text: "  ".into(), style: TypeStyle::Allura }, DEFAULT_INK).is_err());
        assert!(lib.remove("../x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn iniciales() {
        assert_eq!(initials_of("juan  pérez garcía"), "JPG");
        assert_eq!(initials_of(""), "");
    }
}
