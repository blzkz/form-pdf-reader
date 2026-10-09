//! Soporte XFA propio de la aplicación (además de lo que hace PDFium):
//! correcciones de compatibilidad y guardado de datos sobre el PDF original.

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
fn is_dynamic(bytes: &[u8]) -> bool {
    let Ok(doc) = lopdf::Document::load_mem(bytes) else { return false };
    let Ok(cat) = doc.catalog() else { return false };
    if let Ok(lopdf::Object::Boolean(true)) = cat.get(b"NeedsRendering") {
        return true;
    }
    false
}

/// Prepara los bytes que verá PDFium. Si el PDF no es XFA o algo falla, se
/// devuelven los bytes originales sin cambios (nunca se impide abrir el PDF).
pub fn prepare(original: &[u8], continuous: bool) -> Prepared {
    let passthrough = |dynamic| Prepared { render_bytes: original.to_vec(), compat: None, dynamic };
    let xfa = match XfaPdf::parse(original) {
        Ok(Some(x)) => x,
        Ok(None) => return passthrough(false),
        Err(e) => {
            log::warn!("No se pudo analizar el XFA: {e:#}");
            return passthrough(false);
        }
    };
    let dynamic = is_dynamic(original);
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
    let Some(patched) = patched else { return passthrough(dynamic) };
    let mut replacements = vec![(tpl.id, patched.into_bytes())];
    if let (Some(fp), Ok(form)) = (xfa.packet("form"), xfa.packet_data("form"))
        && let Some(f) = compat::patch_form_packet(&String::from_utf8_lossy(&form), &report.subformset_names)
    {
        replacements.push((fp.id, f.into_bytes()));
    }
    match pdfedit::replace_streams(original, &xfa, &replacements) {
        Ok(bytes) => {
            log::info!("Compatibilidad XFA aplicada: {report:?}");
            Prepared { render_bytes: bytes, compat: Some(report), dynamic }
        }
        Err(e) => {
            log::warn!("No se pudo aplicar la compatibilidad XFA: {e:#}");
            passthrough(dynamic)
        }
    }
}

/// Extrae el paquete `datasets` (los datos del formulario) de un PDF.
pub fn read_datasets(pdf: &[u8]) -> Result<Vec<u8>> {
    let x = XfaPdf::parse(pdf)?.ok_or_else(|| anyhow!("{}", crate::t!("err.no_xfa")))?;
    x.packet_data("datasets")
}

/// Escribe los datos de `filled` (PDF guardado por PDFium) sobre `base` (el
/// fichero original del usuario) como actualización incremental que solo
/// sustituye el paquete `datasets`.
pub fn merge_datasets(base: &[u8], filled: &[u8]) -> Result<Vec<u8>> {
    build_save(base, filled, None, None)
}

/// Como `merge_datasets`, y además sustituye los adjuntos incrustados por
/// `attachments` y guarda el estado del formulario (`state`, ver
/// `formstate`) si se indican.
pub fn build_save(base: &[u8], filled: &[u8], attachments: Option<&[attachments::Attachment]>, state: Option<&str>) -> Result<Vec<u8>> {
    let raw = read_datasets(filled).context(crate::t!("err.read_data"))?;
    let data = strip_pdfium_newlines(&raw);
    let x = XfaPdf::parse(base)?.ok_or_else(|| anyhow!("{}", crate::t!("err.orig_no_xfa")))?;
    let ds = x.packet("datasets").ok_or_else(|| anyhow!("{}", crate::t!("err.orig_no_datasets")))?.id;
    let mut b = pdfedit::UpdateBuilder::new(base, &x);
    b.set_stream(ds, &lopdf::Dictionary::new(), &data);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quita_saltos_de_pdfium() {
        let x = b"<a x=\"1>2\">\n<b>\nHola</b>\n<c />\n<d>\n</d>\n</a>";
        let y = strip_pdfium_newlines(x);
        assert_eq!(String::from_utf8(y).unwrap(), "<a x=\"1>2\"><b>Hola</b>\n<c />\n<d></d>\n</a>");
    }
}
