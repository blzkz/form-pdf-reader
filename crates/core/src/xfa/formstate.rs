//! Estado del formulario que fijan los scripts y que no está en los datos:
//! qué campos están abiertos o protegidos (`access`), qué secciones se ven
//! (`presence`) y el valor de los campos sin enlace a datos (casillas "Sí" de
//! "Incorporar datos a este apartado", por ejemplo).
//!
//! Adobe lo guarda en el paquete `form` (restoreState="auto") y lo restaura al
//! abrir. PDFium no. Sin él, al reabrir un PDF el desplegable "Tipo de
//! documento" de los adjuntos salía protegido, entre otras cosas.
//!
//! Al guardar, la aplicación escribe su propio estado (capturado del
//! formulario en vivo) en el diccionario /Info del PDF, con una huella de los
//! datos guardados. Al abrir se usa:
//! 1. el estado propio, si la huella coincide con los datos actuales (el
//!    último en guardar fue esta aplicación);
//! 2. si no, el paquete `form` de Adobe (el último en guardar fue Adobe, o el
//!    PDF está sin rellenar).

use super::pdfedit::{UpdateBuilder, XfaPdf};
use anyhow::Result;
use lopdf::{Dictionary, Object};
use regex::Regex;

const KEY_STATE: &[u8] = b"PDFRE_FormState";
const KEY_HASH: &[u8] = b"PDFRE_DataHash";

/// Huella FNV-1a de 64 bits (estable entre ejecuciones y versiones).
pub fn fingerprint(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn json_opt(v: &Option<String>) -> String {
    v.as_deref().map(json_str).unwrap_or_else(|| "null".into())
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Una entrada de estado: expresión SOM y propiedades a restaurar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub som: String,
    pub access: Option<String>,
    pub presence: Option<String>,
    pub value: Option<String>,
}

/// Lista de entradas como array JSON `[[som, access, presence, value], ...]`
/// (también es un literal JavaScript válido).
pub fn to_json(entries: &[Entry]) -> String {
    let mut o = String::from("[");
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        o.push_str(&format!("[{},{},{},{}]", json_str(&e.som), json_opt(&e.access), json_opt(&e.presence), json_opt(&e.value)));
    }
    o.push(']');
    o
}

/// Extrae el estado del paquete `form` de Adobe.
pub fn from_form_packet(xml: &str) -> Vec<Entry> {
    let re_tag = Regex::new(r"<(/?)([A-Za-z][\w:.-]*)\b([^>]*?)(/?)>").unwrap();
    let re_attr = Regex::new(r#"\b([\w:]+)="([^"]*)""#).unwrap();
    // Elementos que forman parte de la ruta SOM.
    const CONTAINERS: &[&str] = &["subform", "subformSet", "field", "exclGroup", "draw", "pageSet", "pageArea", "area"];
    // Pila: (nombre SOM, índice, contadores de hijos por nombre, es contenedor).
    struct Level {
        seg: Option<String>,
        counts: std::collections::HashMap<String, usize>,
        tag: String,
    }
    let mut stack: Vec<Level> = vec![Level { seg: Some("xfa.form".into()), counts: Default::default(), tag: "#root".into() }];
    let mut out: Vec<Entry> = Vec::new();
    // Valor de reemplazo en curso: (índice de la entrada del campo, texto).
    let mut in_override: Option<usize> = None;
    let mut override_text = String::new();
    let mut last = 0usize;
    for m in re_tag.captures_iter(xml) {
        let whole = m.get(0).unwrap();
        // Texto entre etiquetas dentro de un <value override="1">.
        if in_override.is_some() {
            override_text.push_str(&xml[last..whole.start()]);
        }
        last = whole.end();
        let tag = m[2].to_string();
        let attrs: Vec<(String, String)> = re_attr.captures_iter(&m[3]).map(|a| (a[1].to_string(), xml_unescape(&a[2]))).collect();
        let get = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        if &m[1] == "/" {
            if tag == "value"
                && let Some(idx) = in_override.take()
            {
                let v = override_text.trim().to_string();
                if !v.is_empty() {
                    out[idx].value = Some(v);
                }
                override_text.clear();
            }
            if stack.len() > 1 && stack.last().is_some_and(|l| l.tag == tag) {
                stack.pop();
            }
            continue;
        }
        let self_closing = &m[4] == "/";
        if CONTAINERS.contains(&tag.as_str()) {
            let name = get("name").unwrap_or_else(|| format!("#{tag}"));
            let parent = stack.last_mut().unwrap();
            let idx = *parent.counts.get(&name).unwrap_or(&0);
            parent.counts.insert(name.clone(), idx + 1);
            let path: Vec<&str> = stack.iter().filter_map(|l| l.seg.as_deref()).collect();
            let som = format!("{}.{}[{}]", path.join("."), name, idx);
            let access = get("access").filter(|_| matches!(tag.as_str(), "field" | "exclGroup" | "subform"));
            let presence = get("presence");
            if access.is_some() || presence.is_some() || tag == "field" || tag == "exclGroup" {
                out.push(Entry { som: som.clone(), access, presence, value: None });
            }
            if !self_closing {
                stack.push(Level { seg: Some(format!("{name}[{idx}]")), counts: Default::default(), tag });
            }
        } else if tag == "value" && !self_closing && get("override").as_deref() == Some("1") {
            // Valor de un campo sin enlace a datos, guardado por Adobe.
            let field = stack.last().filter(|l| l.tag == "field" || l.tag == "exclGroup");
            if field.is_some() {
                let som = stack.iter().filter_map(|l| l.seg.as_deref()).collect::<Vec<_>>().join(".");
                in_override = out.iter().rposition(|e| e.som == som);
            }
        } else if !self_closing && in_override.is_none() {
            // Elementos no contenedores (caption, items, value...): no forman
            // parte de la ruta, pero hay que llevar su anidamiento.
            stack.push(Level { seg: None, counts: Default::default(), tag });
        }
    }
    // Quitar campos sin nada que restaurar.
    out.retain(|e| e.access.is_some() || e.presence.is_some() || e.value.is_some());
    out
}

/// Estado guardado por la aplicación en /Info: (json, huella de los datos).
fn read_saved(doc: &lopdf::Document) -> Option<(String, String)> {
    let info = match doc.trailer.get(b"Info").ok()? {
        Object::Reference(r) => doc.get_dictionary(*r).ok()?,
        Object::Dictionary(d) => d,
        _ => return None,
    };
    let hash = info.get(KEY_HASH).ok()?.as_str().ok().map(|b| String::from_utf8_lossy(b).into_owned())?;
    let stream = match info.get(KEY_STATE).ok()? {
        Object::Reference(r) => doc.get_object(*r).ok()?.as_stream().ok()?,
        _ => return None,
    };
    let data = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
    Some((String::from_utf8_lossy(&data).into_owned(), hash))
}

/// Estado a restaurar al abrir (array JSON), o None si no hay.
pub fn for_open(xfa: &XfaPdf) -> Option<String> {
    let datasets = xfa.packet_data("datasets").ok();
    if let Some((json, hash)) = read_saved(xfa.document())
        && datasets.as_deref().map(fingerprint).as_deref() == Some(hash.as_str())
    {
        log::info!("Estado del formulario: el guardado por la aplicación");
        return Some(json);
    }
    let form = xfa.packet_data("form").ok()?;
    let entries = from_form_packet(&String::from_utf8_lossy(&form));
    log::info!("Estado del formulario: paquete form de Adobe ({} entradas)", entries.len());
    for e in &entries {
        log::debug!("estado: {e:?}");
    }
    (!entries.is_empty()).then(|| to_json(&entries))
}

/// Escribe el estado `json` en /Info junto con la huella de `datasets`.
pub fn write(b: &mut UpdateBuilder, doc: &lopdf::Document, json: &str, datasets: &[u8]) -> Result<()> {
    let (info_id, mut info) = match doc.trailer.get(b"Info") {
        Ok(Object::Reference(r)) => (*r, doc.get_dictionary(*r).cloned().unwrap_or_default()),
        Ok(Object::Dictionary(d)) => (b.alloc(), d.clone()),
        _ => (b.alloc(), Dictionary::new()),
    };
    let sid = b.alloc();
    b.set_stream(sid, &Dictionary::new(), json.as_bytes());
    info.set(KEY_STATE.to_vec(), Object::Reference(sid));
    info.set(KEY_HASH.to_vec(), Object::String(fingerprint(datasets).into_bytes(), lopdf::StringFormat::Literal));
    b.set_object(info_id, &Object::Dictionary(info));
    b.set_info(info_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paquete_form() {
        let x = r#"<form checksum="x" xmlns="http://www.xfa.org/schema/xfa-form/2.8/"><subform name="R"><instanceManager name="_s"/><subform name="s"><field name="tipo" access="open"><items><text>A</text></items></field><subform name="extra" presence="visible"><field name="d"/></subform></subform><subform name="s"><field name="tipo"><value override="1"><integer>1</integer></value></field></subform><subform><draw name="t" presence="hidden"/></subform></subform></form>"#;
        let e = from_form_packet(x);
        assert_eq!(
            e,
            vec![
                Entry { som: "xfa.form.R[0].s[0].tipo[0]".into(), access: Some("open".into()), presence: None, value: None },
                Entry { som: "xfa.form.R[0].s[0].extra[0]".into(), access: None, presence: Some("visible".into()), value: None },
                Entry { som: "xfa.form.R[0].s[1].tipo[0]".into(), access: None, presence: None, value: Some("1".into()) },
                Entry { som: "xfa.form.R[0].#subform[0].t[0]".into(), access: None, presence: Some("hidden".into()), value: None },
            ]
        );
        assert_eq!(to_json(&e[..1]), r#"[["xfa.form.R[0].s[0].tipo[0]","open",null,null]]"#);
    }

    #[test]
    fn huella_estable() {
        assert_eq!(fingerprint(b""), "cbf29ce484222325");
        assert_ne!(fingerprint(b"a"), fingerprint(b"b"));
    }
}
