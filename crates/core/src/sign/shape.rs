//! Una firma como trazados vectoriales, listos para el PDF: las letras de
//! las firmas escritas salen del contorno de la fuente (el PDF no necesita
//! llevar la fuente) y los trazos dibujados se suavizan con curvas.
//!
//! Coordenadas en puntos, con el origen abajo a la izquierda (como en PDF)
//! y la firma a su tamaño natural; al ponerla en la página se escala.

use super::library::{SavedSignature, SignatureKind, Stroke};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapePath {
    pub segments: Vec<Segment>,
    /// Grosor del trazo; None: se rellena (letras).
    pub stroke: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub paths: Vec<ShapePath>,
    pub width: f32,
    pub height: f32,
}

/// Tamaño de la letra de las firmas escritas, en puntos.
const FONT_SIZE: f32 = 48.0;
/// Margen alrededor, en puntos.
const PAD: f32 = 4.0;

impl Shape {
    pub fn of(sig: &SavedSignature) -> Shape {
        let (paths, stroke_pad) = match &sig.kind {
            SignatureKind::Typed { text, style } | SignatureKind::Initials { text, style } => (text_paths(text, style.font()), 0.0),
            SignatureKind::Drawn { strokes } => {
                let w = strokes.iter().map(|s| s.width).fold(0.0, f32::max);
                (stroke_paths(strokes), w / 2.0)
            }
        };
        normalize(paths, stroke_pad)
    }

    /// Ancho / alto.
    pub fn aspect(&self) -> f32 {
        if self.height > 0.0 { self.width / self.height } else { 1.0 }
    }

    /// Los mismos trazados escalados y movidos al rectángulo de página
    /// (izquierda, abajo, ancho, alto), manteniendo la proporción y
    /// centrados en él. Devuelve también la escala aplicada.
    pub fn fit(&self, left: f32, bottom: f32, w: f32, h: f32) -> (Vec<ShapePath>, f32) {
        let s = (w / self.width.max(1e-3)).min(h / self.height.max(1e-3));
        let ox = left + (w - self.width * s) / 2.0;
        let oy = bottom + (h - self.height * s) / 2.0;
        let t = |x: f32, y: f32| (ox + x * s, oy + y * s);
        let paths = self
            .paths
            .iter()
            .map(|p| ShapePath {
                segments: p
                    .segments
                    .iter()
                    .map(|seg| match *seg {
                        Segment::MoveTo(x, y) => {
                            let (x, y) = t(x, y);
                            Segment::MoveTo(x, y)
                        }
                        Segment::LineTo(x, y) => {
                            let (x, y) = t(x, y);
                            Segment::LineTo(x, y)
                        }
                        Segment::CubicTo(a, b, c, d, e, f) => {
                            let ((a, b), (c, d), (e, f)) = (t(a, b), t(c, d), t(e, f));
                            Segment::CubicTo(a, b, c, d, e, f)
                        }
                        Segment::Close => Segment::Close,
                    })
                    .collect(),
                stroke: p.stroke.map(|w| w * s),
            })
            .collect();
        (paths, s)
    }
}

/// Mueve los trazados para que empiecen en (PAD, PAD) y calcula el tamaño.
fn normalize(mut paths: Vec<ShapePath>, stroke_pad: f32) -> Shape {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &paths {
        for seg in &p.segments {
            let pts: &[(f32, f32)] = &match *seg {
                Segment::MoveTo(x, y) | Segment::LineTo(x, y) => [(x, y), (x, y), (x, y)],
                Segment::CubicTo(a, b, c, d, e, f) => [(a, b), (c, d), (e, f)],
                Segment::Close => continue,
            };
            for &(x, y) in pts {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 {
        return Shape { paths: Vec::new(), width: 1.0, height: 1.0 };
    }
    let pad = PAD + stroke_pad;
    let (dx, dy) = (pad - x0, pad - y0);
    for p in &mut paths {
        for seg in &mut p.segments {
            *seg = match *seg {
                Segment::MoveTo(x, y) => Segment::MoveTo(x + dx, y + dy),
                Segment::LineTo(x, y) => Segment::LineTo(x + dx, y + dy),
                Segment::CubicTo(a, b, c, d, e, f) => Segment::CubicTo(a + dx, b + dy, c + dx, d + dy, e + dx, f + dy),
                Segment::Close => Segment::Close,
            };
        }
    }
    Shape { paths, width: x1 - x0 + 2.0 * pad, height: y1 - y0 + 2.0 * pad }
}

/// Contornos de las letras de `text` con la fuente dada.
fn text_paths(text: &str, font: &[u8]) -> Vec<ShapePath> {
    let Ok(face) = ttf_parser::Face::parse(font, 0) else { return Vec::new() };
    let scale = FONT_SIZE / face.units_per_em() as f32;
    let space = face.glyph_index(' ').and_then(|g| face.glyph_hor_advance(g)).unwrap_or(face.units_per_em() / 4);
    let mut b = Builder { segs: Vec::new(), scale, x: 0.0, last: (0.0, 0.0) };
    let mut prev = None;
    for c in text.trim().chars() {
        let Some(g) = face.glyph_index(c).or_else(|| face.glyph_index('?')) else { continue };
        if let Some(p) = prev {
            b.x += kerning(&face, p, g) as f32 * scale;
        }
        face.outline_glyph(g, &mut b);
        let mut advance = face.glyph_hor_advance(g).unwrap_or(space) as f32;
        // Las letras de mano suelen traer espacios muy estrechos.
        if c.is_whitespace() {
            advance = advance.max(face.units_per_em() as f32 * 0.35);
        }
        b.x += advance * scale;
        prev = Some(g);
    }
    if b.segs.is_empty() { Vec::new() } else { vec![ShapePath { segments: b.segs, stroke: None }] }
}

/// Ajuste entre dos letras de la tabla `kern`, si la fuente la tiene.
fn kerning(face: &ttf_parser::Face, a: ttf_parser::GlyphId, b: ttf_parser::GlyphId) -> i16 {
    let Some(kern) = face.tables().kern else { return 0 };
    kern.subtables.into_iter().filter(|s| s.horizontal && !s.variable).find_map(|s| s.glyphs_kerning(a, b)).unwrap_or(0)
}

struct Builder {
    segs: Vec<Segment>,
    scale: f32,
    x: f32,
    last: (f32, f32),
}

impl Builder {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.scale, y * self.scale)
    }
}

impl ttf_parser::OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.segs.push(Segment::MoveTo(x, y));
        self.last = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.segs.push(Segment::LineTo(x, y));
        self.last = (x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (cx, cy) = self.p(x1, y1);
        let (x, y) = self.p(x, y);
        self.segs.push(quad_as_cubic(self.last, (cx, cy), (x, y)));
        self.last = (x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (c, d) = self.p(x2, y2);
        let (x, y) = self.p(x, y);
        self.segs.push(Segment::CubicTo(a, b, c, d, x, y));
        self.last = (x, y);
    }

    fn close(&mut self) {
        self.segs.push(Segment::Close);
    }
}

fn quad_as_cubic(p0: (f32, f32), c: (f32, f32), p: (f32, f32)) -> Segment {
    let k = 2.0 / 3.0;
    Segment::CubicTo(p0.0 + k * (c.0 - p0.0), p0.1 + k * (c.1 - p0.1), p.0 + k * (c.0 - p.0), p.1 + k * (c.1 - p.1), p.0, p.1)
}

/// Trazos dibujados (y hacia abajo) como líneas suavizadas (y hacia arriba).
fn stroke_paths(strokes: &[Stroke]) -> Vec<ShapePath> {
    strokes
        .iter()
        .filter(|s| !s.points.is_empty())
        .map(|s| {
            let pts: Vec<(f32, f32)> = s.points.iter().map(|&(x, y)| (x, -y)).collect();
            let mut segs = vec![Segment::MoveTo(pts[0].0, pts[0].1)];
            match pts.len() {
                // Un toque: un punto (con los extremos redondos se ve).
                1 => segs.push(Segment::LineTo(pts[0].0 + 0.01, pts[0].1)),
                2 => segs.push(Segment::LineTo(pts[1].0, pts[1].1)),
                n => {
                    // Curvas por los puntos medios: el trazo pasa cerca de
                    // cada punto sin las esquinas de la línea quebrada.
                    let mut last = pts[0];
                    for i in 1..n - 1 {
                        let mid = ((pts[i].0 + pts[i + 1].0) / 2.0, (pts[i].1 + pts[i + 1].1) / 2.0);
                        segs.push(quad_as_cubic(last, pts[i], mid));
                        last = mid;
                    }
                    segs.push(Segment::LineTo(pts[n - 1].0, pts[n - 1].1));
                }
            }
            ShapePath { segments: segs, stroke: Some(s.width) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::{TypeStyle, library::DEFAULT_INK};

    #[test]
    fn firma_escrita() {
        for style in TypeStyle::ALL {
            let sig = SavedSignature { id: "x".into(), kind: SignatureKind::Typed { text: "Juan Pérez".into(), style }, color: DEFAULT_INK };
            let s = Shape::of(&sig);
            assert!(!s.paths.is_empty(), "{style:?}");
            assert!(s.width > 3.0 * s.height, "{style:?}: {}x{}", s.width, s.height);
        }
    }

    #[test]
    fn firma_dibujada() {
        let sig = SavedSignature {
            id: "x".into(),
            kind: SignatureKind::Drawn { strokes: vec![Stroke { width: 3.0, points: vec![(10.0, 10.0), (60.0, 40.0), (110.0, 10.0)] }] },
            color: DEFAULT_INK,
        };
        let s = Shape::of(&sig);
        assert_eq!(s.paths.len(), 1);
        assert!((s.width - (100.0 + 2.0 * (PAD + 1.5))).abs() < 0.01);
        // Encaja en un rectángulo con la proporción.
        let (p, k) = s.fit(100.0, 200.0, 50.0, 50.0);
        assert!(k > 0.0 && p.len() == 1);
    }
}
