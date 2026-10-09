//! Lectura de los paquetes XFA y escritura de actualizaciones incrementales.
//!
//! Una actualización incremental añade objetos nuevos al final del fichero
//! sin tocar ni un byte del original, igual que hace Adobe Reader al guardar
//! un formulario. Así se conservan la plantilla y la firma de "Reader
//! Extensions" (/Perms /UR3), que cubre el rango de bytes original.

use anyhow::{Context, Result, anyhow, bail};
use flate2::{Compression, write::ZlibEncoder};
use lopdf::{Dictionary, Object, ObjectId};
use std::io::Write;

/// Un paquete del array /XFA: nombre ("template", "datasets"...) y objeto stream.
#[derive(Debug, Clone)]
pub struct XfaPacket {
    pub name: String,
    pub id: ObjectId,
}

/// Información de un PDF con XFA necesaria para leer/sustituir paquetes.
pub struct XfaPdf {
    pub(crate) doc: lopdf::Document,
    pub packets: Vec<XfaPacket>,
}

impl XfaPdf {
    /// Analiza un PDF. Devuelve `Ok(None)` si no tiene XFA en forma de array
    /// de paquetes (el caso de un único stream XFA no se soporta para edición).
    pub fn parse(bytes: &[u8]) -> Result<Option<XfaPdf>> {
        let doc = lopdf::Document::load_mem(bytes).context(crate::t!("err.lopdf_read"))?;
        let root = doc.catalog().context(crate::t!("err.no_catalog"))?;
        let Ok(acro) = root.get(b"AcroForm") else { return Ok(None) };
        let acro = resolve_dict(&doc, acro)?;
        let Ok(xfa) = acro.get(b"XFA") else { return Ok(None) };
        let xfa = match xfa {
            Object::Reference(r) => doc.get_object(*r)?,
            o => o,
        };
        let Object::Array(items) = xfa else { return Ok(None) };
        let mut packets = Vec::new();
        for pair in items.chunks(2) {
            if let [Object::String(name, _), Object::Reference(id)] = pair {
                packets.push(XfaPacket { name: String::from_utf8_lossy(name).into_owned(), id: *id });
            }
        }
        if packets.is_empty() {
            return Ok(None);
        }
        Ok(Some(XfaPdf { doc, packets }))
    }

    pub fn packet(&self, name: &str) -> Option<&XfaPacket> {
        self.packets.iter().find(|p| p.name == name)
    }

    /// Contenido descomprimido de un paquete.
    pub fn packet_data(&self, name: &str) -> Result<Vec<u8>> {
        let p = self.packet(name).ok_or_else(|| anyhow!("{}", crate::t!("err.no_packet", name)))?;
        let obj = self.doc.get_object(p.id)?;
        let stream = obj.as_stream().map_err(|_| anyhow!("{}", crate::t!("err.packet_not_stream", name)))?;
        Ok(stream.decompressed_content().unwrap_or_else(|_| stream.content.clone()))
    }

    pub fn document(&self) -> &lopdf::Document {
        &self.doc
    }
}

fn resolve_dict<'a>(doc: &'a lopdf::Document, o: &'a Object) -> Result<&'a Dictionary> {
    match o {
        Object::Reference(r) => Ok(doc.get_dictionary(*r)?),
        Object::Dictionary(d) => Ok(d),
        _ => bail!("{}", crate::t!("err.expected_dict")),
    }
}

/// Posición del último `startxref` del fichero.
fn last_startxref(bytes: &[u8]) -> Result<u64> {
    let tail_start = bytes.len().saturating_sub(2048);
    let tail = &bytes[tail_start..];
    let pos = tail
        .windows(9)
        .rposition(|w| w == b"startxref")
        .ok_or_else(|| anyhow!("{}", crate::t!("err.no_startxref")))?;
    let rest = std::str::from_utf8(&tail[pos + 9..]).unwrap_or("");
    let num: String = rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    num.parse::<u64>().context(crate::t!("err.bad_startxref"))
}

fn serialize_obj(o: &Object) -> Vec<u8> {
    let mut out = Vec::new();
    write_obj(&mut out, o);
    out
}

fn write_obj(out: &mut Vec<u8>, o: &Object) {
    match o {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Integer(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Object::Real(r) => out.extend_from_slice(format!("{r}").as_bytes()),
        Object::Name(n) => {
            out.push(b'/');
            for &c in n {
                if c.is_ascii_alphanumeric() || b"-_.+".contains(&c) {
                    out.push(c);
                } else {
                    out.extend_from_slice(format!("#{c:02X}").as_bytes());
                }
            }
        }
        Object::String(s, _) => {
            out.push(b'<');
            for c in s {
                out.extend_from_slice(format!("{c:02X}").as_bytes());
            }
            out.push(b'>');
        }
        Object::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_obj(out, x);
            }
            out.push(b']');
        }
        Object::Dictionary(d) => write_dict(out, d),
        Object::Reference((id, g)) => out.extend_from_slice(format!("{id} {g} R").as_bytes()),
        Object::Stream(_) => out.extend_from_slice(b"null"),
    }
}

fn write_dict(out: &mut Vec<u8>, d: &Dictionary) {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        write_obj(out, &Object::Name(k.clone()));
        out.push(b' ');
        write_obj(out, v);
    }
    out.extend_from_slice(b">>");
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// Construye una actualización incremental: objetos nuevos o sustituidos que
/// se añaden al final de `base`, con su propio stream de referencias cruzadas.
pub struct UpdateBuilder<'a> {
    base: &'a [u8],
    doc: &'a lopdf::Document,
    next_id: u32,
    objects: Vec<(ObjectId, Vec<u8>)>,
    /// /Info nuevo para el trailer (si se ha creado o sustituido).
    info: Option<ObjectId>,
}

impl<'a> UpdateBuilder<'a> {
    pub fn new(base: &'a [u8], pdf: &'a XfaPdf) -> UpdateBuilder<'a> {
        Self::from_doc(base, &pdf.doc)
    }

    pub fn from_doc(base: &'a [u8], doc: &'a lopdf::Document) -> UpdateBuilder<'a> {
        let size = doc.trailer.get(b"Size").and_then(|o| o.as_i64()).unwrap_or(0).max(doc.max_id as i64 + 1) as u32;
        UpdateBuilder { base, doc, next_id: size, objects: Vec::new(), info: None }
    }

    /// Reserva un número de objeto nuevo.
    pub fn alloc(&mut self) -> ObjectId {
        let id = (self.next_id, 0);
        self.next_id += 1;
        id
    }

    /// Escribe (o sustituye) un objeto que no es stream.
    pub fn set_object(&mut self, id: ObjectId, obj: &Object) {
        self.objects.retain(|(i, _)| *i != id);
        self.objects.push((id, serialize_obj(obj)));
    }

    /// Escribe (o sustituye) un stream comprimido con Flate. `dict` no debe
    /// llevar /Length ni /Filter (se añaden aquí).
    pub fn set_stream(&mut self, id: ObjectId, dict: &Dictionary, data: &[u8]) {
        let compressed = deflate(data);
        let mut d = dict.clone();
        d.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        d.set("Length", Object::Integer(compressed.len() as i64));
        let mut body = serialize_obj(&Object::Dictionary(d));
        body.extend_from_slice(b"\nstream\n");
        body.extend_from_slice(&compressed);
        body.extend_from_slice(b"\nendstream");
        self.objects.retain(|(i, _)| *i != id);
        self.objects.push((id, body));
    }

    /// Hace que el trailer de la actualización apunte a este /Info.
    pub fn set_info(&mut self, id: ObjectId) {
        self.info = Some(id);
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn finish(self) -> Result<Vec<u8>> {
        if self.objects.is_empty() {
            return Ok(self.base.to_vec());
        }
        let prev = last_startxref(self.base)?;
        let trailer = &self.doc.trailer;
        let mut out = self.base.to_vec();
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }
        let mut entries: Vec<(u32, u64, u16)> = Vec::new();
        for ((id, gen_), body) in &self.objects {
            let offset = out.len() as u64;
            out.extend_from_slice(format!("{id} {gen_} obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
            entries.push((*id, offset, *gen_));
        }
        // Stream de referencias cruzadas (el original también las usa).
        let xref_id = self.next_id;
        let xref_offset = out.len() as u64;
        if xref_offset > u32::MAX as u64 {
            bail!("{}", crate::t!("err.too_big"));
        }
        entries.push((xref_id, xref_offset, 0));
        entries.sort_by_key(|e| e.0);
        let mut index = Vec::new();
        let mut rows = Vec::new();
        for (id, off, g) in &entries {
            index.push(Object::Integer(*id as i64));
            index.push(Object::Integer(1));
            rows.push(1u8);
            rows.extend_from_slice(&(*off as u32).to_be_bytes());
            rows.extend_from_slice(&g.to_be_bytes());
        }
        let mut d = Dictionary::new();
        d.set("Type", Object::Name(b"XRef".to_vec()));
        d.set("Size", Object::Integer(xref_id as i64 + 1));
        d.set("Index", Object::Array(index));
        d.set("W", Object::Array(vec![Object::Integer(1), Object::Integer(4), Object::Integer(2)]));
        d.set("Prev", Object::Integer(prev as i64));
        for key in [&b"Root"[..], b"Info", b"ID", b"Encrypt"] {
            if let Ok(v) = trailer.get(key) {
                d.set(key.to_vec(), v.clone());
            }
        }
        if let Some(info) = self.info {
            d.set("Info", Object::Reference(info));
        }
        d.set("Length", Object::Integer(rows.len() as i64));
        out.extend_from_slice(format!("{xref_id} 0 obj\n").as_bytes());
        out.extend_from_slice(&serialize_obj(&Object::Dictionary(d)));
        out.extend_from_slice(b"\nstream\n");
        out.extend_from_slice(&rows);
        out.extend_from_slice(b"\nendstream\nendobj\n");
        out.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
        Ok(out)
    }
}

/// Devuelve `base` + una actualización incremental que sustituye el
/// contenido de los streams indicados (por id de objeto) por `data`.
pub fn replace_streams(base: &[u8], xfa: &XfaPdf, replacements: &[(ObjectId, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut b = UpdateBuilder::new(base, xfa);
    for (id, data) in replacements {
        b.set_stream(*id, &Dictionary::new(), data);
    }
    b.finish()
}
