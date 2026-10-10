//! form-pdf-reader: visor de PDF y editor de formularios (AcroForm y XFA).

mod app;
use form_pdf_reader::t;
mod autotest;
mod settings;
mod viewer;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,lopdf=error")).init();

    // Idioma elegido en los ajustes (antes del primer texto).
    settings::Settings::load().apply_language();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli(&args) {
        std::process::exit(code);
    }
    let mut file: Option<PathBuf> = None;
    for a in args {
        match a.as_str() {
            "-h" | "--help" => {
                println!("{}", t!("cli.help"));
                return Ok(());
            }
            "-V" | "--version" => {
                println!("form-pdf-reader {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => file = Some(PathBuf::from(a)),
        }
    }

    // PDFium y V8 se inicializan en este hilo, que es el de la interfaz.
    form_pdf_reader::pdfium::init();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Form PDF Reader")
            .with_app_id("form-pdf-reader")
            // Icono propio de la ventana (X11; en Wayland el panel lo toma
            // del .desktop a partir del app_id).
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icons/256/form-pdf-reader.png")).unwrap_or_default())
            .with_inner_size([1100.0, 900.0])
            .with_min_inner_size([480.0, 360.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native("form-pdf-reader", options, Box::new(move |cc| Ok(Box::new(app::PdfApp::new(cc, file)))))
}


/// Órdenes de línea de comandos sin interfaz. Devuelve el código de salida
/// si se ejecutó alguna.
fn cli(args: &[String]) -> Option<i32> {
    use form_pdf_reader::pdfium::Document;
    let first = args.first()?.as_str();
    let run = |f: &dyn Fn() -> anyhow::Result<()>| match f() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    match first {
        "--export-flat" | "--exportar-plano" => Some(run(&|| {
            let (Some(i), Some(o)) = (args.get(1), args.get(2)) else { anyhow::bail!(t!("cli.missing_in_out")) };
            let dpi = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(150.0);
            let mut d = Document::open(std::path::Path::new(i))?;
            d.export_flat(std::path::Path::new(o), dpi)?;
            println!("{}", t!("cli.exported", o));
            Ok(())
        })),
        "--data" | "--datos" => Some(run(&|| {
            let Some(i) = args.get(1) else { anyhow::bail!(t!("cli.missing_in")) };
            let b = std::fs::read(i)?;
            let ds = form_pdf_reader::xfa::read_datasets(&b)?;
            println!("{}", String::from_utf8_lossy(&ds));
            Ok(())
        })),
        "--info" => Some(run(&|| {
            let Some(i) = args.get(1) else { anyhow::bail!(t!("cli.missing_in")) };
            let d = Document::open(std::path::Path::new(i))?;
            println!("{}", t!("cli.info", format!("{:?}", d.kind()), d.is_xfa(), d.page_count(), d.is_continuous()));
            if let Some(c) = d.compat() {
                println!("{}", t!("cli.fixes", format!("{c:?}")));
            }
            Ok(())
        })),
        _ => None,
    }
}
