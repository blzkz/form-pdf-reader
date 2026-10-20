//! Prueba del modelo de orientación: una página de un PDF, girada 0, 90,
//! 180 y 270 grados a la derecha, y lo que dice el modelo de cada una.
//!
//!   cargo run -p form-pdf-reader-core --features orientation --example orientacion -- modelo.onnx doc.pdf
use form_pdf_reader::pdfium::Document;
use form_pdf_reader::scan::orientation::Orientation;

fn rotate_right(px: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let mut out = vec![0u8; px.len()];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = (h - 1 - y, x);
            out[(ny * h + nx) * 4..(ny * h + nx) * 4 + 4].copy_from_slice(&px[(y * w + x) * 4..(y * w + x) * 4 + 4]);
        }
    }
    (out, h, w)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let model = Orientation::load(&std::fs::read(&args[1]).unwrap()).unwrap();
    let doc = Document::open(std::path::Path::new(&args[2])).unwrap();
    let (pw, ph) = doc.page_size(0);
    let (w, h) = (600usize, (600.0 * ph / pw) as usize);
    let (mut px, mut cw, mut ch) = (doc.render(0, w as i32, h as i32).unwrap(), w, h);
    for turn in 0..4 {
        let s = model.scores(&px, cw, ch).unwrap();
        println!("girada {:>3}° a la derecha -> {:?}", turn * 90, s.map(|v| (v * 1000.0).round() / 1000.0));
        (px, cw, ch) = rotate_right(&px, cw, ch);
    }
}
