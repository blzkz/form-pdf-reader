//! Pruebas de integración con PDFium.
//!
//! PDFium y V8 quedan ligados al hilo que los inicializa, así que todo va en
//! una única función de test (cargo ejecuta cada #[test] en su propio hilo).

use form_pdf_reader::pdfium::{Document, FormKind, keys};
use form_pdf_reader::xfa;
use std::path::{Path, PathBuf};

/// Formulario XFA dinámico real (Banco de España), en blanco.
const XFA: &str = "tests/fixtures/TINTERNET_original.pdf";

/// El mismo formulario rellenado con datos inventados y guardado con Adobe
/// Reader, con dos adjuntos.
const XFA_ADOBE: &str = "tests/fixtures/TINTERNET_adobe.pdf";

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
    let src = Path::new(XFA);
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
    // Con un campo de texto XFA enfocado la app debe mostrar el teclado
    // (PDFium no lo avisa; ver Document::editing_text).
    assert!(!doc.editing_text());
    click(&mut doc, 100.0, 788.0);
    assert!(doc.editing_text(), "campo de texto pulsado");
    doc.type_text("Juan");
    doc.key_down(keys::VK_TAB, 0);
    doc.char_input(9, 0);
    doc.key_up(keys::VK_TAB, 0);
    assert!(doc.editing_text(), "Tab desde un campo de texto");
    doc.type_text("Pérez");
    // Casilla "Hombre" pulsando en su texto.
    click(&mut doc, 286.0, 968.0);
    assert!(!doc.editing_text(), "una casilla no es un campo de texto");
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

/// Anotaciones: se crean, se dibujan, se guardan, se editan y se borran.
fn anotaciones() {
    use form_pdf_reader::pdfium::AnnotKind;
    let mut doc = Document::open(Path::new("tests/fixtures/normal.pdf")).unwrap();
    assert!(doc.can_annotate());
    let antes = doc.render(0, 300, 424).unwrap();
    // Resaltar la primera palabra "zanahoria".
    let hit = doc.search("zanahoria").into_iter().next().unwrap();
    doc.add_markup(hit.page, AnnotKind::Highlight, &hit.rects, [255, 220, 0], "zanahoria").unwrap();
    doc.add_markup(0, AnnotKind::StrikeOut, &[[100.0, 700.0, 200.0, 688.0]], [220, 0, 0], "").unwrap();
    doc.add_note(0, 50.0, 500.0, "Revisar esto", [255, 200, 0]).unwrap();
    doc.add_free_text(0, 60.0, 400.0, "Texto libre\ncon dos líneas ñ", 14.0, [0, 0, 255]).unwrap();
    doc.add_ink(0, &[vec![[100.0, 300.0], [150.0, 320.0], [200.0, 300.0]], vec![[120.0, 280.0]]], 2.0, [0, 80, 200]).unwrap();
    let despues = doc.render(0, 300, 424).unwrap();
    assert_ne!(antes, despues, "las anotaciones deben verse");
    let a = doc.annotations(0);
    let kinds: Vec<AnnotKind> = a.iter().map(|a| a.kind).collect();
    assert!(kinds.contains(&AnnotKind::StrikeOut) && kinds.contains(&AnnotKind::Note) && kinds.contains(&AnnotKind::FreeText) && kinds.contains(&AnnotKind::Ink), "{kinds:?}");
    let texto = a.iter().find(|a| a.kind == AnnotKind::FreeText).unwrap();
    assert_eq!(texto.contents, "Texto libre\ncon dos líneas ñ");
    assert_eq!(texto.font_size, 14.0);
    assert_eq!(texto.color, [0, 0, 255]);
    assert!((texto.rect[0] - 60.0).abs() < 0.1 && (texto.rect[3] - 400.0).abs() < 0.1, "{:?}", texto.rect);
    // La de encima en un punto.
    assert_eq!(doc.annotation_at(0, 55.0, 495.0).map(|a| a.kind), Some(AnnotKind::Note));
    // Guardar y volver a abrir.
    let out = tmp("anotado.pdf");
    doc.save(&out).unwrap();
    drop(doc);
    let mut doc = Document::open(&out).unwrap();
    let n = doc.annotations(0).len();
    assert_eq!(n, kinds.len(), "se conservan al guardar");
    // Editar la nota y el texto libre.
    let nota = doc.annotations(0).into_iter().find(|a| a.kind == AnnotKind::Note).unwrap();
    doc.set_annotation_text(0, nota.index, "Revisado").unwrap();
    let texto = doc.annotations(0).into_iter().find(|a| a.kind == AnnotKind::FreeText).unwrap();
    doc.set_annotation_text(0, texto.index, "Otro texto").unwrap();
    let a = doc.annotations(0);
    assert_eq!(a.iter().find(|a| a.kind == AnnotKind::Note).unwrap().contents, "Revisado");
    let texto = a.iter().find(|a| a.kind == AnnotKind::FreeText).unwrap();
    assert_eq!(texto.contents, "Otro texto");
    assert_eq!(texto.font_size, 14.0);
    assert_eq!(a.len(), n);
    // Borrar.
    doc.remove_annotation(0, texto.index).unwrap();
    assert_eq!(doc.annotations(0).len(), n - 1);
    // En XFA no.
    assert!(!Document::open(Path::new(XFA)).unwrap().can_annotate());
}

/// Organizar páginas: girar, borrar, mover, insertar y extraer (también
/// con un PDF cifrado).
fn paginas() {
    use form_pdf_reader::pdfium::pages::{self, PageEdit};
    let base = std::fs::read("tests/fixtures/normal.pdf").unwrap();
    let otro = std::fs::read("tests/fixtures/acroform.pdf").unwrap();
    let n_otro = pages::page_count(&otro, None).unwrap();
    let texto = |b: &[u8], p: usize| {
        let out = tmp("paginas-tmp.pdf");
        std::fs::write(&out, b).unwrap();
        let d = Document::open(&out).unwrap();
        d.text_range(p, 0, d.text_char_count(p))
    };
    let primera = texto(&base, 0);
    let tercera = texto(&base, 2);
    let b = pages::edit_pages(
        &base,
        None,
        &[
            PageEdit::Rotate { pages: vec![0], quarter_turns: 1 },
            PageEdit::Move { pages: vec![2], to: 0 },
            PageEdit::Insert { pdf: otro.clone(), password: None, at: 3 },
            PageEdit::Delete { pages: vec![1] },
        ],
    )
    .unwrap();
    // [3, 1(girada), 2] + otro al final, y sin la 1 (girada).
    assert_eq!(pages::page_count(&b, None).unwrap(), 2 + n_otro);
    assert_eq!(texto(&b, 0), tercera);
    assert_eq!(pages::rotations(&b, None).unwrap()[0], 0);
    assert!(b.starts_with(&base), "actualización incremental");
    let b = pages::edit_pages(&base, None, &[PageEdit::Rotate { pages: vec![0, 1], quarter_turns: -1 }]).unwrap();
    assert_eq!(&pages::rotations(&b, None).unwrap()[..2], &[3, 3]);
    assert_eq!(texto(&b, 0), primera);
    // No se pueden borrar todas.
    assert!(pages::edit_pages(&base, None, &[PageEdit::Delete { pages: vec![0, 1, 2] }]).is_err());
    // Extraer.
    let e = pages::extract_pages(&base, None, &[2, 0]).unwrap();
    assert_eq!(pages::page_count(&e, None).unwrap(), 2);
    assert_eq!(texto(&e, 0), tercera);
    assert_eq!(texto(&e, 1), primera);
    // Cifrado: sigue pidiendo la contraseña.
    let src = cifrar(Path::new("tests/fixtures/normal.pdf"), "secreto", "paginas-cifrado.pdf");
    let cif = std::fs::read(&src).unwrap();
    assert!(pages::page_count(&cif, None).is_err());
    let b = pages::edit_pages(&cif, Some("secreto"), &[PageEdit::Delete { pages: vec![0] }]).unwrap();
    assert!(pages::page_count(&b, None).is_err(), "debe seguir cifrado");
    assert_eq!(pages::page_count(&b, Some("secreto")).unwrap(), 2);
    // XFA no.
    assert!(!Document::open(Path::new(XFA)).unwrap().can_edit_pages());
    assert!(Document::open(Path::new("tests/fixtures/acroform.pdf")).unwrap().can_edit_pages());
}

/// El PDF de un escaneo: una página por foto, con el tamaño de la foto.
fn escaneo() {
    use form_pdf_reader::scan::{ScanImage, images_to_pdf};
    let jpeg = std::fs::read("tests/fixtures/escaneo.jpg").unwrap();
    let pages = [ScanImage { jpeg: jpeg.clone(), width: 120, height: 160 }, ScanImage { jpeg, width: 120, height: 160 }];
    let pdf = images_to_pdf(&pages).unwrap();
    let out = tmp("escaneo.pdf");
    std::fs::write(&out, &pdf).unwrap();
    let doc = Document::open(&out).unwrap();
    assert_eq!(doc.page_count(), 2);
    let (w, h) = doc.page_size(0);
    assert!((w - 595.28).abs() < 0.1 && (h - 595.28 * 160.0 / 120.0).abs() < 0.1, "{w}x{h}");
    let px = doc.render(0, 120, 160).unwrap();
    // La banda oscura de arriba de la foto.
    assert!(px[(25 * 120 + 60) * 4] < 80, "la foto debe verse");
    assert!(images_to_pdf(&[ScanImage { jpeg: b"no es jpeg".to_vec(), width: 10, height: 10 }]).is_err());
}

/// El modelo de orientación (scripts/descargar-modelos.sh): una página
/// girada se pone derecha. Sin el modelo descargado no se prueba.
#[cfg(feature = "orientation")]
fn orientacion_del_escaneo() {
    use form_pdf_reader::scan::orientation::Orientation;
    let Ok(onnx) = std::fs::read("../../vendor/models/doc_ori.onnx") else {
        eprintln!("sin vendor/models/doc_ori.onnx: no se prueba");
        return;
    };
    let model = Orientation::load(&onnx).unwrap();
    let doc = Document::open(Path::new("tests/fixtures/normal.pdf")).unwrap();
    let (w, h) = (420usize, 594usize);
    let (mut px, mut w, mut h) = (doc.render(0, w as i32, h as i32).unwrap(), w, h);
    for turns in 0..4 {
        let (fix, conf) = model.upright_turns(&px, w, h).unwrap();
        assert_eq!((fix as usize + turns) % 4, 0, "girada {turns} cuartos: corrige {fix} (confianza {conf})");
        assert!(conf > 0.6, "{conf}");
        // Un cuarto de vuelta más a la derecha.
        let mut out = vec![0u8; px.len()];
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = (h - 1 - y, x);
                out[(ny * h + nx) * 4..(ny * h + nx) * 4 + 4].copy_from_slice(&px[(y * w + x) * 4..(y * w + x) * 4 + 4]);
            }
        }
        (px, w, h) = (out, h, w);
    }
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
    let src = Path::new(XFA);
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
    let src = Path::new(XFA);
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
    let atts = xfa::attachments::load(&saved, None).unwrap();
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

/// Formulario guardado con Adobe Reader.
fn pdf_adobe() {
    let src = Path::new(XFA_ADOBE);
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
    let src = Path::new(XFA);
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

/// Valores de los elementos `etiqueta` de un paquete `datasets`.
fn valores(ds: &str, etiqueta: &str) -> Vec<String> {
    let re = regex::Regex::new(&format!(r"<{etiqueta}(?:\s[^>]*?)?\s*>([^<]*)</{etiqueta}")).unwrap();
    re.captures_iter(ds).map(|c| c[1].to_string()).collect()
}

fn ds_de(pdf: &Path) -> String {
    String::from_utf8(xfa::read_datasets(&std::fs::read(pdf).unwrap()).unwrap()).unwrap()
}

/// Saltos de línea de los párrafos: Adobe los guarda como `&#xD;` y PDFium
/// solo dibuja como salto el `\n`. Al abrir se convierten y al guardar se
/// escriben otra vez como Adobe.
fn saltos_de_linea() {
    let src = Path::new(XFA_ADOBE);
    let mut doc = Document::open(src).unwrap();
    let campo = "xfa.form.Impreso.FormContent.Sections.experienciaProfesional.table.section[1].content.line4Subform.tareasYResponsabilidades";
    let r = doc.eval_js(&format!("var v = xfa.resolveNode(\"{campo}\").rawValue; v.indexOf(\"\\r\") + \" \" + v.split(\"\\n\").length")).unwrap();
    let (cr, lineas) = r.split_once(' ').unwrap();
    assert_eq!(cr, "-1", "quedan retornos de carro en el valor");
    assert!(lineas.parse::<usize>().unwrap() > 5, "líneas: {lineas}");

    // Guardar sin cambios: los párrafos quedan igual que los guardó Adobe.
    let out = tmp("saltos.pdf");
    doc.save(&out).unwrap();
    let orig = valores(&ds_de(src), "tareasYResponsabilidades");
    assert!(orig.iter().any(|v| v.contains("&#xD;&#xD;")), "{orig:?}");
    assert_eq!(valores(&ds_de(&out), "tareasYResponsabilidades"), orig);
    drop(doc);

    // Un salto escrito con Intro también se guarda como `&#xD;`.
    let mut doc = Document::open(&out).unwrap();
    click(&mut doc, 300.0, 2700.0);
    doc.key_down(keys::VK_END, keys::MOD_CTRL);
    doc.key_up(keys::VK_END, keys::MOD_CTRL);
    tecla(&mut doc, keys::VK_RETURN);
    doc.type_text("FIN");
    doc.kill_focus();
    let out2 = tmp("saltos2.pdf");
    doc.save(&out2).unwrap();
    let v = valores(&ds_de(&out2), "tareasYResponsabilidades");
    assert!(v[1].ends_with("&#xD;FIN"), "{:?}", v[1]);
    assert!(!v[1].contains('\n') && !v[1].contains('\r'), "{:?}", v[1]);
}

fn tecla(doc: &mut Document, vk: i32) {
    doc.key_down(vk, 0);
    if let Some(c) = keys::char_for_vk(vk) {
        doc.char_input(c as u32, 0);
    }
    doc.key_up(vk, 0);
}

fn arrastrar(doc: &mut Document, a: (f64, f64), b: (f64, f64)) {
    doc.left_down(0, a.0, a.1, 0);
    for i in 1..=10 {
        let t = i as f64 / 10.0;
        doc.mouse_move(0, a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, keys::MOD_LBUTTON);
    }
    doc.left_up(0, b.0, b.1, 0);
}

/// Seleccionar parte del texto de un campo arrastrando el ratón y borrarla
/// o sustituirla, como en Adobe. PDFium no selecciona al arrastrar en XFA y,
/// con texto seleccionado, Retroceso y Supr borraban un solo carácter.
fn seleccion_en_campos() {
    let mut doc = Document::open(Path::new(XFA_ADOBE)).unwrap();
    let len = |d: &Document| d.focused_text().chars().count();
    // Párrafo "Tareas" de la segunda experiencia profesional.
    let (a, b) = ((38.0, 2533.0), (75.0, 2533.0));
    arrastrar(&mut doc, a, b);
    let sel = doc.selected_text();
    assert!(!sel.is_empty() && doc.focused_text().contains(&sel), "selección: {sel:?}");
    let n = len(&doc);
    tecla(&mut doc, keys::VK_BACK);
    assert_eq!(len(&doc), n - sel.chars().count(), "Retroceso debe borrar la selección");

    arrastrar(&mut doc, a, b);
    let (n, k) = (len(&doc), doc.selected_text().chars().count());
    assert!(k > 0);
    doc.type_text("Z");
    assert_eq!(len(&doc), n - k + 1, "escribir debe sustituir la selección");

    arrastrar(&mut doc, a, b);
    let (n, k) = (len(&doc), doc.selected_text().chars().count());
    tecla(&mut doc, keys::VK_DELETE);
    assert_eq!(len(&doc), n - k, "Supr debe borrar la selección");
    assert_eq!(doc.selected_text(), "");

    // Arrastrar hasta fuera del campo: selecciona hasta el final sin perder
    // el foco (un clic fuera se lo daría a otro campo).
    arrastrar(&mut doc, a, (100.0, 4000.0));
    assert!(len(&doc) > 0, "el campo perdió el foco");
    assert!(doc.focused_text().ends_with(&doc.selected_text()) && !doc.selected_text().is_empty());

    // Un clic sin mover el ratón solo coloca el cursor.
    doc.left_down(0, b.0, b.1, 0);
    doc.left_up(0, b.0, b.1, 0);
    assert_eq!(doc.selected_text(), "");
    doc.kill_focus();
}

/// Un punto (en coordenadas de página) de cada campo de texto de la
/// primera página, en orden de lectura.
fn campos_de_texto(doc: &Document) -> Vec<(f64, f64)> {
    let (w, _) = doc.page_size(0);
    let h = doc.content_height(0);
    let mut puntos = Vec::new();
    let mut zonas: Vec<[f64; 4]> = Vec::new();
    let mut y = 4.0;
    while y < h as f64 {
        let mut x = 4.0;
        while x < w as f64 {
            let dentro = zonas.iter().any(|b| x >= b[0] && x <= b[2] && y >= b[1].min(b[3]) && y <= b[1].max(b[3]));
            if !dentro && matches!(doc.field_at(0, x, y), 6 | 15) {
                zonas.push(doc.field_bounds(0, x, y));
                puntos.push((x + 3.0, y + 2.0));
            }
            x += 8.0;
        }
        y += 6.0;
    }
    puntos
}

/// Escribe `texto` en el primer campo de texto de la primera página que lo
/// acepta (algunos son fechas o tienen formato) y lo deja guardado en el
/// formulario. Devuelve dónde.
fn rellenar_un_campo(doc: &mut Document, texto: &str, xfa_datos: bool) -> Option<(f64, f64)> {
    for (x, y) in campos_de_texto(doc).into_iter().take(20) {
        click(doc, x, y);
        doc.select_all();
        doc.type_text(texto);
        doc.kill_focus();
        let ok = if xfa_datos {
            datasets(doc).contains(texto)
        } else {
            click(doc, x, y);
            let t = doc.focused_text();
            doc.kill_focus();
            t == texto
        };
        if ok {
            return Some((x, y));
        }
    }
    None
}

/// Formularios públicos de distintos organismos (ver fixtures/README.md):
/// se abren con su tipo, se dibujan, se rellena un campo, se guarda como
/// actualización incremental y el dato se lee al reabrir. Dos están cifrados
/// (contraseña de propietario).
fn corpus() {
    let casos = [
        ("hmrc-chv1.pdf", FormKind::XfaFull),
        ("us-army-da4856.pdf", FormKind::XfaFull),
        ("ec-mir-7.3.1.pdf", FormKind::XfaFull),
        ("gencat-22100.pdf", FormKind::XfaFull),
        ("irs-f1040x.pdf", FormKind::XfaForeground),
        ("uscis-i9.pdf", FormKind::AcroForm),
    ];
    let texto = "12345";
    for (f, tipo) in casos {
        let src = Path::new("tests/fixtures/corpus").join(f);
        let mut doc = Document::open(&src).unwrap();
        doc.set_alert_handler(Box::new(|_| form_pdf_reader::pdfium::AlertAnswer::Ok));
        assert_eq!(doc.kind(), tipo, "{f}");
        let xfa = tipo != FormKind::AcroForm;
        assert_eq!(doc.is_xfa(), xfa, "{f}: XFA cargado");
        // La primera página tiene contenido (no solo el aviso de "espere").
        let (w, h) = doc.page_size(0);
        let px = doc.render(0, (w / 4.0) as i32, (h / 4.0) as i32).unwrap();
        let tinta = px.chunks(4).filter(|c| c[0] < 128).count();
        assert!(tinta > 200, "{f}: página casi en blanco ({tinta})");
        // En los XFA estáticos PDFium escribe en la capa AcroForm (el valor se
        // pasa a `datasets` al guardar; ver xfa::acrosync).
        let dinamico = tipo == FormKind::XfaFull;
        let (x, y) = rellenar_un_campo(&mut doc, texto, dinamico).unwrap_or_else(|| panic!("{f}: ningún campo de texto aceptó el texto"));

        let out = tmp(&format!("corpus-{f}"));
        doc.save(&out).unwrap();
        drop(doc);
        let orig = std::fs::read(&src).unwrap();
        assert!(std::fs::read(&out).unwrap().starts_with(&orig), "{f}: el original debe quedar intacto");
        let mut d2 = Document::open(&out).unwrap();
        d2.set_alert_handler(Box::new(|_| form_pdf_reader::pdfium::AlertAnswer::Ok));
        assert_eq!(d2.is_xfa(), xfa, "{f}: XFA cargado al reabrir");
        if dinamico {
            assert!(datasets(&mut d2).contains(texto), "{f}: el dato no está al reabrir");
        }
        // En XFA, el dato debe estar en `datasets`, que es lo que lee Adobe
        // (en los estáticos PDFium solo lo deja en la capa AcroForm).
        if xfa {
            let ds = String::from_utf8_lossy(&xfa::read_datasets(&std::fs::read(&out).unwrap()).unwrap()).into_owned();
            assert!(ds.contains(&format!(">{texto}</")), "{f}: el dato no está en datasets");
        }
        click(&mut d2, x, y);
        assert_eq!(d2.focused_text(), texto, "{f}: el campo no muestra el dato al reabrir");
        d2.kill_focus();
    }
}

/// Copia de `src` cifrada (AES-256) con contraseña para abrirla.
fn cifrar(src: &Path, contraseña: &str, nombre: &str) -> PathBuf {
    use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
    use lopdf::{EncryptionState, EncryptionVersion, Permissions};
    let mut doc = lopdf::Document::load(src).unwrap();
    let filtro: std::sync::Arc<dyn CryptFilter> = std::sync::Arc::new(Aes256CryptFilter);
    let clave = [7u8; 32];
    let version = EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: std::collections::BTreeMap::from([(b"StdCF".to_vec(), filtro)]),
        file_encryption_key: &clave,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: "propietario",
        user_password: contraseña,
        permissions: Permissions::all(),
    };
    doc.encrypt(&EncryptionState::try_from(version).unwrap()).unwrap();
    let out = tmp(nombre);
    doc.save(&out).unwrap();
    out
}

fn abrir_con(path: &Path, contraseña: Option<&str>) -> anyhow::Result<Document> {
    Document::open_with(path, form_pdf_reader::pdfium::OpenOptions { continuous_xfa: true, password: contraseña.map(str::to_string) })
}

fn pide_contraseña(r: anyhow::Result<Document>) -> Option<bool> {
    r.err().and_then(|e| e.downcast_ref::<form_pdf_reader::pdfium::PasswordRequired>().map(|p| p.wrong))
}

/// PDF que piden contraseña para abrirlos: se pide, se avisa si no es
/// correcta y, al guardar, el PDF sigue cifrado con la misma contraseña.
fn contraseña_de_usuario() {
    // AcroForm.
    let src = cifrar(Path::new("tests/fixtures/acroform.pdf"), "secreto", "acro-cifrado.pdf");
    assert_eq!(pide_contraseña(abrir_con(&src, None)), Some(false), "debe pedir contraseña");
    assert_eq!(pide_contraseña(abrir_con(&src, Some("otra"))), Some(true), "contraseña incorrecta");
    let mut doc = abrir_con(&src, Some("secreto")).unwrap();
    assert_eq!(doc.kind(), FormKind::AcroForm);
    doc.left_down(0, 300.0, 731.0, 0);
    doc.left_up(0, 300.0, 731.0, 0);
    doc.type_text("Cifrado");
    doc.kill_focus();
    let out = tmp("acro-cifrado-guardado.pdf");
    doc.save(&out).unwrap();
    drop(doc);
    assert_eq!(pide_contraseña(abrir_con(&out, None)), Some(false), "el guardado debe seguir cifrado");
    let mut d2 = abrir_con(&out, Some("secreto")).unwrap();
    d2.left_down(0, 300.0, 731.0, 0);
    d2.left_up(0, 300.0, 731.0, 0);
    assert_eq!(d2.focused_text(), "Cifrado");
    d2.kill_focus();
    drop(d2);

    // XFA dinámico: el núcleo descifra la plantilla y los datos con la
    // contraseña y cifra lo que añade al guardar.
    let src = cifrar(Path::new(XFA), "secreto", "xfa-cifrado.pdf");
    assert_eq!(pide_contraseña(abrir_con(&src, None)), Some(false));
    let mut doc = abrir_con(&src, Some("secreto")).unwrap();
    assert!(doc.is_xfa() && doc.is_continuous(), "XFA con sus correcciones");
    click(&mut doc, 100.0, 788.0);
    doc.type_text("Ana");
    doc.kill_focus();
    let out = tmp("xfa-cifrado-guardado.pdf");
    doc.save(&out).unwrap();
    drop(doc);
    assert!(std::fs::read(&out).unwrap().starts_with(&std::fs::read(&src).unwrap()), "actualización incremental");
    assert_eq!(pide_contraseña(abrir_con(&out, None)), Some(false));
    let ds = xfa::read_datasets_with(&std::fs::read(&out).unwrap(), Some("secreto")).unwrap();
    assert!(String::from_utf8_lossy(&ds).contains("<nombre>ANA</nombre>"), "datos guardados");
    let mut d2 = abrir_con(&out, Some("secreto")).unwrap();
    assert!(d2.is_xfa());
    assert!(datasets_con(&mut d2, "secreto").contains("<nombre>ANA</nombre>"));
}

fn datasets_con(doc: &mut Document, contraseña: &str) -> String {
    let b = doc.bytes_for_save().unwrap();
    String::from_utf8_lossy(&xfa::read_datasets_with(&b, Some(contraseña)).unwrap()).into_owned()
}

/// Píxeles de tinta azul de firma en un rectángulo de página (izquierda,
/// abajo, derecha, arriba), renderizando a 1 píxel por punto.
fn tinta(doc: &Document, page: usize, r: [f32; 4]) -> usize {
    let (w, h) = doc.page_size(page);
    let px = doc.render_region(page, w as i32, h as i32, 0, 0, w as i32, h as i32).unwrap();
    let mut n = 0;
    for y in (h - r[3]) as usize..(h - r[1]) as usize {
        for x in r[0] as usize..r[2] as usize {
            let i = (y * w as usize + x) * 4;
            let (cr, cb) = (px[i] as i32, px[i + 2] as i32);
            if cb > cr + 20 && cr < 140 {
                n += 1;
            }
        }
    }
    n
}

fn firmas_manuscritas() {
    use form_pdf_reader::pdfium::{render_signature, signature_size};
    use form_pdf_reader::sign::{Library, SignatureKind, Stroke, TypeStyle, library::DEFAULT_INK};
    let lib = Library::new(tmp("firmas"));
    let escrita = lib.add(SignatureKind::Typed { text: "Juan Pérez".into(), style: TypeStyle::GreatVibes }, DEFAULT_INK).unwrap();
    let dibujada = lib
        .add(SignatureKind::Drawn { strokes: vec![Stroke { width: 4.0, points: vec![(0.0, 30.0), (40.0, 0.0), (80.0, 30.0), (120.0, 0.0)] }] }, DEFAULT_INK)
        .unwrap();

    // Vista previa con fondo transparente.
    let (w, h, px) = render_signature(&escrita, 40).unwrap();
    assert_eq!(px.len(), (w * h * 4) as usize);
    assert!(w > 3 * h);
    assert!(px.chunks(4).any(|c| c[3] == 0) && px.chunks(4).any(|c| c[3] > 200), "fondo transparente y trazo opaco");

    // PDF normal: poner, mover, guardar y volver a abrir.
    let mut doc = Document::open(Path::new("tests/fixtures/normal.pdf")).unwrap();
    assert!(doc.can_sign());
    let (sw, sh) = signature_size(&escrita);
    let a = [60.0, 40.0, 60.0 + sw, 40.0 + sh];
    let b = [300.0, 40.0, 300.0 + sw, 40.0 + sh];
    assert_eq!(tinta(&doc, 0, a), 0);
    let i = doc.add_signature(0, a, &escrita).unwrap();
    doc.move_signature(i, b).unwrap();
    let c = [100.0, 120.0, 260.0, 160.0];
    let j = doc.add_signature(1, c, &dibujada).unwrap();
    doc.remove_signature(j);
    let d = [100.0, 220.0, 260.0, 260.0];
    doc.add_signature(2, d, &dibujada).unwrap();
    assert_eq!(doc.placed_signatures().len(), 2);
    let out = tmp("firmado.pdf");
    doc.save(&out).unwrap();
    assert!(doc.placed_signatures().is_empty(), "al guardar pasan al PDF");
    assert!(tinta(&doc, 0, b) > 200);
    drop(doc);
    // Una anotación por firma que queda, sin copias de las movidas o quitadas.
    let bytes = std::fs::read(&out).unwrap();
    let cuenta = |n: &[u8]| bytes.windows(n.len()).filter(|w| *w == n).count();
    assert_eq!(cuenta(b"/Subtype/Stamp") + cuenta(b"/Subtype /Stamp"), 2);
    let doc = Document::open(&out).unwrap();
    assert!(tinta(&doc, 0, b) > 200, "la firma se guarda en el PDF");
    assert_eq!(tinta(&doc, 0, a), 0, "en el sitio al que se movió");
    assert_eq!(tinta(&doc, 1, c), 0, "la quitada no está");
    assert!(tinta(&doc, 2, d) > 100);
    drop(doc);

    // AcroForm: igual.
    let mut doc = Document::open(Path::new("tests/fixtures/acroform.pdf")).unwrap();
    doc.add_signature(0, [60.0, 40.0, 60.0 + sw, 40.0 + sh], &escrita).unwrap();
    let out = tmp("acro-firmado.pdf");
    doc.save(&out).unwrap();
    drop(doc);
    let doc = Document::open(&out).unwrap();
    assert!(tinta(&doc, 0, [60.0, 40.0, 60.0 + sw, 40.0 + sh]) > 200);
    drop(doc);

    // XFA dinámico: no se puede.
    let mut doc = Document::open(Path::new(XFA)).unwrap();
    assert!(!doc.can_sign());
    assert!(doc.add_signature(0, a, &escrita).is_err());
}

fn firmas_digitales() {
    use form_pdf_reader::pdfium::SignedState;
    let src = Path::new("tests/fixtures/firmado-pdfjs.pdf");
    let doc = Document::open(src).unwrap();
    let f = doc.digital_signatures();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].signer.as_deref(), Some("test-pdf-signer"));
    assert_eq!(f[0].issuer.as_deref(), Some("pdf-sign-ca"));
    assert_eq!(f[0].sub_filter.as_deref(), Some("adbe.pkcs7.detached"));
    assert!(f[0].reason.as_deref().unwrap_or("").starts_with("Test signature"));
    assert_eq!(f[0].certification, None);
    assert_eq!(f[0].state, SignedState::Intact);
    assert!(!doc.has_usage_rights());
    drop(doc);

    // Un byte cambiado dentro de lo firmado, y cambios añadidos después.
    let b = std::fs::read(src).unwrap();
    let mut alterado = b.clone();
    alterado[101] = alterado[101].wrapping_add(1);
    let p = tmp("firmado-alterado.pdf");
    std::fs::write(&p, &alterado).unwrap();
    assert_eq!(Document::open(&p).unwrap().digital_signatures()[0].state, SignedState::Broken);
    let sx = b.windows(9).rposition(|w| w == b"startxref").unwrap();
    let mut ampliado = b.clone();
    ampliado.extend_from_slice(b"\n% cambio posterior\n");
    ampliado.extend_from_slice(&b[sx..]);
    std::fs::write(&p, &ampliado).unwrap();
    assert_eq!(Document::open(&p).unwrap().digital_signatures()[0].state, SignedState::ChangedAfter);

    // Derechos de uso de Adobe (formulario del Banco de España).
    assert!(Document::open(Path::new(XFA)).unwrap().has_usage_rights());
}

#[test]
fn pdfium_completo() {
    firmas_digitales();
    firmas_manuscritas();
    normal();
    anotaciones();
    paginas();
    escaneo();
    #[cfg(feature = "orientation")]
    orientacion_del_escaneo();
    acroform();
    xfa_dinamico();
    desplegable_cierre();
    pdf_adobe();
    saltos_de_linea();
    seleccion_en_campos();
    corpus();
    contraseña_de_usuario();
    adjuntos();
    fechas_y_cascada();
}

/// Firma digital PAdES con los certificados de prueba (RSA y EC): invisible,
/// en un campo nuevo visible y sobre un documento ya firmado. Las firmas se
/// leen como intactas y, si está poppler, `pdfsig` las da por válidas.
#[test]
fn firma_digital_pades() {
    use form_pdf_reader::pades::{self, P12Key, Placement, Request};
    use form_pdf_reader::pdfium::SignedState;
    let rsa = std::fs::read("tests/fixtures/certs/prueba-rsa.p12").unwrap();
    let ec = std::fs::read("tests/fixtures/certs/prueba-ec-3des.p12").unwrap();
    assert!(P12Key::open(&rsa, "mala").is_err());
    let mut k_rsa = P12Key::open(&rsa, "prueba").expect("PKCS#12 RSA (AES)");
    let mut k_ec = P12Key::open(&ec, "prueba").expect("PKCS#12 EC (3DES)");
    let info = pades::cert_info(&pades::Signer::chain(&k_rsa)[0]).unwrap();
    assert_eq!(info.subject, "PEREZ GARCIA JUAN - 00000000T (PRUEBA)");
    assert_eq!(info.holder_id.as_deref(), Some("IDCES-00000000T"));
    assert!(info.for_signing && !info.expired);

    let casos: [(&str, Placement, bool, usize); 3] = [
        ("tests/fixtures/normal.pdf", Placement::Invisible, false, 1),
        ("tests/fixtures/acroform.pdf", Placement::New { page: 0, rect: [72.0, 72.0, 272.0, 132.0] }, true, 1),
        ("tests/fixtures/firmado-pdfjs.pdf", Placement::New { page: 0, rect: [300.0, 600.0, 500.0, 660.0] }, false, 2),
    ];
    for (i, (src, placement, use_ec, firmas)) in casos.into_iter().enumerate() {
        let key = if use_ec { &mut k_ec } else { &mut k_rsa };
        let base = std::fs::read(src).unwrap();
        let req = Request { placement, reason: Some("Prueba de firma ñ".into()), location: Some("Madrid".into()), handwritten: None, timestamp: None, lock: false };
        let firmado = pades::sign(&base, None, &req, key).unwrap_or_else(|e| panic!("{src}: {e:#}"));
        assert!(firmado.starts_with(&base), "{src}: actualización incremental");
        let p = tmp(&format!("pades-{i}.pdf"));
        std::fs::write(&p, &firmado).unwrap();
        let doc = Document::open(&p).unwrap();
        let f = doc.digital_signatures();
        assert_eq!(f.len(), firmas, "{src}");
        let nueva = f.last().unwrap();
        assert_eq!(nueva.sub_filter.as_deref(), Some("ETSI.CAdES.detached"), "{src}");
        assert_eq!(nueva.state, SignedState::Intact, "{src}");
        assert!(nueva.signer.as_deref().unwrap_or("").contains("(PRUEBA)"), "{src}");
        // Las anteriores siguen intactas... salvo que cubren hasta su final:
        // ahora hay una actualización después.
        if firmas > 1 {
            assert_eq!(f[0].state, SignedState::ChangedAfter, "{src}");
        }
        let campos = pades::signature_fields(&firmado, None).unwrap();
        assert!(campos.iter().filter(|c| c.signed).count() >= 1, "{src}");
        if let Ok(out) = std::process::Command::new("pdfsig").arg(&p).output() {
            let s = String::from_utf8_lossy(&out.stdout);
            assert!(s.contains("Signature Validation: Signature is Valid."), "{src}: pdfsig:\n{s}");
        }
    }
}

/// Firmar bloqueando el documento: la primera firma es de certificación
/// (DocMDP 1); sobre un documento ya firmado, bloquea el campo (/Lock /P 1).
#[test]
fn firma_digital_bloqueo() {
    use form_pdf_reader::pades::{self, P12Key, Placement, Request};
    let mut key = P12Key::open(&std::fs::read("tests/fixtures/certs/prueba-rsa.p12").unwrap(), "prueba").unwrap();
    for (i, (src, certifica)) in [("tests/fixtures/normal.pdf", true), ("tests/fixtures/firmado-pdfjs.pdf", false)].into_iter().enumerate() {
        let base = std::fs::read(src).unwrap();
        assert_eq!(pades::lock_permission(&base, None), None, "{src}");
        let req = Request { placement: Placement::Invisible, reason: None, location: None, handwritten: None, timestamp: None, lock: true };
        let firmado = pades::sign(&base, None, &req, &mut key).unwrap();
        assert_eq!(pades::lock_permission(&firmado, None), Some(1), "{src}");
        let p = tmp(&format!("bloqueado-{i}.pdf"));
        std::fs::write(&p, &firmado).unwrap();
        let doc = Document::open(&p).unwrap();
        assert!(doc.locked_by_signature(), "{src}");
        let firmas = doc.digital_signatures();
        assert_eq!(firmas.last().unwrap().certification, certifica.then_some(1), "{src}");
        if let Ok(out) = std::process::Command::new("pdfsig").arg(&p).output() {
            let s = String::from_utf8_lossy(&out.stdout);
            assert!(s.matches("Signature is Valid.").count() >= 1, "{src}: pdfsig:\n{s}");
        }
    }
}
