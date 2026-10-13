//! Formularios XFA estáticos: PDFium rellena la capa AcroForm (los campos
//! que dibuja y edita), pero al guardar no pasa sus valores al paquete
//! `datasets`. Adobe Reader, en cambio, abre un XFA estático con los datos de
//! `datasets`: sin esto vería el formulario vacío.
//!
//! Cada campo AcroForm de un XFA estático se llama como su ruta XFA
//! (`topmostSubform[0].Page1[0].f1_01[0]`). Se busca su nodo de datos
//! siguiendo esa ruta desde la raíz de `xfa:data`; los subformularios que no
//! aparecen en los datos (páginas, subformularios sin enlace) se saltan.
//! Los campos con enlace global (mismo nombre en cualquier sitio) se buscan
//! por nombre. Lo que no se encuentra se deja como está.

use super::pdfedit::{UpdateBuilder, XfaPdf};
use anyhow::Result;
use lopdf::{Dictionary, Object};
use std::collections::HashMap;

/// Devuelve `saved` (el PDF que guardó PDFium) con los valores de la capa
/// AcroForm copiados a `datasets`, o None si no hay nada que cambiar.
pub fn sync(saved: &[u8], password: Option<&str>) -> Result<Option<Vec<u8>>> {
    let Some(x) = XfaPdf::parse_with(saved, password)? else { return Ok(None) };
    let Some(packet) = x.packet("datasets").cloned() else { return Ok(None) };
    let data = x.packet_data("datasets")?;
    let template = x.packet_data("template").unwrap_or_default();
    let values = acro_values(x.document());
    if values.is_empty() {
        return Ok(None);
    }
    let items = check_items(&String::from_utf8_lossy(&template));
    let Some(new) = apply(&data, &values, &items) else { return Ok(None) };
    let mut b = UpdateBuilder::new(saved, &x);
    b.set_stream(packet.id, &Dictionary::new(), &new)?;
    Ok(Some(b.finish()?))
}

/// Valor de un campo AcroForm.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    /// Casilla o botón de opción: valor de exportación, o None si está
    /// desmarcada (/Off).
    Check(Option<String>),
}

/// Campos AcroForm con valor: nombre completo -> valor.
fn acro_values(doc: &lopdf::Document) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let Ok(cat) = doc.catalog() else { return out };
    let Some(acro) = cat.get(b"AcroForm").ok().and_then(|o| deref_dict(doc, o)) else { return out };
    let Ok(Object::Array(fields)) = acro.get(b"Fields").map(|o| deref(doc, o)) else { return out };
    for f in fields {
        walk(doc, f, "", None, &mut out, 0);
    }
    out
}

fn deref<'a>(doc: &'a lopdf::Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(r) => doc.get_object(*r).unwrap_or(o),
        o => o,
    }
}

fn deref_dict<'a>(doc: &'a lopdf::Document, o: &'a Object) -> Option<&'a Dictionary> {
    deref(doc, o).as_dict().ok()
}

fn walk(doc: &lopdf::Document, o: &Object, parent: &str, parent_ft: Option<&[u8]>, out: &mut Vec<(String, Value)>, depth: usize) {
    if depth > 32 {
        return;
    }
    let Some(d) = deref_dict(doc, o) else { return };
    let name = match d.get(b"T") {
        Ok(Object::String(t, _)) => {
            let t = lopdf::decode_text_string(&Object::String(t.clone(), lopdf::StringFormat::Literal)).unwrap_or_default();
            if parent.is_empty() { t } else { format!("{parent}.{t}") }
        }
        _ => parent.to_string(),
    };
    let ft = d.get(b"FT").ok().and_then(|o| o.as_name().ok()).or(parent_ft);
    if let Ok(v) = d.get(b"V")
        && d.has(b"T")
    {
        let value = match (ft, deref(doc, v)) {
            (Some(b"Btn"), Object::Name(n)) => Value::Check((n.as_slice() != b"Off").then(|| String::from_utf8_lossy(n).into_owned())),
            (_, s @ Object::String(..)) => Value::Text(lopdf::decode_text_string(s).unwrap_or_default()),
            (_, Object::Name(n)) => Value::Text(String::from_utf8_lossy(n).into_owned()),
            _ => Value::Text(String::new()),
        };
        out.push((name.clone(), value));
    }
    if let Ok(Object::Array(kids)) = d.get(b"Kids").map(|o| deref(doc, o)) {
        for k in kids {
            walk(doc, k, &name, ft, out, depth + 1);
        }
    }
}

/// Valores (marcada, desmarcada) de las casillas de la plantilla, por nombre
/// de campo: los dos primeros elementos de `<items>`.
fn check_items(template: &str) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    let re = regex::Regex::new(r#"(?s)<field\b[^>]*?\bname="([^"]+)"[^>]*>(.*?)</field\s*>"#).unwrap();
    let item = regex::Regex::new(r"(?s)<(?:text|integer|decimal)\b[^>]*>([^<]*)</").unwrap();
    for c in re.captures_iter(template) {
        let body = &c[2];
        if !body.contains("<checkButton") {
            continue;
        }
        let Some(i) = body.find("<items") else { continue };
        let vals: Vec<String> = item.captures_iter(&body[i..]).take(2).map(|m| m[1].trim().to_string()).collect();
        if let [on, off] = vals.as_slice() {
            out.entry(c[1].to_string()).or_insert((on.clone(), off.clone()));
        }
    }
    out
}

/// Un elemento del XML de datos: nombre, ruta de sus antepasados (índices en
/// `elems`) y dónde está su contenido.
#[derive(Debug)]
struct Elem {
    name: String,
    parent: Option<usize>,
    /// Posición de su número de orden entre los hermanos con el mismo nombre.
    index: usize,
    /// Rango de bytes de la etiqueta de apertura (incluida la autocerrada).
    open: (usize, usize),
    /// Rango del contenido (vacío y None si es autocerrada).
    content: Option<(usize, usize)>,
    has_children: bool,
}

/// Recorre el XML y devuelve sus elementos (sin comentarios ni PI).
fn elements(xml: &[u8]) -> Vec<Elem> {
    let mut elems: Vec<Elem> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut counts: Vec<HashMap<String, usize>> = vec![HashMap::new()];
    let mut i = 0;
    while i < xml.len() {
        if xml[i] != b'<' {
            i += 1;
            continue;
        }
        if xml[i..].starts_with(b"<!--") {
            i = find(xml, i, b"-->").map_or(xml.len(), |p| p + 3);
            continue;
        }
        if xml[i..].starts_with(b"<![CDATA[") {
            i = find(xml, i, b"]]>").map_or(xml.len(), |p| p + 3);
            continue;
        }
        let end = tag_end(xml, i);
        let tag = &xml[i..end];
        match tag.get(1) {
            Some(b'?') | Some(b'!') => {}
            Some(b'/') => {
                if let Some(e) = stack.pop() {
                    let start = elems[e].open.1;
                    elems[e].content = Some((start, i));
                    counts.pop();
                }
            }
            _ => {
                let name_end = tag.iter().skip(1).position(|c| c.is_ascii_whitespace() || *c == b'/' || *c == b'>').map_or(tag.len(), |p| p + 1);
                let name = String::from_utf8_lossy(&tag[1..name_end]).into_owned();
                let self_closing = tag.ends_with(b"/>");
                let parent = stack.last().copied();
                if let Some(p) = parent {
                    elems[p].has_children = true;
                }
                let n = counts.last_mut().unwrap().entry(name.clone()).or_insert(0);
                let index = *n;
                *n += 1;
                elems.push(Elem { name, parent, index, open: (i, end), content: None, has_children: false });
                if !self_closing {
                    stack.push(elems.len() - 1);
                    counts.push(HashMap::new());
                }
            }
        }
        i = end;
    }
    elems
}

fn find(xml: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    xml[from..].windows(pat.len()).position(|w| w == pat).map(|p| from + p)
}

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

/// `nombre[3]` -> ("nombre", 3).
fn segment(s: &str) -> (&str, usize) {
    match s.strip_suffix(']').and_then(|s| s.rsplit_once('[')) {
        Some((n, i)) => (n, i.parse().unwrap_or(0)),
        None => (s, 0),
    }
}

/// Nodo de datos de un campo por su nombre AcroForm, o None.
fn resolve(elems: &[Elem], root: usize, full: &str) -> Option<usize> {
    let segs: Vec<(&str, usize)> = full.split('.').map(segment).collect();
    let (last, _) = *segs.last()?;
    let mut cur = root;
    for (k, &(name, idx)) in segs.iter().enumerate() {
        let child = elems.iter().position(|e| e.parent == Some(cur) && e.name == name && e.index == idx);
        match child {
            Some(c) => cur = c,
            // La raíz de datos coincide con el primer subformulario.
            None if k == 0 && elems[cur].name == name => {}
            // Subformulario sin datos propios (página, enlace "none").
            None if k + 1 < segs.len() => {}
            None => return None,
        }
    }
    (elems[cur].name == last && !elems[cur].has_children).then_some(cur)
}

/// Escapa un valor para el XML de datos, con los saltos de línea como Adobe.
fn escape(v: &str) -> String {
    let mut o = String::with_capacity(v.len());
    let mut chars = v.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                o.push_str("&#xD;")
            }
            '\n' => o.push_str("&#xD;"),
            c => o.push(c),
        }
    }
    o
}

/// Escribe `values` en el XML de datos. None si nada cambia.
pub(crate) fn apply(xml: &[u8], values: &[(String, Value)], items: &HashMap<String, (String, String)>) -> Option<Vec<u8>> {
    let elems = elements(xml);
    let data = elems.iter().position(|e| e.name == "xfa:data")?;
    // Un cambio por nodo: un grupo de casillas aparece en AcroForm como
    // campo padre y como hijos, y dos cambios del mismo nodo se pisarían.
    let mut edits: HashMap<usize, (usize, usize, Vec<u8>)> = HashMap::new();
    for (full, value) in values {
        let leaf = segment(full.rsplit('.').next().unwrap_or(full)).0;
        let text = match value {
            Value::Text(t) => t.clone(),
            Value::Check(on) => match (on, items.get(leaf)) {
                (Some(_), Some((on, _))) => on.clone(),
                (Some(v), None) => v.clone(),
                (None, Some((_, off))) => off.clone(),
                (None, None) => "0".into(),
            },
        };
        // Por su ruta; si no, por nombre (enlace global), si solo hay uno.
        let target = resolve(&elems, data, full).or_else(|| {
            let mut same = elems.iter().enumerate().filter(|(_, e)| e.name == leaf && !e.has_children && is_under(&elems, e, data));
            match (same.next(), same.next()) {
                (Some((i, _)), None) => Some(i),
                _ => None,
            }
        });
        let Some(t) = target else { continue };
        let e = &elems[t];
        let escaped = escape(&text);
        match e.content {
            Some((a, b)) => {
                if xml[a..b] != *escaped.as_bytes() {
                    edits.insert(t, (a, b, escaped.into_bytes()));
                } else {
                    edits.remove(&t);
                }
            }
            None if !escaped.is_empty() => {
                // <campo .../>  ->  <campo ...>valor</campo>
                let (a, b) = e.open;
                let open = &xml[a..b];
                let body = open[..open.len() - 2].strip_suffix(b" ").unwrap_or(&open[..open.len() - 2]);
                let mut new = body.to_vec();
                new.push(b'>');
                new.extend_from_slice(escaped.as_bytes());
                new.extend_from_slice(format!("</{}>", e.name).as_bytes());
                edits.insert(t, (a, b, new));
            }
            None => {}
        }
    }
    if edits.is_empty() {
        return None;
    }
    let mut edits: Vec<(usize, usize, Vec<u8>)> = edits.into_values().collect();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = xml.to_vec();
    for (a, b, new) in edits {
        out.splice(a..b, new);
    }
    Some(out)
}

fn is_under(elems: &[Elem], e: &Elem, root: usize) -> bool {
    let mut p = e.parent;
    while let Some(i) = p {
        if i == root {
            return true;
        }
        p = elems[i].parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valores_al_paquete_de_datos() {
        let xml = b"<xfa:datasets xmlns:xfa=\"x\"\n><xfa:data\n><top\n><f1_01\n/><Addr\n><f1_09\n/></Addr\n><c1_1\n>0</c1_1\n><g\n/></top\n></xfa:data\n></xfa:datasets\n>";
        let values = vec![
            ("top[0].Page1[0].f1_01[0]".to_string(), Value::Text("A&B\nC".into())),
            ("top[0].Page1[0].Addr[0].f1_09[0]".to_string(), Value::Text("Calle".into())),
            ("top[0].Page1[0].c1_1[0]".to_string(), Value::Check(Some("1".into()))),
            ("top[0].Page2[0].nada[0]".to_string(), Value::Text("x".into())),
        ];
        let items = HashMap::from([("c1_1".to_string(), ("1".to_string(), "0".to_string()))]);
        let out = String::from_utf8(apply(xml, &values, &items).unwrap()).unwrap();
        assert!(out.contains("<f1_01\n>A&amp;B&#xD;C</f1_01>"), "{out}");
        assert!(out.contains("<f1_09\n>Calle</f1_09>"), "{out}");
        assert!(out.contains("<c1_1\n>1</c1_1\n>"), "{out}");
        assert!(!out.contains(">x<"), "{out}");
        // Sin cambios: None.
        assert!(apply(out.as_bytes(), &values[..3], &items).is_none());
    }
}
