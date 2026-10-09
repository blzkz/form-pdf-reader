use form_pdf_reader::xfa::pdfedit::XfaPdf;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&a[1]).unwrap();
    let x = XfaPdf::parse(&b).unwrap().unwrap();
    for p in &x.packets { eprintln!("{}", p.name); }
    std::fs::write(&a[3], x.packet_data(&a[2]).unwrap()).unwrap();
}
