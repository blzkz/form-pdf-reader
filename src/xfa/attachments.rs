//! Ficheros adjuntos incrustados en el PDF (árbol de nombres /EmbeddedFiles).
//!
//! Es lo que hace Acrobat con `doc.importDataObject(nombre)`: cada adjunto es
//! un stream /EmbeddedFile referenciado por un /Filespec, y el árbol de
//! nombres del catálogo los indexa por `nombre` (el identificador que usa el
//! formulario, p. ej. "1", "2"...).

use super::pdfedit::UpdateBuilder;
use anyhow::{Context, Result};
use lopdf::{Dictionary, Object};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// Clave en el árbol de nombres (cName de Acrobat).
    pub name: String,
    /// Nombre del fichero (propiedad `path` de Acrobat).
    pub file_name: String,
    pub data: Vec<u8>,
}

/// Decodifica una cadena de texto PDF (UTF-16BE con BOM o PDFDocEncoding).
fn text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Codifica como cadena de texto PDF: ASCII tal cual, o UTF-16BE con BOM.
fn pdf_text(s: &str) -> Vec<u8> {
    if s.is_ascii() {
        s.as_bytes().to_vec()
    } else {
        let mut v = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_be_bytes());
        }
        v
    }
}

fn resolve<'a>(doc: &'a lopdf::Document, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(r) => doc.get_object(*r).ok(),
        o => Some(o),
    }
}

fn dict<'a>(doc: &'a lopdf::Document, o: &'a Object) -> Option<&'a Dictionary> {
    resolve(doc, o).and_then(|o| o.as_dict().ok())
}

/// Recorre un árbol de nombres (con /Names y /Kids).
fn walk_names(doc: &lopdf::Document, node: &Dictionary, out: &mut Vec<(String, Object)>, depth: usize) {
    if depth > 32 {
        return;
    }
    if let Ok(Object::Array(a)) = node.get(b"Names") {
        for pair in a.chunks(2) {
            if let [Object::String(k, _), v] = pair {
                out.push((text(k), v.clone()));
            }
        }
    }
    if let Ok(Object::Array(kids)) = node.get(b"Kids") {
        for k in kids {
            if let Some(d) = dict(doc, k) {
                walk_names(doc, d, out, depth + 1);
            }
        }
    }
}

/// Lee los adjuntos incrustados de un PDF.
pub fn load(pdf: &[u8]) -> Result<Vec<Attachment>> {
    let doc = lopdf::Document::load_mem(pdf).context(crate::t!("err.read_pdf"))?;
    load_from(&doc)
}

pub fn load_from(doc: &lopdf::Document) -> Result<Vec<Attachment>> {
    let mut out = Vec::new();
    let Ok(cat) = doc.catalog() else { return Ok(out) };
    let Some(names) = cat.get(b"Names").ok().and_then(|o| dict(doc, o)) else { return Ok(out) };
    let Some(ef) = names.get(b"EmbeddedFiles").ok().and_then(|o| dict(doc, o)) else { return Ok(out) };
    let mut entries = Vec::new();
    walk_names(doc, ef, &mut entries, 0);
    for (key, spec) in entries {
        let Some(fs) = dict(doc, &spec) else { continue };
        let file_name = fs
            .get(b"UF")
            .or_else(|_| fs.get(b"F"))
            .ok()
            .and_then(|o| resolve(doc, o))
            .and_then(|o| o.as_str().ok())
            .map(text)
            .unwrap_or_else(|| key.clone());
        let Some(efd) = fs.get(b"EF").ok().and_then(|o| dict(doc, o)) else { continue };
        let Some(stream) = efd.get(b"UF").or_else(|_| efd.get(b"F")).ok().and_then(|o| resolve(doc, o)).and_then(|o| o.as_stream().ok()) else {
            continue;
        };
        let data = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
        out.push(Attachment { name: key, file_name, data });
    }
    Ok(out)
}

fn mime(file_name: &str) -> Option<&'static str> {
    let ext = file_name.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "tif" | "tiff" => "image/tiff",
        "txt" => "text/plain",
        "xml" => "application/xml",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "odt" => "application/vnd.oasis.opendocument.text",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => return None,
    })
}

/// Añade a la actualización el árbol /EmbeddedFiles con exactamente los
/// adjuntos indicados (sustituye al que hubiera).
pub fn write(b: &mut UpdateBuilder, doc: &lopdf::Document, atts: &[Attachment]) -> Result<()> {
    let root_id = doc.trailer.get(b"Root").and_then(|o| o.as_reference()).context(crate::t!("err.no_root"))?;
    let catalog = doc.get_dictionary(root_id).context(crate::t!("err.bad_catalog"))?.clone();

    // Diccionario /Names actual (puede ser indirecto, directo o no existir).
    let (names_id, mut names, rewrite_catalog) = match catalog.get(b"Names") {
        Ok(Object::Reference(r)) => (*r, doc.get_dictionary(*r).cloned().unwrap_or_default(), false),
        Ok(Object::Dictionary(d)) => (b.alloc(), d.clone(), true),
        _ => (b.alloc(), Dictionary::new(), true),
    };

    let mut sorted: Vec<&Attachment> = atts.iter().collect();
    sorted.sort_by_key(|a| pdf_text(&a.name));
    if sorted.is_empty() {
        names.remove(b"EmbeddedFiles");
    } else {
        let mut arr = Vec::new();
        for a in sorted {
            let ef_id = b.alloc();
            let mut ef = Dictionary::new();
            ef.set("Type", Object::Name(b"EmbeddedFile".to_vec()));
            if let Some(m) = mime(&a.file_name) {
                ef.set("Subtype", Object::Name(m.as_bytes().to_vec()));
            }
            let mut params = Dictionary::new();
            params.set("Size", Object::Integer(a.data.len() as i64));
            ef.set("Params", Object::Dictionary(params));
            b.set_stream(ef_id, &ef, &a.data);

            let fs_id = b.alloc();
            let ascii: String = a.file_name.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '_' }).collect();
            let mut fs = Dictionary::new();
            fs.set("Type", Object::Name(b"Filespec".to_vec()));
            fs.set("F", Object::String(ascii.into_bytes(), lopdf::StringFormat::Literal));
            fs.set("UF", Object::String(pdf_text(&a.file_name), lopdf::StringFormat::Hexadecimal));
            let mut efd = Dictionary::new();
            efd.set("F", Object::Reference(ef_id));
            efd.set("UF", Object::Reference(ef_id));
            fs.set("EF", Object::Dictionary(efd));
            b.set_object(fs_id, &Object::Dictionary(fs));

            arr.push(Object::String(pdf_text(&a.name), lopdf::StringFormat::Literal));
            arr.push(Object::Reference(fs_id));
        }
        let tree_id = b.alloc();
        let mut tree = Dictionary::new();
        tree.set("Names", Object::Array(arr));
        b.set_object(tree_id, &Object::Dictionary(tree));
        names.set("EmbeddedFiles", Object::Reference(tree_id));
    }
    b.set_object(names_id, &Object::Dictionary(names));
    if rewrite_catalog {
        let mut cat = catalog;
        cat.set("Names", Object::Reference(names_id));
        b.set_object(root_id, &Object::Dictionary(cat));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texto_pdf() {
        assert_eq!(text(&pdf_text("hola.pdf")), "hola.pdf");
        assert_eq!(text(&pdf_text("título ñ.pdf")), "título ñ.pdf");
    }
}
