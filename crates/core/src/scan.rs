//! Escanear documentos con la cámara: encontrar la hoja en la foto, los
//! filtros de documento y el PDF con las páginas escaneadas.
//!
//! Sin bibliotecas de visión: la hoja suele ser más clara que lo que la
//! rodea, así que se separa con un umbral (Otsu), se toma la mancha clara más
//! grande y sus cuatro puntos extremos son las esquinas. El usuario puede
//! moverlas después. La perspectiva la corrige la aplicación con esas
//! esquinas (en Android, `Matrix.setPolyToPoly`).

use crate::pdfium::{init, sys};
use anyhow::{Result, bail};
use std::ffi::{c_int, c_uchar, c_ulong, c_void};

/// Esquinas de la hoja (izquierda arriba, derecha arriba, derecha abajo,
/// izquierda abajo), en fracciones del ancho y del alto de la foto (0 a 1).
pub type Quad = [[f32; 2]; 4];

/// Busca la hoja en una foto en escala de grises (`w`×`h`, una fila tras
/// otra). Conviene una foto pequeña (unos 400 píxeles de lado). None si no se
/// distingue una hoja.
pub fn detect_document(gray: &[u8], w: usize, h: usize) -> Option<Quad> {
    if w < 8 || h < 8 || gray.len() < w * h {
        return None;
    }
    let img = blur(&blur(&gray[..w * h], w, h), w, h);
    let t = otsu(&img);
    // La mancha clara más grande (4-vecinos).
    let mut label = vec![0u32; w * h];
    let mut best: (usize, u32) = (0, 0);
    let mut next = 1u32;
    let mut stack = Vec::new();
    for start in 0..w * h {
        if img[start] <= t || label[start] != 0 {
            continue;
        }
        let mut n = 0;
        label[start] = next;
        stack.push(start);
        while let Some(i) = stack.pop() {
            n += 1;
            let (x, y) = (i % w, i / w);
            let mut visit = |j: usize| {
                if img[j] > t && label[j] == 0 {
                    label[j] = next;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if n > best.0 {
            best = (n, next);
        }
        next += 1;
    }
    let (area, id) = best;
    let ratio = area as f32 / (w * h) as f32;
    // Ni una mancha pequeña ni la foto entera (no hay fondo distinto).
    if !(0.12..=0.97).contains(&ratio) {
        return None;
    }
    // Los puntos extremos en las diagonales son las esquinas.
    let (mut tl, mut br, mut tr, mut bl) = ((i64::MAX, 0, 0), (i64::MIN, 0, 0), (i64::MIN, 0, 0), (i64::MAX, 0, 0));
    for (i, &l) in label.iter().enumerate() {
        if l != id {
            continue;
        }
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        if x + y < tl.0 {
            tl = (x + y, x, y);
        }
        if x + y > br.0 {
            br = (x + y, x, y);
        }
        if x - y > tr.0 {
            tr = (x - y, x, y);
        }
        if x - y < bl.0 {
            bl = (x - y, x, y);
        }
    }
    let p = |c: (i64, i64, i64)| [(c.1 as f32 + 0.5) / w as f32, (c.2 as f32 + 0.5) / h as f32];
    let quad = [p(tl), p(tr), p(br), p(bl)];
    // Un cuadrilátero que no cubre la mancha no es una hoja.
    let qa = quad_area(&quad) * (w * h) as f32;
    if qa < area as f32 * 0.7 {
        return None;
    }
    Some(quad)
}

/// Área (en fracciones de la foto) de un cuadrilátero.
fn quad_area(q: &Quad) -> f32 {
    let mut a = 0.0;
    for i in 0..4 {
        let (p, n) = (q[i], q[(i + 1) % 4]);
        a += p[0] * n[1] - n[0] * p[1];
    }
    a.abs() / 2.0
}

/// Desenfoque 3×3 (media).
fn blur(img: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut s, mut n) = (0u32, 0u32);
            for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    s += img[yy * w + xx] as u32;
                    n += 1;
                }
            }
            out[y * w + x] = (s / n) as u8;
        }
    }
    out
}

/// Umbral de Otsu: el que mejor separa los tonos en dos grupos.
fn otsu(img: &[u8]) -> u8 {
    let mut hist = [0u64; 256];
    for &v in img {
        hist[v as usize] += 1;
    }
    let total = img.len() as f64;
    let sum: f64 = hist.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum();
    let (mut sum_b, mut w_b, mut best, mut t) = (0.0, 0.0, 0.0, 0u8);
    for (i, &c) in hist.iter().enumerate() {
        w_b += c as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += i as f64 * c as f64;
        let (m_b, m_f) = (sum_b / w_b, (sum - sum_b) / w_f);
        let between = w_b * w_f * (m_b - m_f) * (m_b - m_f);
        if between > best {
            best = between;
            t = i as u8;
        }
    }
    t
}

/// Cómo queda la página escaneada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanFilter {
    /// En color, con el papel más blanco y más contraste.
    Color,
    Gray,
    /// Blanco y negro, como una fotocopia (umbral adaptativo: también con
    /// sombras o luz desigual).
    BlackWhite,
}

/// Aplica el filtro a una imagen RGBA (`w`×`h`) y la devuelve en RGBA.
pub fn filter(rgba: &[u8], w: usize, h: usize, f: ScanFilter) -> Vec<u8> {
    let n = (w * h).min(rgba.len() / 4);
    match f {
        ScanFilter::Color => {
            let mut out = rgba[..n * 4].to_vec();
            for c in 0..3 {
                let (lo, hi) = percentiles(rgba[..n * 4].iter().skip(c).step_by(4).copied());
                for px in out.as_chunks_mut::<4>().0 {
                    px[c] = stretch(px[c], lo, hi);
                }
            }
            out
        }
        ScanFilter::Gray | ScanFilter::BlackWhite => {
            let gray: Vec<u8> = rgba[..n * 4].as_chunks::<4>().0.iter().map(|p| luma(p[0], p[1], p[2])).collect();
            let g = if f == ScanFilter::Gray {
                let (lo, hi) = percentiles(gray.iter().copied());
                gray.iter().map(|&v| stretch(v, lo, hi)).collect()
            } else {
                adaptive_threshold(&gray, w, h)
            };
            g.iter().flat_map(|&v| [v, v, v, 255]).collect()
        }
    }
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8
}

/// Tonos del 2 % y del 98 % (lo más oscuro y lo más claro, sin los extremos).
fn percentiles(values: impl Iterator<Item = u8>) -> (u8, u8) {
    let mut hist = [0u64; 256];
    let mut n = 0u64;
    for v in values {
        hist[v as usize] += 1;
        n += 1;
    }
    let find = |target: u64| {
        let mut acc = 0;
        for (i, &c) in hist.iter().enumerate() {
            acc += c;
            if acc > target {
                return i as u8;
            }
        }
        255
    };
    (find(n / 50), find(n - n / 50 - 1))
}

fn stretch(v: u8, lo: u8, hi: u8) -> u8 {
    if hi <= lo.saturating_add(10) {
        return v;
    }
    (((v.max(lo) - lo) as u32 * 255) / (hi - lo) as u32).min(255) as u8
}

/// Umbral de Bradley: negro lo que es un 15 % más oscuro que la media de su
/// alrededor (una ventana de 1/8 del lado mayor).
fn adaptive_threshold(gray: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut integral = vec![0u64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u64;
        for x in 0..w {
            row += gray[y * w + x] as u64;
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let s = (w.max(h) / 16).max(2);
    let mut out = vec![255u8; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(s), (y + s + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(s), (x + s + 1).min(w));
            let count = ((x1 - x0) * (y1 - y0)) as u64;
            let sum = integral[y1 * (w + 1) + x1] + integral[y0 * (w + 1) + x0] - integral[y0 * (w + 1) + x1] - integral[y1 * (w + 1) + x0];
            if gray[y * w + x] as u64 * count * 100 <= sum * 85 {
                out[y * w + x] = 0;
            }
        }
    }
    out
}

/// Una página escaneada: la foto en JPEG y su tamaño en píxeles.
pub struct ScanImage {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

struct JpegReader<'a>(&'a [u8]);

unsafe extern "C" fn read_block(param: *mut c_void, pos: c_ulong, buf: *mut c_uchar, size: c_ulong) -> c_int {
    unsafe {
        let r = &*(param as *const JpegReader);
        let (pos, size) = (pos as usize, size as usize);
        if pos + size > r.0.len() {
            return 0;
        }
        std::ptr::copy_nonoverlapping(r.0.as_ptr().add(pos), buf, size);
        1
    }
}

/// Ancho de una hoja A4, en puntos.
const A4_WIDTH: f64 = 595.28;

/// Un PDF con una página por imagen. El lado corto de cada página mide lo
/// que el ancho de un A4 y el largo sigue la proporción de la foto.
pub fn images_to_pdf(pages: &[ScanImage]) -> Result<Vec<u8>> {
    if pages.is_empty() {
        bail!("no hay páginas");
    }
    init();
    unsafe {
        let doc = sys::FPDF_CreateNewDocument();
        if doc.is_null() {
            bail!("PDFium no pudo crear el documento");
        }
        let r = (|| {
            for (i, p) in pages.iter().enumerate() {
                if p.width == 0 || p.height == 0 {
                    bail!("la página {} no tiene tamaño", i + 1);
                }
                // PDFium no lo comprueba: un JPEG empieza por FF D8 FF.
                if !p.jpeg.starts_with(&[0xFF, 0xD8, 0xFF]) {
                    bail!("la página {} no es una imagen JPEG válida", i + 1);
                }
                let (w, h) = (p.width as f64, p.height as f64);
                let (pw, ph) = if h >= w { (A4_WIDTH, A4_WIDTH * h / w) } else { (A4_WIDTH * w / h, A4_WIDTH) };
                let mut page = sys::FPDFPage_New(doc, i as c_int, pw, ph);
                if page.is_null() {
                    bail!("PDFium no pudo crear la página {}", i + 1);
                }
                let img = sys::FPDFPageObj_NewImageObj(doc);
                let reader = JpegReader(&p.jpeg);
                let mut access = sys::FPDF_FILEACCESS { m_FileLen: p.jpeg.len() as c_ulong, m_GetBlock: Some(read_block), m_Param: &reader as *const JpegReader as *mut c_void };
                let ok = sys::FPDFImageObj_LoadJpegFileInline(&mut page, 1, img, &mut access) != 0;
                if !ok {
                    sys::FPDFPageObj_Destroy(img);
                    sys::FPDF_ClosePage(page);
                    bail!("la página {} no es una imagen JPEG válida", i + 1);
                }
                sys::FPDFImageObj_SetMatrix(img, pw, 0.0, 0.0, ph, 0.0, 0.0);
                sys::FPDFPage_InsertObject(page, img);
                sys::FPDFPage_GenerateContent(page);
                sys::FPDF_ClosePage(page);
            }
            crate::pdfium::pages::save_doc(doc, false)
        })();
        sys::FPDF_CloseDocument(doc);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Una hoja clara girada sobre un fondo oscuro.
    fn photo(w: usize, h: usize, quad: &[[f32; 2]; 4]) -> Vec<u8> {
        let inside = |x: f32, y: f32| {
            (0..4).all(|i| {
                let (a, b) = (quad[i], quad[(i + 1) % 4]);
                (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]) >= 0.0
            })
        };
        let mut g = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                // Ruido del fondo y del papel.
                let noise = ((x * 7 + y * 13) % 9) as u8;
                g[y * w + x] = if inside(x as f32, y as f32) { 210 + noise } else { 60 + noise * 3 };
            }
        }
        g
    }

    #[test]
    fn encontrar_la_hoja() {
        let (w, h) = (300, 400);
        let q = [[60.0, 50.0], [250.0, 80.0], [230.0, 350.0], [40.0, 320.0]];
        let found = detect_document(&photo(w, h, &q), w, h).expect("debe encontrar la hoja");
        for (f, e) in found.iter().zip(q.iter()) {
            assert!((f[0] * w as f32 - e[0]).abs() < 6.0 && (f[1] * h as f32 - e[1]).abs() < 6.0, "{found:?}");
        }
        // Una foto sin contraste: nada.
        assert!(detect_document(&vec![128u8; w * h], w, h).is_none());
    }

    #[test]
    fn filtros() {
        // Papel gris claro con un trazo oscuro (como el texto) y la mitad
        // derecha en sombra: el blanco y negro deja el papel blanco, también
        // en la sombra, y el trazo negro.
        let (w, h) = (128, 128);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let paper = if x < 64 { 190 } else { 120 };
                let v = if (60..63).contains(&y) && (10..118).contains(&x) { paper / 4 } else { paper + (x % 5) as u8 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let bw = filter(&rgba, w, h, ScanFilter::BlackWhite);
        assert_eq!(bw[(5 * w + 5) * 4], 255);
        assert_eq!(bw[(30 * w + 100) * 4], 255, "papel en sombra");
        assert_eq!(bw[(61 * w + 30) * 4], 0);
        assert_eq!(bw[(61 * w + 100) * 4], 0, "trazo en sombra");
        let c = filter(&rgba, w, h, ScanFilter::Color);
        assert!(c[(5 * w + 5) * 4] > 200, "el papel se aclara");
        assert_eq!(filter(&rgba, w, h, ScanFilter::Gray).len(), rgba.len());
    }
}

/// Orientación de una página escaneada, con el modelo PP-LCNet_x1_0_doc_ori
/// de PaddleOCR (Apache 2.0) en formato ONNX, ejecutado con tract.
#[cfg(feature = "orientation")]
pub mod orientation {
    use anyhow::{Context, Result};
    use tract_onnx::prelude::*;

    /// Lado de la imagen que mira el modelo.
    const SIDE: usize = 224;
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];

    pub struct Orientation {
        model: std::sync::Arc<TypedRunnableModel>,
    }

    impl Orientation {
        /// Carga el modelo (los bytes del fichero .onnx).
        pub fn load(onnx: &[u8]) -> Result<Orientation> {
            let model = tract_onnx::onnx()
                .model_for_read(&mut std::io::Cursor::new(onnx))
                .context("el modelo de orientación no es un ONNX válido")?
                .with_input_fact(0, f32::fact([1, 3, SIDE, SIDE]).into())?
                .into_optimized()?
                .into_runnable()?;
            Ok(Orientation { model })
        }

        /// Cuartos de vuelta a la derecha que hay que girar la imagen RGBA para
        /// ponerla derecha, y la confianza del modelo (0 a 1). El modelo dice
        /// cuánto está girada a la derecha (0°, 90°, 180° o 270°).
        pub fn upright_turns(&self, rgba: &[u8], w: usize, h: usize) -> Result<(u8, f32)> {
            let s = self.scores(rgba, w, h)?;
            let (k, p) = s.iter().copied().enumerate().fold((0, f32::MIN), |a, (i, v)| if v > a.1 { (i, v) } else { a });
            Ok((((4 - k) % 4) as u8, p))
        }

        /// Probabilidad de cada clase (0°, 90°, 180° y 270°) de una imagen RGBA.
        pub fn scores(&self, rgba: &[u8], w: usize, h: usize) -> Result<[f32; 4]> {
            let input = prepare(rgba, w, h);
            let out = self.model.run(tvec!(input.into()))?;
            let v = out[0].to_plain_array_view::<f32>()?;
            let raw: Vec<f32> = v.iter().copied().collect();
            anyhow::ensure!(raw.len() == 4, "el modelo da {} valores", raw.len());
            // Ya son probabilidades (softmax en el modelo) o se normalizan aquí.
            let sum: f32 = raw.iter().sum();
            let p = if (sum - 1.0).abs() < 0.01 && raw.iter().all(|&x| x >= 0.0) {
                raw
            } else {
                let m = raw.iter().copied().fold(f32::MIN, f32::max);
                let e: Vec<f32> = raw.iter().map(|x| (x - m).exp()).collect();
                let s: f32 = e.iter().sum();
                e.iter().map(|x| x / s).collect()
            };
            Ok([p[0], p[1], p[2], p[3]])
        }
    }

    /// Lado corto a 256, recorte central de 224 y normalización (RGB).
    fn prepare(rgba: &[u8], w: usize, h: usize) -> Tensor {
        let scale = 256.0 / w.min(h).max(1) as f32;
        let (rw, rh) = (((w as f32 * scale).round() as usize).max(SIDE), ((h as f32 * scale).round() as usize).max(SIDE));
        let (ox, oy) = ((rw - SIDE) / 2, (rh - SIDE) / 2);
        let t: tract_ndarray::Array4<f32> = tract_ndarray::Array4::from_shape_fn((1, 3, SIDE, SIDE), |(_, c, y, x)| {
            // Punto de la imagen original (muestreo bilineal).
            let sx = ((x + ox) as f32 + 0.5) / rw as f32 * w as f32 - 0.5;
            let sy = ((y + oy) as f32 + 0.5) / rh as f32 * h as f32 - 0.5;
            let v = bilinear(rgba, w, h, sx, sy, c) / 255.0;
            (v - MEAN[c]) / STD[c]
        });
        t.into_tensor()
    }

    fn bilinear(rgba: &[u8], w: usize, h: usize, x: f32, y: f32, c: usize) -> f32 {
        let x = x.clamp(0.0, (w - 1) as f32);
        let y = y.clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize| rgba[(yy * w + xx) * 4 + c] as f32;
        let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
        let bottom = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }
}
