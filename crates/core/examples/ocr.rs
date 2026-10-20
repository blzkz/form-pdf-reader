//! Prueba del OCR: añade la capa de texto a un PDF y la lee.
//!
//!   cargo run --release -p form-pdf-reader-core --features ocr --example ocr -- entrada.pdf salida.pdf
use form_pdf_reader::ocr::{Ocr, add_text_layer};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let m = |f: &str| std::fs::read(format!("vendor/models/{f}")).unwrap();
    let t0 = std::time::Instant::now();
    let ocr = Ocr::load(&m("det.onnx"), &m("rec.onnx"), &String::from_utf8(m("rec_dict.txt")).unwrap()).unwrap();
    println!("modelos cargados en {:?}", t0.elapsed());
    let t1 = std::time::Instant::now();
    let (pdf, n) = add_text_layer(&std::fs::read(&args[1]).unwrap(), None, &ocr, false, |i, total| eprintln!("página {i}/{total}")).unwrap();
    println!("{n} líneas en {:?}", t1.elapsed());
    std::fs::write(&args[2], pdf).unwrap();
}
