//! Reconocimiento de texto (OCR) con los modelos PP-OCR de PaddleOCR
//! (Apache 2.0) en formato ONNX, ejecutados con tract: el de detección de
//! líneas (PP-OCRv3 mobile det) y el de reconocimiento del alfabeto latino
//! (PP-OCRv5 latin mobile rec, 32 idiomas).
//!
//! Sirve para dar a las páginas escaneadas una capa de texto invisible
//! encima de la imagen: el PDF se puede buscar y el texto seleccionar y
//! copiar, en esta aplicación y en cualquier visor.

use crate::pdfium::{init, sys};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::ffi::c_int;
use std::sync::{Arc, Mutex};
use tract_onnx::prelude::*;

/// Lado mayor de la imagen que mira el detector.
const DET_SIDE: f32 = 1280.0;
/// Alto de las líneas para el reconocedor, y el ancho se agrupa en tramos.
const REC_HEIGHT: usize = 48;
const REC_STEP: usize = 160;
const REC_MAX_WIDTH: usize = 3200;
/// Umbrales del detector (los de PaddleOCR).
const DET_THRESHOLD: f32 = 0.3;
const BOX_THRESHOLD: f32 = 0.6;
const UNCLIP_RATIO: f32 = 1.5;
/// Resolución a la que se leen las páginas de un PDF.
const PAGE_DPI: f32 = 200.0;
const PAGE_MAX_SIDE: f32 = 2600.0;

/// Una línea de texto reconocida: su texto, su caja (izquierda, arriba,
/// derecha, abajo, en píxeles de la imagen) y la confianza (0 a 1).
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub rect: [f32; 4],
    pub confidence: f32,
}

type Plan = Arc<TypedRunnableModel>;

/// Los modelos de OCR cargados.
pub struct Ocr {
    det: InferenceModel,
    rec: InferenceModel,
    dict: Vec<String>,
    /// Modelos ya preparados para cada tamaño de entrada.
    plans: Mutex<HashMap<(bool, usize, usize), Plan>>,
}

impl Ocr {
    /// Carga los modelos (los bytes de det.onnx y rec.onnx) y el diccionario
    /// del reconocedor (un carácter por línea).
    pub fn load(det: &[u8], rec: &[u8], dict: &str) -> Result<Ocr> {
        let det = tract_onnx::onnx().model_for_read(&mut std::io::Cursor::new(det)).context("el modelo de detección no es un ONNX válido")?;
        let rec = tract_onnx::onnx().model_for_read(&mut std::io::Cursor::new(rec)).context("el modelo de reconocimiento no es un ONNX válido")?;
        let dict: Vec<String> = dict.lines().map(str::to_string).collect();
        if dict.is_empty() {
            bail!("el diccionario está vacío");
        }
        Ok(Ocr { det, rec, dict, plans: Mutex::new(HashMap::new()) })
    }

    /// El modelo preparado para una entrada de `h`×`w` (detector o reconocedor).
    fn plan(&self, det: bool, h: usize, w: usize) -> Result<Plan> {
        let mut plans = self.plans.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = plans.get(&(det, h, w)) {
            return Ok(p.clone());
        }
        let m = if det { &self.det } else { &self.rec };
        let p = m.clone().with_input_fact(0, f32::fact([1, 3, h, w]).into())?.into_optimized()?.into_runnable()?;
        plans.insert((det, h, w), p.clone());
        Ok(p)
    }

    /// Las líneas de texto de una imagen RGBA (`w`×`h`), de arriba abajo.
    pub fn recognize(&self, rgba: &[u8], w: usize, h: usize) -> Result<Vec<OcrLine>> {
        if w < 8 || h < 8 || rgba.len() < w * h * 4 {
            return Ok(Vec::new());
        }
        let boxes = self.detect(rgba, w, h)?;
        let mut lines = Vec::new();
        for b in boxes {
            if let Some((text, confidence)) = self.read_line(rgba, w, h, b)?
                && !text.trim().is_empty()
            {
                lines.push(OcrLine { text: text.trim().to_string(), rect: b, confidence });
            }
        }
        Ok(lines)
    }

    /// Cajas de las líneas de texto (izquierda, arriba, derecha, abajo).
    fn detect(&self, rgba: &[u8], w: usize, h: usize) -> Result<Vec<[f32; 4]>> {
        let s = (DET_SIDE / w.max(h) as f32).min(1.0);
        let round32 = |v: f32| ((v / 32.0).round() as usize).max(1) * 32;
        let (dw, dh) = (round32(w as f32 * s), round32(h as f32 * s));
        // BGR, con la media y la desviación de ImageNet (como PaddleOCR).
        const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
        const STD: [f32; 3] = [0.229, 0.224, 0.225];
        let input = tract_ndarray::Array4::from_shape_fn((1, 3, dh, dw), |(_, c, y, x)| {
            let sx = (x as f32 + 0.5) * w as f32 / dw as f32 - 0.5;
            let sy = (y as f32 + 0.5) * h as f32 / dh as f32 - 0.5;
            let v = bilinear(rgba, w, h, sx, sy, 2 - c) / 255.0;
            (v - MEAN[c]) / STD[c]
        });
        let out = self.plan(true, dh, dw)?.run(tvec!(input.into_tensor().into()))?;
        let prob = out[0].to_plain_array_view::<f32>()?;
        let prob: Vec<f32> = prob.iter().copied().collect();
        if prob.len() < dw * dh {
            bail!("el detector da un mapa de {} valores", prob.len());
        }
        let (fx, fy) = (w as f32 / dw as f32, h as f32 / dh as f32);
        let mut boxes: Vec<[f32; 4]> = regions(&prob, dw, dh)
            .into_iter()
            .map(|[l, t, r, b]| {
                // El detector encoge las zonas de texto: se agrandan
                // (distancia = área × 1,5 / perímetro, como PaddleOCR).
                let (bw, bh) = (r - l + 1.0, b - t + 1.0);
                let d = bw * bh * UNCLIP_RATIO / (2.0 * (bw + bh));
                [((l - d) * fx).max(0.0), ((t - d) * fy).max(0.0), ((r + 1.0 + d) * fx).min(w as f32), ((b + 1.0 + d) * fy).min(h as f32)]
            })
            .filter(|b| b[2] - b[0] >= 4.0 && b[3] - b[1] >= 4.0)
            .collect();
        // De arriba abajo y, en la misma línea, de izquierda a derecha.
        boxes.sort_by(|a, b| {
            let same_row = (a[1] - b[1]).abs() < (a[3] - a[1]).min(b[3] - b[1]) / 2.0;
            if same_row { a[0].total_cmp(&b[0]) } else { a[1].total_cmp(&b[1]) }
        });
        Ok(boxes)
    }

    /// Lee el texto de una caja de la imagen.
    fn read_line(&self, rgba: &[u8], w: usize, h: usize, b: [f32; 4]) -> Result<Option<(String, f32)>> {
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        if bw < 2.0 || bh < 2.0 {
            return Ok(None);
        }
        let lw = ((REC_HEIGHT as f32 * bw / bh).round() as usize).clamp(8, REC_MAX_WIDTH);
        let width = lw.div_ceil(REC_STEP) * REC_STEP;
        // BGR normalizado a [-1, 1]; lo que sobra a la derecha queda a 0.
        let input = tract_ndarray::Array4::from_shape_fn((1, 3, REC_HEIGHT, width), |(_, c, y, x)| {
            if x >= lw {
                return 0.0;
            }
            let sx = b[0] + (x as f32 + 0.5) * bw / lw as f32 - 0.5;
            let sy = b[1] + (y as f32 + 0.5) * bh / REC_HEIGHT as f32 - 0.5;
            bilinear(rgba, w, h, sx, sy, 2 - c) / 127.5 - 1.0
        });
        let out = self.plan(false, REC_HEIGHT, width)?.run(tvec!(input.into_tensor().into()))?;
        let v = out[0].to_plain_array_view::<f32>()?;
        let shape = v.shape().to_vec();
        let classes = *shape.last().unwrap_or(&0);
        if classes < 2 {
            bail!("el reconocedor da {classes} clases");
        }
        let values: Vec<f32> = v.iter().copied().collect();
        Ok(Some(self.ctc(&values, classes)))
    }

    /// Decodificación CTC: el más probable de cada paso, sin repetidos ni
    /// vacíos (clase 0); la última clase es el espacio.
    fn ctc(&self, values: &[f32], classes: usize) -> (String, f32) {
        let mut text = String::new();
        let (mut sum, mut n) = (0.0, 0);
        let mut last = 0;
        for step in values.chunks_exact(classes) {
            let (k, p) = step.iter().copied().enumerate().fold((0, f32::MIN), |a, (i, x)| if x > a.1 { (i, x) } else { a });
            if k != 0 && k != last {
                match self.dict.get(k - 1) {
                    Some(c) => text.push_str(c),
                    None => text.push(' '),
                }
                sum += p;
                n += 1;
            }
            last = k;
        }
        (text, if n > 0 { sum / n as f32 } else { 0.0 })
    }
}

/// Zonas de texto del mapa de probabilidad del detector: cada mancha (8
/// vecinos) por encima del umbral cuya probabilidad media pasa del mínimo.
/// Devuelve sus rectángulos (izquierda, arriba, derecha, abajo) en píxeles
/// del mapa.
fn regions(prob: &[f32], w: usize, h: usize) -> Vec<[f32; 4]> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if seen[start] || prob[start] <= DET_THRESHOLD {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut l, mut t, mut r, mut b) = (w, h, 0, 0);
        let (mut sum, mut n) = (0.0f32, 0usize);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            (l, t, r, b) = (l.min(x), t.min(y), r.max(x), b.max(y));
            sum += prob[i];
            n += 1;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !seen[j] && prob[j] > DET_THRESHOLD {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if n >= 6 && sum / n as f32 >= BOX_THRESHOLD && r > l && b > t {
            out.push([l as f32, t as f32, r as f32, b as f32]);
        }
    }
    out
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

/// Añade a un PDF una capa de texto invisible con el texto reconocido en
/// cada página. Las páginas que ya tienen texto se dejan como están.
/// `progress` recibe (páginas hechas, total). Devuelve el PDF y el número de
/// líneas añadidas. `incremental`: los cambios se añaden al final del
/// fichero (para un documento que ya existía).
pub fn add_text_layer(bytes: &[u8], password: Option<&str>, ocr: &Ocr, incremental: bool, mut progress: impl FnMut(usize, usize)) -> Result<(Vec<u8>, usize)> {
    init();
    let d = crate::pdfium::pages::MemDoc::open(bytes, password)?;
    let n = d.page_count();
    let mut added = 0;
    unsafe {
        let font = sys::FPDFText_LoadStandardFont(d.doc, c"Helvetica".as_ptr());
        if font.is_null() {
            bail!("PDFium no pudo cargar la fuente");
        }
        for i in 0..n {
            progress(i, n);
            let page = sys::FPDF_LoadPage(d.doc, i as c_int);
            if page.is_null() {
                continue;
            }
            // Con texto ya (no es una imagen escaneada): nada.
            let tp = sys::FPDFText_LoadPage(page);
            let has_text = !tp.is_null() && sys::FPDFText_CountChars(tp) > 0;
            if !tp.is_null() {
                sys::FPDFText_ClosePage(tp);
            }
            if !has_text {
                let (pw, ph) = (sys::FPDF_GetPageWidthF(page), sys::FPDF_GetPageHeightF(page));
                let s = (PAGE_DPI / 72.0).min(PAGE_MAX_SIDE / pw.max(ph).max(1.0));
                let (w, h) = ((pw * s).round().max(1.0) as i32, (ph * s).round().max(1.0) as i32);
                let rgba = render(page, w, h);
                let lines = ocr.recognize(&rgba, w as usize, h as usize)?;
                for l in &lines {
                    if put_line(d.doc, page, font, l, w, h) {
                        added += 1;
                    }
                }
                if !lines.is_empty() {
                    sys::FPDFPage_GenerateContent(page);
                }
            }
            sys::FPDF_ClosePage(page);
        }
        sys::FPDFFont_Close(font);
    }
    progress(n, n);
    Ok((crate::pdfium::pages::save_doc(d.doc, incremental)?, added))
}

/// La página dibujada a `w`×`h` píxeles, en RGBA.
unsafe fn render(page: sys::FPDF_PAGE, w: i32, h: i32) -> Vec<u8> {
    unsafe {
        let bmp = sys::FPDFBitmap_Create(w, h, 1);
        sys::FPDFBitmap_FillRect(bmp, 0, 0, w, h, 0xFFFF_FFFF);
        sys::FPDF_RenderPageBitmap(bmp, page, 0, 0, w, h, 0, sys::FPDF_REVERSE_BYTE_ORDER as c_int);
        let stride = sys::FPDFBitmap_GetStride(bmp) as usize;
        let src = sys::FPDFBitmap_GetBuffer(bmp) as *const u8;
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h as usize {
            px.extend_from_slice(std::slice::from_raw_parts(src.add(row * stride), w as usize * 4));
        }
        sys::FPDFBitmap_Destroy(bmp);
        px
    }
}

/// Pone el texto de una línea, invisible, sobre su sitio en la página (la
/// página se dibujó a `w`×`h` píxeles). Ocupa justo el ancho de la línea.
unsafe fn put_line(doc: sys::FPDF_DOCUMENT, page: sys::FPDF_PAGE, font: sys::FPDF_FONT, l: &OcrLine, w: i32, h: i32) -> bool {
    unsafe {
        let to_page = |x: f32, y: f32| {
            let (mut px, mut py) = (0.0f64, 0.0f64);
            sys::FPDF_DeviceToPage(page, 0, 0, w, h, 0, x.round() as c_int, y.round() as c_int, &mut px, &mut py);
            (px as f32, py as f32)
        };
        let (x0, y0) = to_page(l.rect[0], l.rect[3]);
        let (x1, y1) = to_page(l.rect[2], l.rect[1]);
        let (left, bottom, right, top) = (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1));
        let height = top - bottom;
        if height <= 0.5 || right - left <= 0.5 {
            return false;
        }
        let size = height * 0.8;
        let obj = sys::FPDFPageObj_CreateTextObj(doc, font, size);
        if obj.is_null() {
            return false;
        }
        let text = crate::pdfium::to_wide(&l.text);
        sys::FPDFText_SetText(obj, text.as_ptr());
        sys::FPDFTextObj_SetTextRenderMode(obj, sys::FPDF_TEXTRENDERMODE_INVISIBLE);
        let (mut bl, mut bb, mut br, mut bt) = (0.0, 0.0, 0.0, 0.0);
        sys::FPDFPageObj_GetBounds(obj, &mut bl, &mut bb, &mut br, &mut bt);
        let tw = (br - bl).max(0.1);
        // Mismo ancho que la línea en la imagen; la línea base un poco por
        // encima del borde inferior.
        let sx = ((right - left) / tw) as f64;
        sys::FPDFPageObj_Transform(obj, sx, 0.0, 0.0, 1.0, left as f64, (bottom + height * 0.22) as f64);
        sys::FPDFPage_InsertObject(page, obj);
        true
    }
}
