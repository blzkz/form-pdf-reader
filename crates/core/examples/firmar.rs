//! Firma un PDF con un fichero PKCS#12 (para probar la firma digital).
//!
//!   cargo run -p form-pdf-reader-core --example firmar -- entrada.pdf salida.pdf cert.p12 contraseña [visible]

use form_pdf_reader::pades::{self, P12Key, Placement, Request};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("uso: firmar entrada.pdf salida.pdf cert.p12 contraseña [visible]");
        std::process::exit(2);
    }
    let base = std::fs::read(&a[1])?;
    let mut key = P12Key::open(&std::fs::read(&a[3])?, &a[4])?;
    let placement = if a.get(5).is_some() { Placement::New { page: 0, rect: [72.0, 72.0, 272.0, 126.0] } } else { Placement::Invisible };
    let req = Request { placement, reason: Some("Prueba".into()), location: None, handwritten: None };
    std::fs::write(&a[2], pades::sign(&base, None, &req, &mut key)?)?;
    Ok(())
}
