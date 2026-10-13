//! Soporte XFA propio de la aplicación (además de lo que hace PDFium):
//! correcciones de compatibilidad y guardado de datos sobre el PDF original.

pub mod acrosync;
pub mod attachments;
pub mod compat;
pub mod formstate;
pub mod pdfedit;

use anyhow::{Context, Result, anyhow};
pub use compat::CompatReport;
use pdfedit::XfaPdf;

/// Resultado de preparar un PDF para PDFium.
pub struct Prepared {
    /// Bytes que se entregan a PDFium (original + parches en memoria).
    pub render_bytes: Vec<u8>,
    /// Informe de correcciones aplicadas (None si no se tocó nada).
    pub compat: Option<CompatReport>,
    /// true si es un XFA dinámico (las páginas las genera XFA).
    pub dynamic: bool,
}

/// Detecta XFA dinámico: /NeedsRendering true o AcroForm sin campos.
fn is_dynamic(bytes: &[u8], password: Option<&str>) -> bool {
    let Ok(doc) = pdfedit::load(bytes, password) else { return false };
    let Ok(cat) = doc.catalog() else { return false };
    if let Ok(lopdf::Object::Boolean(true)) = cat.get(b"NeedsRendering") {
        return true;
    }
    false
}

/// Prepara los bytes que verá PDFium. Si el PDF no es XFA o algo falla, se
/// devuelven los bytes originales sin cambios (nunca se impide abrir el PDF).
pub fn prepare(original: &[u8], continuous: bool, password: Option<&str>) -> Prepared {
    let passthrough = |dynamic| Prepared { render_bytes: original.to_vec(), compat: None, dynamic };
    let xfa = match XfaPdf::parse_with(original, password) {
        Ok(Some(x)) => x,
        Ok(None) => return passthrough(false),
        Err(e) => {
            log::warn!("No se pudo analizar el XFA: {e:#}");
            return passthrough(false);
        }
    };
    let dynamic = is_dynamic(original, password);
    let Some(tpl) = xfa.packet("template").cloned() else { return passthrough(dynamic) };
    let template = match xfa.packet_data("template") {
        Ok(d) => String::from_utf8_lossy(&d).into_owned(),
        Err(e) => {
            log::warn!("No se pudo leer la plantilla XFA: {e:#}");
            return passthrough(dynamic);
        }
    };
    let state = formstate::for_open(&xfa);
    let (patched, report) = compat::patch_template_with_state(&template, continuous && dynamic, state.as_deref());
    let mut replacements = Vec::new();
    if let Some(patched) = patched {
        replacements.push((tpl.id, patched.into_bytes()));
        if let (Some(fp), Ok(form)) = (xfa.packet("form"), xfa.packet_data("form"))
            && let Some(f) = compat::patch_form_packet(&String::from_utf8_lossy(&form), &report.subformset_names)
        {
            replacements.push((fp.id, f.into_bytes()));
        }
    }
    let template_patched = !replacements.is_empty();
    if let (Some(dp), Ok(ds)) = (xfa.packet("datasets"), xfa.packet_data("datasets"))
        && let Some(d) = breaks_for_pdfium(&ds)
    {
        replacements.push((dp.id, d));
    }
    if replacements.is_empty() {
        return passthrough(dynamic);
    }
    match pdfedit::replace_streams(original, &xfa, &replacements) {
        Ok(bytes) => {
            log::info!("Compatibilidad XFA aplicada: {report:?}");
            Prepared { render_bytes: bytes, compat: template_patched.then_some(report), dynamic }
        }
        Err(e) => {
            log::warn!("No se pudo aplicar la compatibilidad XFA: {e:#}");
            passthrough(dynamic)
        }
    }
}

/// Extrae el paquete `datasets` (los datos del formulario) de un PDF.
pub fn read_datasets(pdf: &[u8]) -> Result<Vec<u8>> {
    read_datasets_with(pdf, None)
}

/// Como `read_datasets`, para un PDF que pide `password` para abrirlo.
pub fn read_datasets_with(pdf: &[u8], password: Option<&str>) -> Result<Vec<u8>> {
    let x = XfaPdf::parse_with(pdf, password)?.ok_or_else(|| anyhow!("{}", crate::t!("err.no_xfa")))?;
    x.packet_data("datasets")
}

/// Escribe los datos de `filled` (PDF guardado por PDFium) sobre `base` (el
/// fichero original del usuario) como actualización incremental que solo
/// sustituye el paquete `datasets`.
pub fn merge_datasets(base: &[u8], filled: &[u8]) -> Result<Vec<u8>> {
    build_save(base, filled, None, None, None)
}

/// Como `merge_datasets`, y además sustituye los adjuntos incrustados por
/// `attachments` y guarda el estado del formulario (`state`, ver
/// `formstate`) si se indican.
pub fn build_save(base: &[u8], filled: &[u8], attachments: Option<&[attachments::Attachment]>, state: Option<&str>, password: Option<&str>) -> Result<Vec<u8>> {
    let raw = read_datasets_with(filled, password).context(crate::t!("err.read_data"))?;
    let data = breaks_like_adobe(&strip_pdfium_newlines(&raw));
    let x = XfaPdf::parse_with(base, password)?.ok_or_else(|| anyhow!("{}", crate::t!("err.orig_no_xfa")))?;
    let ds = x.packet("datasets").ok_or_else(|| anyhow!("{}", crate::t!("err.orig_no_datasets")))?.id;
    let mut b = pdfedit::UpdateBuilder::new(base, &x);
    b.set_stream(ds, &lopdf::Dictionary::new(), &data)?;
    if let Some(atts) = attachments {
        attachments::write(&mut b, x.document(), atts)?;
    }
    if let Some(json) = state {
        formstate::write(&mut b, x.document(), json, &data)?;
    }
    b.finish()
}

/// El serializador XML de PDFium añade un salto de línea justo después de
/// cada etiqueta de apertura (`<nombre>\nJUAN</nombre>`). Dentro de un
/// elemento de texto ese salto pasa a formar parte del valor, así que se
/// elimina exactamente uno tras cada etiqueta de apertura.
pub fn strip_pdfium_newlines(xml: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut i = 0;
    while i < xml.len() {
        let c = xml[i];
        if c == b'<' && i + 1 < xml.len() && !matches!(xml[i + 1], b'/' | b'?' | b'!') {
            // Etiqueta de apertura: copiar hasta su '>' respetando comillas.
            let mut j = i + 1;
            let mut quote: Option<u8> = None;
            while j < xml.len() {
                let d = xml[j];
                match quote {
                    Some(q) if d == q => quote = None,
                    Some(_) => {}
                    None if d == b'"' || d == b'\'' => quote = Some(d),
                    None if d == b'>' => break,
                    None => {}
                }
                j += 1;
            }
            let end = (j + 1).min(xml.len());
            out.extend_from_slice(&xml[i..end]);
            let self_closing = j > 0 && xml[j - 1] == b'/';
            i = end;
            if !self_closing && i < xml.len() && xml[i] == b'\n' {
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Adobe guarda los saltos de línea de los campos de varias líneas como
/// retorno de carro (`&#xD;`), y PDFium solo dibuja como salto el `\n`: el
/// párrafo salía en un solo bloque. Para PDFium se cambian por `\n` (los
/// saltos "\r\n" quedan en uno solo). Devuelve None si no hay ninguno.
pub fn breaks_for_pdfium(ds: &[u8]) -> Option<Vec<u8>> {
    let cr = regex::bytes::Regex::new(r"(?i)(?:&#x0*d;|&#0*13;|\r)(?:&#x0*a;|&#0*10;|\n)?").unwrap();
    if !cr.is_match(ds) {
        return None;
    }
    Some(cr.replace_all(ds, &b"&#xA;"[..]).into_owned())
}

/// Escribe los saltos de línea de los valores como Adobe (`&#xD;`). PDFium
/// los serializa como bytes sueltos (`\n`, o `\r` si vinieron así), y un
/// lector XML convierte un `\r` literal en `\n`. Solo se tocan los valores
/// (texto entre una etiqueta de apertura y su cierre, sin hijos); los saltos
/// entre etiquetas son formato y se quedan como están. Recibe el XML ya
/// pasado por `strip_pdfium_newlines`.
pub fn breaks_like_adobe(xml: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    // true si lo último fue una etiqueta de apertura (no autocerrada).
    let mut after_open = false;
    let mut i = 0;
    while i < xml.len() {
        if xml[i] == b'<' {
            let end = tag_end(xml, i);
            let tag = &xml[i..end];
            after_open = !matches!(tag.get(1), Some(b'/' | b'?' | b'!')) && !tag.ends_with(b"/>");
            out.extend_from_slice(tag);
            i = end;
            continue;
        }
        let next = xml[i..].iter().position(|&c| c == b'<').map_or(xml.len(), |p| i + p);
        let text = &xml[i..next];
        if after_open && xml[next..].starts_with(b"</") {
            let mut j = 0;
            while j < text.len() {
                match text[j] {
                    b'\r' => {
                        out.extend_from_slice(b"&#xD;");
                        if text.get(j + 1) == Some(&b'\n') {
                            j += 1;
                        }
                    }
                    b'\n' => out.extend_from_slice(b"&#xD;"),
                    c => out.push(c),
                }
                j += 1;
            }
        } else {
            out.extend_from_slice(text);
        }
        i = next;
    }
    out
}

/// Fin (exclusivo) de la etiqueta que empieza en `start`, respetando comillas.
fn tag_end(xml: &[u8], start: usize) -> usize {
    let mut quote: Option<u8> = None;
    let mut j = start + 1;
    while j < xml.len() {
        let d = xml[j];
        match quote {
            Some(q) if d == q => quote = None,
            Some(_) => {}
            None if d == b'"' || d == b'\'' => quote = Some(d),
            None if d == b'>' => return j + 1,
            None => {}
        }
        j += 1;
    }
    xml.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quita_saltos_de_pdfium() {
        let x = b"<a x=\"1>2\">\n<b>\nHola</b>\n<c />\n<d>\n</d>\n</a>";
        let y = strip_pdfium_newlines(x);
        assert_eq!(String::from_utf8(y).unwrap(), "<a x=\"1>2\"><b>Hola</b>\n<c />\n<d></d>\n</a>");
    }

    #[test]
    fn saltos_de_adobe_para_pdfium() {
        let x = b"<a><b>UNO&#xD;DOS&#xd;&#xA;TRES&#13;CUATRO\r\nCINCO\rSEIS\nSIETE</b></a>";
        let y = breaks_for_pdfium(x).unwrap();
        assert_eq!(String::from_utf8(y).unwrap(), "<a><b>UNO&#xA;DOS&#xA;TRES&#xA;CUATRO&#xA;CINCO&#xA;SEIS\nSIETE</b></a>");
        assert!(breaks_for_pdfium(b"<a><b>X\nY</b></a>").is_none());
    }

    #[test]
    fn saltos_como_adobe_al_guardar() {
        let x = b"<a>\n<b x=\"1>2\">UNO\nDOS\r\nTRES\rCUATRO</b>\n<c/>\n<d>\n</d>\n</a>";
        let y = breaks_like_adobe(x);
        assert_eq!(String::from_utf8(y).unwrap(), "<a>\n<b x=\"1>2\">UNO&#xD;DOS&#xD;TRES&#xD;CUATRO</b>\n<c/>\n<d>&#xD;</d>\n</a>");
    }
}
