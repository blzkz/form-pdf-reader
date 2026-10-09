//! Pruebas de integración con PDFium.
//!
//! PDFium y V8 quedan ligados al hilo que los inicializa, así que todo va en
//! una única función de test (cargo ejecuta cada #[test] en su propio hilo).

use form_pdf_reader::pdfium::{Document, FormKind, keys};
use form_pdf_reader::xfa;
use std::path::{Path, PathBuf};

/// Fichero de `diagnostico/`, en la raíz del repositorio (las pruebas se
/// ejecutan desde la carpeta del crate). No está en el repositorio: las
/// partes que lo usan se omiten si no existe.
fn diag(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../diagnostico").join(name)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pdfre-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn datasets(doc: &mut Document) -> String {
    let b = doc.bytes_for_save().unwrap();
    String::from_utf8_lossy(&xfa::read_datasets(&b).unwrap()).into_owned()
}

fn click(doc: &mut Document, x: f64, y: f64) {
    doc.left_down(0, x, y, 0);
    doc.left_up(0, x, y, 0);
}

fn xfa_dinamico() {
    let src = &diag("TINTERNET_original.pdf");
    if !src.exists() {
        eprintln!("(sin {}: se omite la prueba XFA)", src.display());
        return;
    }
    let mut doc = Document::open(src).unwrap();
    assert_eq!(doc.kind(), FormKind::XfaFull);
    assert!(doc.is_xfa() && doc.is_dynamic_xfa() && doc.is_continuous());
    let c = doc.compat().unwrap().clone();
    assert!(c.refresh_button && c.resolve_node_patches == 1 && c.breaks_neutralized == 2);
    assert_eq!(doc.page_count(), 1);
    let h0 = doc.content_height(0);
    assert!(h0 > 2000.0 && h0 < 3500.0, "alto inesperado {h0}");
    assert!(!doc.refresh_if_structure_changed());

    // Pulsar en la ETIQUETA de "Nombre", escribir, Tab, escribir.
    click(&mut doc, 100.0, 788.0);
    doc.type_text("Juan");
    doc.key_down(keys::VK_TAB, 0);
    doc.char_input(9, 0);
    doc.key_up(keys::VK_TAB, 0);
    doc.type_text("Pérez");
    // Casilla "Hombre" pulsando en su texto.
    click(&mut doc, 286.0, 968.0);
    doc.kill_focus();
    let ds = datasets(&mut doc);
    // Al salir de cada campo el formulario pasa el texto a mayúsculas (evento
    // "listen" propagado, como en Adobe).
    assert!(ds.contains("<nombre>JUAN</nombre>"), "nombre: {ds:.400}");
    assert!(ds.contains("<primerApellido>PÉREZ</primerApellido>"));
    assert!(ds.contains("<Hombre>1</Hombre>"));
    assert!(ds.contains("<nacionalidadPrincipal>ESP</nacionalidadPrincipal>"), "desplegable por defecto");
    assert!(!ds.contains("<nombre>\n") && !ds.contains("<Hombre>\n"), "quedan saltos de línea de PDFium dentro de valores");

    // Desplegable: abrir con la flecha y elegir con el teclado.
    click(&mut doc, 289.0, 929.0);
    doc.key_down(keys::VK_DOWN, 0);
    doc.key_up(keys::VK_DOWN, 0);
    assert_ne!(doc.focused_text(), "ESP");
    doc.key_down(keys::VK_ESCAPE, 0);
    doc.kill_focus();

    // "Sí" en experiencia profesional: aparece la tabla y se refrescan controles.
    click(&mut doc, 196.0, 1769.0);
    let _ = doc.take_updates();
    assert!(doc.refresh_if_structure_changed());
    assert!(doc.content_height(0) > h0 + 100.0);

    // Guardar: el original queda intacto como prefijo y solo cambian los datos.
    let out = tmp("relleno.pdf");
    doc.save(&out).unwrap();
    let orig = std::fs::read(src).unwrap();
    let saved = std::fs::read(&out).unwrap();
    assert!(saved.starts_with(&orig), "el original no es prefijo del guardado");
    let tpl_orig = xfa::pdfedit::XfaPdf::parse(&orig).unwrap().unwrap().packet_data("template").unwrap();
    let tpl_saved = xfa::pdfedit::XfaPdf::parse(&saved).unwrap().unwrap().packet_data("template").unwrap();
    assert_eq!(tpl_orig, tpl_saved, "la plantilla no debe cambiar");

    // Exportación plana troceada en A4.
    let flat = tmp("plano.pdf");
    doc.export_flat(&flat, 72.0).unwrap();
    let pages = lopdf::Document::load(&flat).unwrap().get_pages().len();
    assert!((3..=6).contains(&pages), "hojas: {pages}");
    drop(doc);

    // Reabrir y comprobar el valor.
    let mut d2 = Document::open(&out).unwrap();
    click(&mut d2, 100.0, 822.0);
    assert_eq!(d2.focused_text(), "JUAN");
}

fn acroform() {
    let src = Path::new("tests/fixtures/acroform.pdf");
    let mut doc = Document::open(src).unwrap();
    assert_eq!(doc.kind(), FormKind::AcroForm);
    assert!(!doc.is_xfa());
    let (_, h) = doc.page_size(0);
    // Coordenadas PDF (origen abajo-izquierda).
    let (x, y) = (300.0, 731.0);
    doc.left_down(0, x, y, 0);
    doc.left_up(0, x, y, 0);
    doc.type_text("Hola");
    doc.left_down(0, 168.0, 693.0, 0);
    doc.left_up(0, 168.0, 693.0, 0);
    doc.kill_focus();
    let out = tmp("acro.pdf");
    doc.save(&out).unwrap();
    let pdf = lopdf::Document::load(&out).unwrap();
    let mut vals = Vec::new();
    for obj in pdf.objects.values() {
        if let Ok(d) = obj.as_dict()
            && let (Ok(t), Ok(v)) = (d.get(b"T"), d.get(b"V")) {
                vals.push(format!("{:?}={:?}", t, v));
            }
    }
    let all = vals.join(" ");
    assert!(all.contains("Hola"), "valores: {all}");
    assert!(all.contains("Yes"), "casilla: {all}");
    let _ = h;
}

fn normal() {
    let doc = Document::open(Path::new("tests/fixtures/normal.pdf")).unwrap();
    assert_eq!(doc.kind(), FormKind::None);
    assert_eq!(doc.page_count(), 3);
    let hits = doc.search("zanahoria");
    assert_eq!(hits.len(), 3);
    assert!(hits.iter().all(|h| !h.rects.is_empty()));
    let px = doc.render(0, 300, 424).unwrap();
    assert_eq!(px.len(), 300 * 424 * 4);
    assert!(px.chunks(4).any(|c| c[0] < 50), "la página debería tener texto negro");
}

fn region(doc: &Document, y0: i32, h: i32) -> Vec<u8> {
    let (w, ph) = doc.page_size(0);
    doc.render_region(0, w as i32, ph as i32, 0, y0, w as i32, h).unwrap()
}

fn click_mm(doc: &mut Document, x: f64, y: f64) {
    doc.mouse_move(0, x, y, 0);
    doc.left_down(0, x, y, 0);
    doc.left_up(0, x, y, 0);
    let _ = doc.take_updates();
    doc.refresh_if_structure_changed();
}

/// Abrir un desplegable y cerrarlo pulsando fuera no debe dejar restos.
fn desplegable_cierre() {
    let src = &diag("TINTERNET_original.pdf");
    if !src.exists() {
        return;
    }
    let mut doc = Document::open(src).unwrap();
    let antes = region(&doc, 944, 300);
    click_mm(&mut doc, 289.0, 929.0);
    assert!(doc.list_open(), "la lista debería estar abierta");
    // Igual que la interfaz: el ratón sale de la lista y se pulsa fuera.
    doc.mouse_move(0, 394.0, 1014.0, 0);
    doc.mouse_move(0, 500.0, 1100.0, 0);
    doc.left_down(0, 500.0, 1100.0, 0);
    doc.left_up(0, 500.0, 1100.0, 0);
    assert!(!doc.list_open());
    let despues = region(&doc, 944, 300);
    let w = doc.page_size(0).0 as usize;
    let diff: Vec<(usize, usize)> = antes.chunks(4).zip(despues.chunks(4)).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| (i % w, 944 + i / w)).collect();
    if let (Some(minx), Some(maxx), Some(miny), Some(maxy)) = (diff.iter().map(|d| d.0).min(), diff.iter().map(|d| d.0).max(), diff.iter().map(|d| d.1).min(), diff.iter().map(|d| d.1).max()) {
        panic!("quedan restos de la lista tras cerrarla: {} píxeles en x {minx}..{maxx}, y {miny}..{maxy}", diff.len());
    }
}

/// Componente de adjuntos del formulario: añadir, ver, eliminar, guardar.
fn adjuntos() {
    let src = &diag("TINTERNET_original.pdf");
    if !src.exists() {
        return;
    }
    let mut doc = Document::open(src).unwrap();
    assert!(doc.has_attachment_bridge());
    let fichero = std::rc::Rc::new(std::cell::RefCell::new(std::fs::canonicalize("tests/fixtures/normal.pdf").unwrap()));
    let f2 = fichero.clone();
    doc.set_browse_handler(Box::new(move || Some(f2.borrow().to_string_lossy().into_owned())));
    doc.set_alert_handler(Box::new(|r| panic!("alerta inesperada: {}", r.message)));

    click_mm(&mut doc, 35.0, 2589.0); // "Sí, presenta documentación adjunta"
    let h1 = doc.content_height(0);
    click_mm(&mut doc, 542.0, 2723.0); // Añadir
    *fichero.borrow_mut() = std::fs::canonicalize("tests/fixtures/acroform.pdf").unwrap();
    click_mm(&mut doc, 542.0, 2723.0); // Añadir otro
    let nombres: Vec<String> = doc.attachments().iter().map(|a| a.file_name.clone()).collect();
    assert_eq!(nombres, ["normal.pdf", "acroform.pdf"]);
    let h2 = doc.content_height(0);
    assert!(h2 > h1, "la tabla debería crecer");

    click_mm(&mut doc, 542.0, 2742.0); // Ver fila 1
    let u = doc.take_updates();
    let _ = u;
    click_mm(&mut doc, 542.0, 2778.0); // Eliminar fila 2
    let nombres: Vec<String> = doc.attachments().iter().map(|a| a.file_name.clone()).collect();
    assert_eq!(nombres, ["normal.pdf"]);
    assert!(doc.content_height(0) < h2, "la fila eliminada debería desaparecer");
    assert!(doc.field_at(0, 100.0, 2770.0) < 0, "quedan campos de la fila eliminada");

    let out = tmp("con_adjunto.pdf");
    doc.kill_focus();
    doc.save(&out).unwrap();
    drop(doc);
    let saved = std::fs::read(&out).unwrap();
    assert!(saved.starts_with(&std::fs::read(src).unwrap()));
    let atts = xfa::attachments::load(&saved).unwrap();
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0].file_name, "normal.pdf");
    assert_eq!(atts[0].data, std::fs::read("tests/fixtures/normal.pdf").unwrap());
    let ds = String::from_utf8_lossy(&xfa::read_datasets(&saved).unwrap()).into_owned();
    assert!(ds.contains("<bdeAttachNombreDoc>normal.pdf</bdeAttachNombreDoc>"));
    assert!(!ds.contains("acroform.pdf"));
    let mut d2 = Document::open(&out).unwrap();
    assert_eq!(d2.attachments().len(), 1);
    // Estado restaurado al reabrir: el tipo de documento sigue abierto (lo
    // abre el script al adjuntar y Adobe lo guarda en el paquete form).
    let fila = "xfa.form.Impreso[0].FormContent[0].BDEAttachment[0].ListaAdjuntos[0].table[0].tableRows[0].tableRow[0]";
    assert_eq!(d2.eval_js(&format!("xfa.resolveNode(\"{fila}.bdeAttachDescDoc\").access")).as_deref(), Some("open"));
    // Y es interactivo: el subformSet "tableRows" se sustituye por un subform
    // (PDFium no considera abierto nada dentro de un subformSet).
    assert_eq!(d2.eval_js(&format!("xfa.resolveNode(\"{fila}\").parent.className")).as_deref(), Some("subform"));
}

/// PDF real rellenado con Adobe. Tiene datos personales, así que no está en
/// el repositorio: si no existe, la prueba se omite.
fn pdf_real() {
    let src = &diag("formulario_real.pdf");
    if !src.exists() {
        eprintln!("(sin {}: se omite la prueba con el PDF real)", src.display());
        return;
    }
    let mut doc = Document::open(src).unwrap();
    let alertas = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let a2 = alertas.clone();
    doc.set_alert_handler(Box::new(move |r| {
        a2.borrow_mut().push(r.message.clone());
        form_pdf_reader::pdfium::AlertAnswer::Ok
    }));
    // Abrir no debe cambiar datos: PDFium dispara eventos change al rellenar
    // listas por script y los scripts del formulario borraban valores.
    let antes = String::from_utf8_lossy(&xfa::read_datasets(&std::fs::read(src).unwrap()).unwrap()).into_owned();
    let despues = datasets(&mut doc);
    let norm = |t: &str| t.replace("\n>", ">").replace("\n/>", "/>");
    let (antes, despues) = (norm(&antes), norm(&despues));
    for etiqueta in ["TipoCurso", "bdeDescripcionExtra", "areaDeEstudio", "tipoDeTitulacion", "fechaDeInicio"] {
        // Valores no vacíos (PDFium crea nodos vacíos para campos sin dato).
        let v = |t: &str| t.split(&format!("<{etiqueta}>")).skip(1).map(|r| r.split('<').next().unwrap_or("").to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>();
        assert!(!v(&despues).is_empty(), "falta {etiqueta}");
        assert_eq!(v(&despues), v(&antes), "{etiqueta} cambió al abrir");
    }
    // Validar (último botón del formulario): debe darlo por válido. Se
    // busca desde el final pulsando cada botón hasta que responda.
    let y = doc.content_height(0) as f64;
    let mut dy = 2.0;
    while dy < 400.0 && alertas.borrow().is_empty() {
        if doc.field_at(0, 525.0, y - dy) == 13 {
            click(&mut doc, 525.0, y - dy);
            dy += 10.0;
        }
        dy += 2.0;
    }
    assert!(alertas.borrow().iter().any(|m| m.contains("validado correctamente")), "alertas: {:?}", alertas.borrow());

    // Cambiar el tipo de documento de un adjunto (como el usuario: con el
    // foco en el desplegable) no debe dejar oculta su descripción.
    let fila = "xfa.form.Impreso[0].FormContent[0].BDEAttachment[0].ListaAdjuntos[0].table[0].tableRows[0].tableRow[0]";
    let r = doc.eval_js(&format!(
        "var f = xfa.resolveNode(\"{fila}.bdeAttachDescDoc\"); xfa.host.setFocus(f); f.rawValue = \"Otros\"; f.execEvent(\"change\"); xfa.resolveNode(\"{fila}.bdeDescripcionExtra\").presence"
    ));
    assert_eq!(r.as_deref(), Some("visible"), "la descripción del adjunto quedó oculta");

}

fn titulaciones(doc: &mut Document) -> String {
    let b = doc.bytes_for_save().unwrap();
    let s = String::from_utf8_lossy(&xfa::read_datasets(&b).unwrap()).into_owned();
    let i = s.find("<titulaciones>").unwrap();
    let j = s.find("</titulaciones>").unwrap();
    s[i..j].to_string()
}

/// Fechas (tecleadas y con calendario) y desplegables en cascada de la
/// sección de titulaciones.
fn fechas_y_cascada() {
    let src = &diag("TINTERNET_original.pdf");
    if !src.exists() {
        return;
    }
    // Fecha tecleada: se guarda como DD/MM/AAAA, igual que Adobe (la
    // validación del propio formulario exige ese formato en el dato).
    let mut doc = Document::open(src).unwrap();
    assert!(doc.compat().unwrap().date_fields_fixed >= 2);
    click_mm(&mut doc, 60.0, 1555.0);
    doc.type_text("01/02/2020");
    doc.kill_focus();
    assert!(titulaciones(&mut doc).contains("<fechaDeInicio>01/02/2020</fechaDeInicio>"), "{}", titulaciones(&mut doc));

    drop(doc);

    // Calendario: pulsar la flecha visible (algo fuera del campo) y un día.
    let mut doc = Document::open(src).unwrap();
    click_mm(&mut doc, 60.0, 1555.0);
    click_mm(&mut doc, 99.0, 1558.0);
    assert!(doc.list_open(), "el calendario debería abrirse");
    doc.mouse_move(0, 150.0, 1667.0, 0);
    click_mm(&mut doc, 173.0, 1667.0); // un día de la tercera semana
    doc.kill_focus();
    let t = titulaciones(&mut doc);
    let i = t.find("<fechaDeInicio>").expect("sin fecha") + "<fechaDeInicio>".len();
    let fecha = &t[i..i + 10];
    let f = fecha.as_bytes();
    assert!(f[2] == b'/' && f[5] == b'/' && fecha[6..].chars().all(|c| c.is_ascii_digit()), "fecha del calendario: {t}");
    drop(doc);

    // Cascada Nivel -> Tipo -> Área, con el ratón y sin guardar entre medias.
    let mut doc = Document::open(src).unwrap();
    click_mm(&mut doc, 560.0, 1555.0);
    doc.mouse_move(0, 333.0, 1590.0, 0);
    click_mm(&mut doc, 333.0, 1601.0); // BACHILLERATO
    click_mm(&mut doc, 285.0, 1600.0);
    doc.key_down(keys::VK_DOWN, 0);
    doc.key_up(keys::VK_DOWN, 0);
    doc.key_down(keys::VK_RETURN, 0);
    doc.key_up(keys::VK_RETURN, 0);
    click_mm(&mut doc, 560.0, 1600.0);
    doc.key_down(keys::VK_DOWN, 0);
    doc.key_up(keys::VK_DOWN, 0);
    doc.key_down(keys::VK_RETURN, 0);
    doc.key_up(keys::VK_RETURN, 0);
    doc.kill_focus();
    let t = titulaciones(&mut doc);
    assert!(t.contains("<nivelObtenidoPorElEstudio>3</nivelObtenidoPorElEstudio>"), "{t}");
    assert!(t.contains("<tipoDeTitulacion>2001</tipoDeTitulacion>"), "{t}");
    assert!(t.contains("<areaDeEstudio>2"), "el área de estudio debería rellenarse: {t}");

    // Guardar y reabrir: los eventos change que PDFium dispara al abrir (al
    // rellenar las listas por script) no deben vaciar Tipo ni Área.
    let out = tmp("cascada.pdf");
    doc.save(&out).unwrap();
    drop(doc);
    let mut d2 = Document::open(&out).unwrap();
    let t2 = titulaciones(&mut d2);
    let campo = |t: &str, n: &str| t.split(&format!("<{n}>")).nth(1).and_then(|r| r.split('<').next()).unwrap_or("").to_string();
    for n in ["nivelObtenidoPorElEstudio", "tipoDeTitulacion", "areaDeEstudio"] {
        assert_eq!(campo(&t2, n), campo(&t, n), "{n} cambió al reabrir: {t2}");
    }
}

#[test]
fn pdfium_completo() {
    normal();
    acroform();
    xfa_dinamico();
    desplegable_cierre();
    pdf_real();
    adjuntos();
    fechas_y_cascada();
}
