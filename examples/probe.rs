//! Diagnóstico: cargo run --example probe -- FICHERO "click:x,y;shot:y0,h,ruta;type:txt;key:down;datos"
use form_pdf_reader::pdfium::{self, Document, keys};
use form_pdf_reader::xfa;
use std::path::Path;

fn shot(doc: &Document, y0: f64, h: f64, path: &str) {
    let s: f64 = std::env::var("PROBE_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
    let (w, ph) = doc.page_size(0);
    let (rw, rh) = ((w as f64 * s) as i32, (h * s) as i32);
    let buf = doc.render_region(0, (w as f64 * s) as i32, (ph as f64 * s) as i32, 0, (y0 * s) as i32, rw, rh).unwrap();
    let f = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(f, rw as u32, rh as u32);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    let mut wtr = e.write_header().unwrap();
    // BGRA -> RGBA
    let mut v = buf.clone();
    for p in v.chunks_mut(4) { p.swap(0, 2); p[3] = 255; }
    wtr.write_image_data(&v).unwrap();
}

fn main() {
    env_logger::init();
    let a: Vec<String> = std::env::args().collect();
    pdfium::init();
    let mut doc = Document::open(Path::new(&a[1])).unwrap();
    let sep = std::env::var("PROBE_SEP").unwrap_or(";".into());
    for step in a[2].split(sep.as_str()).filter(|s| !s.is_empty()) {
        let (c, arg) = step.split_once(':').unwrap_or((step, ""));
        let n: Vec<f64> = arg.split(',').filter_map(|x| x.parse().ok()).collect();
        match c {
            "click" => {
                doc.mouse_move(0, n[0], n[1], 0);
                let d = doc.left_down(0, n[0], n[1], 0);
                doc.left_up(0, n[0], n[1], 0);
                let u = doc.take_updates();
                for f in &u.open_files { println!("abrir: {}", f.display()); }
                doc.refresh_if_structure_changed();
                println!("click {arg}: down={d} field={} list_open={}", doc.field_at(0, n[0], n[1]), doc.list_open());
            }
            "move" => { doc.mouse_move(0, n[0], n[1], 0); }
            "field" => println!("field_at {arg} = {}", doc.field_at(0, n[0], n[1])),
            "shot" => shot(&doc, n[0], n[1], arg.splitn(3, ',').nth(2).unwrap()),
            "type" => doc.type_text(arg),
            "kill" => doc.kill_focus(),
            "refresh" => { doc.refresh_widgets(); }
            "cmd" => doc.exec_click(arg),
            "eval" => println!("eval: {:?}", doc.eval_js(arg)),
            "key" => {
                let vk = match arg { "down" => keys::VK_DOWN, "up" => keys::VK_UP, "enter" => keys::VK_RETURN, "tab" => keys::VK_TAB, "esc" => keys::VK_ESCAPE, _ => 0 };
                doc.key_down(vk, 0); doc.key_up(vk, 0);
            }
            "datos" => {
                let b = doc.bytes_for_save().unwrap();
                let s = String::from_utf8_lossy(&xfa::read_datasets(&b).unwrap()).into_owned();
                let tag = std::env::var("PROBE_TAG").unwrap_or("titulaciones".into());
                if let (Some(i), Some(j)) = (s.find(&format!("<{tag}>")), s.find(&format!("</{tag}>"))) { println!("{}", &s[i..j]); }
            }
            "save" => doc.save(Path::new(arg)).unwrap(),
            _ => eprintln!("?? {c}"),
        }
    }
}
