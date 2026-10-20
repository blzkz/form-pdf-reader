//! Firma digital con certificado (PAdES básico, B-B): la que reconocen
//! Adobe y los validadores europeos.
//!
//! La firma se añade como actualización incremental al final del PDF ya
//! guardado, sin tocar lo anterior: un campo de firma (nuevo o uno vacío del
//! documento) con su diccionario `/Sig`, cuyo `/Contents` lleva la firma CMS
//! y cuyo `/ByteRange` dice qué bytes cubre (todos menos ese hueco).
//!
//! Se hace en dos pasos para que la clave privada no tenga que pasar por
//! aquí: `prepare` deja el PDF con el hueco y devuelve lo que hay que firmar;
//! quien tiene la clave (un fichero `.p12`, el almacén del sistema, una
//! tarjeta o el DNIe) lo firma, y `Prepared::finish` mete la firma.

pub mod cert;
mod cms;
mod local_time;
pub mod p12;
pub mod tsa;

use anyhow::{Context, Result, anyhow, bail};
use lopdf::{Dictionary, Object, ObjectId};
use sha2::{Digest, Sha256};

use crate::sign::{Segment, SavedSignature, Shape};
use crate::xfa::pdfedit::{self, UpdateBuilder};

pub use cert::{CertInfo, build_chain, cert_info};
pub use cms::{KeyKind, key_kind};
pub use cms::ecdsa_der;
pub use p12::P12Key;

/// Quien firma: su certificado y la operación de firmar.
pub trait Signer {
    /// El certificado del firmante y después los intermedios (DER).
    fn chain(&self) -> Vec<Vec<u8>>;
    /// Firma `data` con SHA-256: PKCS#1 v1.5 con RSA, o ECDSA (en DER o
    /// como r||s).
    fn sign(&mut self, data: &[u8]) -> Result<Vec<u8>>;
}

/// Dónde va la firma.
#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    /// Sin nada visible en la página (se ve en el panel de firmas).
    Invisible,
    /// En un campo de firma vacío del documento, por su nombre completo.
    Field(String),
    /// En un campo nuevo, en `rect` (izquierda, abajo, derecha, arriba, en
    /// puntos) de la página `page` (desde 0).
    New { page: usize, rect: [f32; 4] },
}

/// Lo que se pide al firmar.
#[derive(Debug, Clone)]
pub struct Request {
    pub placement: Placement,
    pub reason: Option<String>,
    pub location: Option<String>,
    /// La firma manuscrita que se ve junto al texto (si es visible).
    pub handwritten: Option<SavedSignature>,
    /// Servidor de sellado de tiempo (`http://...`), si se quiere sello.
    pub timestamp: Option<String>,
    /// Bloquear el documento tras firmar: no admite más cambios ni firmas.
    /// Si es la primera firma, es una firma de certificación (DocMDP 1); si
    /// ya había firmas, bloquea el campo y todo el documento (FieldMDP, /P 1).
    pub lock: bool,
}

/// Un campo de firma del documento.
#[derive(Debug, Clone, PartialEq)]
pub struct SignatureField {
    /// Nombre completo (con los de sus padres, separados por puntos).
    pub name: String,
    /// Página (desde 0) y rectángulo (izquierda, abajo, derecha, arriba).
    pub page: Option<usize>,
    pub rect: [f32; 4],
    /// Ya está firmado.
    pub signed: bool,
}

/// El PDF con el hueco de la firma, listo para firmar.
pub struct Prepared {
    pdf: Vec<u8>,
    /// Posición del `<` de `/Contents` y número de cifras hexadecimales.
    contents_at: usize,
    contents_len: usize,
    chain: Vec<Vec<u8>>,
    attrs: der::asn1::SetOfVec<x509_cert::attr::Attribute>,
    to_sign: Vec<u8>,
    tsa: Option<String>,
}

impl Prepared {
    /// Lo que hay que firmar (los atributos firmados de la firma CMS).
    pub fn data_to_sign(&self) -> &[u8] {
        &self.to_sign
    }

    /// Mete la firma `signature` (de `data_to_sign`) y devuelve el PDF
    /// firmado.
    pub fn finish(self, signature: &[u8]) -> Result<Vec<u8>> {
        // El sello de tiempo cubre el valor de la firma tal como va en el CMS.
        let signature = match key_kind(&self.chain[0])? {
            KeyKind::Ec => ecdsa_der(signature),
            KeyKind::Rsa => signature.to_vec(),
        };
        let token = match &self.tsa {
            Some(url) => Some(tsa::timestamp(url, &signature)?),
            None => None,
        };
        let cms = cms::signed_data(&self.chain, self.attrs, &signature, token)?;
        let hex: String = cms.iter().map(|b| format!("{b:02X}")).collect();
        if hex.len() > self.contents_len {
            bail!("la firma no cabe en el hueco reservado ({} > {})", hex.len(), self.contents_len);
        }
        let mut pdf = self.pdf;
        pdf[self.contents_at + 1..self.contents_at + 1 + hex.len()].copy_from_slice(hex.as_bytes());
        Ok(pdf)
    }
}

/// Firma `base` (un PDF ya guardado; `password` si está cifrado) con
/// `signer`.
pub fn sign(base: &[u8], password: Option<&str>, req: &Request, signer: &mut dyn Signer) -> Result<Vec<u8>> {
    let p = prepare(base, password, req, &signer.chain())?;
    let sig = signer.sign(p.data_to_sign())?;
    p.finish(&sig)
}

/// Los campos de firma del documento.
pub fn signature_fields(base: &[u8], password: Option<&str>) -> Result<Vec<SignatureField>> {
    let doc = pdfedit::load(base, password)?;
    let pages = page_index(&doc);
    let mut out = Vec::new();
    for f in sig_fields(&doc) {
        let (rect, page) = widget_place(&doc, &f, &pages);
        out.push(SignatureField { name: f.name, page, rect, signed: f.signed });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Preparar
// ---------------------------------------------------------------------------

/// Hueco para la firma CMS, en bytes: certificados y unos 8 KB para lo
/// demás; con sello de tiempo, 12 KB más (lleva los certificados del
/// servidor).
fn contents_size(chain: &[Vec<u8>], timestamp: bool) -> usize {
    chain.iter().map(Vec::len).sum::<usize>() + 8192 + if timestamp { 12288 } else { 0 }
}

/// Prepara la firma de `base` con el certificado (y la cadena) `chain`.
pub fn prepare(base: &[u8], password: Option<&str>, req: &Request, chain: &[Vec<u8>]) -> Result<Prepared> {
    let leaf = chain.first().ok_or_else(|| anyhow!("{}", crate::t!("pades.no_cert")))?;
    let info = cert_info(leaf).ok_or_else(|| anyhow!("{}", crate::t!("pades.bad_cert")))?;
    let doc = pdfedit::load(base, password).context(crate::t!("pades.read_pdf"))?;
    let pages = page_index(&doc);
    let mut up = UpdateBuilder::from_doc(base, &doc);
    let now = local_time::now();

    // El diccionario /Sig (el hueco de /Contents y /ByteRange va aparte).
    let sig_id = up.alloc();
    // Bloqueo: certificar si nadie firmó antes; si no, bloquear el campo.
    let already_signed = sig_fields(&doc).iter().any(|f| f.signed);
    let certify = req.lock && !already_signed;
    let field_lock = req.lock && already_signed;
    let mut sig = Dictionary::new();
    sig.set("Type", Object::Name(b"Sig".to_vec()));
    sig.set("Filter", Object::Name(b"Adobe.PPKLite".to_vec()));
    sig.set("SubFilter", Object::Name(b"ETSI.CAdES.detached".to_vec()));
    sig.set("M", text(&now.pdf()));
    sig.set("Name", text(&info.subject));
    if let Some(r) = req.reason.as_deref().filter(|s| !s.trim().is_empty()) {
        sig.set("Reason", text(r));
    }
    if let Some(l) = req.location.as_deref().filter(|s| !s.trim().is_empty()) {
        sig.set("Location", text(l));
    }
    let mut app = Dictionary::new();
    app.set("Name", Object::Name(b"Form PDF Reader".to_vec()));
    app.set("REx", text(env!("CARGO_PKG_VERSION")));
    let mut build = Dictionary::new();
    build.set("App", Object::Dictionary(app));
    sig.set("Prop_Build", Object::Dictionary(build));
    let size = contents_size(chain, req.timestamp.is_some());
    let mut tail = Vec::new();
    tail.extend_from_slice(b"/ByteRange ");
    tail.extend_from_slice(&[b' '; BYTE_RANGE_LEN]);
    tail.extend_from_slice(b" /Contents <");
    tail.extend(std::iter::repeat_n(b'0', size * 2));
    tail.extend_from_slice(b">");
    if req.lock {
        let mut params = Dictionary::new();
        params.set("Type", Object::Name(b"TransformParams".to_vec()));
        params.set("P", Object::Integer(1));
        params.set("V", Object::Name(b"1.2".to_vec()));
        if field_lock {
            params.set("Action", Object::Name(b"All".to_vec()));
        }
        let mut sig_ref = Dictionary::new();
        sig_ref.set("Type", Object::Name(b"SigRef".to_vec()));
        sig_ref.set("TransformMethod", Object::Name(if certify { b"DocMDP".to_vec() } else { b"FieldMDP".to_vec() }));
        sig_ref.set("TransformParams", Object::Dictionary(params));
        sig.set("Reference", Object::Array(vec![Object::Dictionary(sig_ref)]));
    }
    up.set_dict_with_raw(sig_id, &sig, &tail)?;

    // El campo de firma y su anotación (widget).
    let catalog_id = doc.trailer.get(b"Root").and_then(Object::as_reference).map_err(|_| anyhow!("{}", crate::t!("pades.read_pdf")))?;
    let mut catalog = doc.get_dictionary(catalog_id)?.clone();
    let (acro_id, mut acro) = match catalog.get(b"AcroForm") {
        Ok(Object::Reference(id)) => (Some(*id), doc.get_dictionary(*id)?.clone()),
        Ok(Object::Dictionary(d)) => (None, d.clone()),
        _ => (None, Dictionary::new()),
    };
    // El campo (lleva /V) y su widget (lleva /AP); el mismo objeto si van juntos.
    let (rect, page, field_id, widget_id, mut widget) = match &req.placement {
        Placement::Field(name) => {
            let f = sig_fields(&doc).into_iter().find(|f| &f.name == name).ok_or_else(|| anyhow!("{}", crate::t!("pades.no_field", name)))?;
            if f.signed {
                bail!("{}", crate::t!("pades.field_signed", name));
            }
            let (rect, page) = widget_place(&doc, &f, &pages);
            (rect, page.unwrap_or(0), f.field, f.widget, doc.get_dictionary(f.widget)?.clone())
        }
        Placement::Invisible | Placement::New { .. } => {
            let (page, rect) = match req.placement {
                Placement::New { page, rect } => (page, rect),
                _ => (0, [0.0; 4]),
            };
            let page_id = *pages.get(page).ok_or_else(|| anyhow!("{}", crate::t!("pades.no_page", page + 1)))?;
            let id = up.alloc();
            let mut d = Dictionary::new();
            d.set("Type", Object::Name(b"Annot".to_vec()));
            d.set("Subtype", Object::Name(b"Widget".to_vec()));
            d.set("FT", Object::Name(b"Sig".to_vec()));
            d.set("T", text(&new_field_name(&doc)));
            d.set("P", Object::Reference(page_id));
            d.set("Rect", Object::Array(rect.iter().map(|v| Object::Real(*v)).collect()));
            // Imprimir y bloqueada.
            d.set("F", Object::Integer(132));
            // En la página: añadirla a sus /Annots.
            let mut page_dict = doc.get_dictionary(page_id)?.clone();
            match page_dict.get(b"Annots") {
                Ok(Object::Reference(aid)) => {
                    let mut arr = doc.get_object(*aid)?.as_array()?.clone();
                    arr.push(Object::Reference(id));
                    up.set_object(*aid, &Object::Array(arr))?;
                }
                Ok(Object::Array(a)) => {
                    let mut arr = a.clone();
                    arr.push(Object::Reference(id));
                    page_dict.set("Annots", Object::Array(arr));
                    up.set_object(page_id, &Object::Dictionary(page_dict))?;
                }
                _ => {
                    page_dict.set("Annots", Object::Array(vec![Object::Reference(id)]));
                    up.set_object(page_id, &Object::Dictionary(page_dict))?;
                }
            }
            // Y en los campos del formulario.
            match acro.get(b"Fields") {
                Ok(Object::Reference(fid)) => {
                    let mut arr = doc.get_object(*fid)?.as_array()?.clone();
                    arr.push(Object::Reference(id));
                    up.set_object(*fid, &Object::Array(arr))?;
                }
                Ok(Object::Array(a)) => {
                    let mut arr = a.clone();
                    arr.push(Object::Reference(id));
                    acro.set("Fields", Object::Array(arr));
                }
                _ => acro.set("Fields", Object::Array(vec![Object::Reference(id)])),
            }
            (rect, page, id, id, d)
        }
    };
    let _ = page;
    // Bloqueo del campo (y de todo lo demás) al firmarlo.
    let lock_dict = || {
        let mut l = Dictionary::new();
        l.set("Type", Object::Name(b"SigFieldLock".to_vec()));
        l.set("Action", Object::Name(b"All".to_vec()));
        l.set("P", Object::Integer(1));
        Object::Dictionary(l)
    };
    if field_id == widget_id {
        widget.set("V", Object::Reference(sig_id));
        if field_lock {
            widget.set("Lock", lock_dict());
        }
    } else {
        let mut field = doc.get_dictionary(field_id)?.clone();
        field.set("V", Object::Reference(sig_id));
        if field_lock {
            field.set("Lock", lock_dict());
        }
        up.set_object(field_id, &Object::Dictionary(field))?;
    }
    // La apariencia: el texto (y la firma manuscrita) en el rectángulo.
    let (w, h) = ((rect[2] - rect[0]).abs(), (rect[3] - rect[1]).abs());
    let ap_id = up.alloc();
    let (content, font) = appearance(w, h, &info.subject, &now.shown(), req)?;
    let mut ap = Dictionary::new();
    ap.set("Type", Object::Name(b"XObject".to_vec()));
    ap.set("Subtype", Object::Name(b"Form".to_vec()));
    ap.set("BBox", Object::Array(vec![Object::Real(0.0), Object::Real(0.0), Object::Real(w), Object::Real(h)]));
    if font {
        let mut f = Dictionary::new();
        f.set("Type", Object::Name(b"Font".to_vec()));
        f.set("Subtype", Object::Name(b"Type1".to_vec()));
        f.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        f.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Dictionary(f));
        let mut res = Dictionary::new();
        res.set("Font", Object::Dictionary(fonts));
        ap.set("Resources", Object::Dictionary(res));
    }
    up.set_stream(ap_id, &ap, &content)?;
    let mut apd = Dictionary::new();
    apd.set("N", Object::Reference(ap_id));
    widget.set("AP", Object::Dictionary(apd));
    up.set_object(widget_id, &Object::Dictionary(widget))?;

    // Formulario con firmas que solo admite añadir (SigFlags 3).
    acro.set("SigFlags", Object::Integer(3));
    let mut catalog_changed = false;
    match acro_id {
        Some(id) => up.set_object(id, &Object::Dictionary(acro))?,
        None => {
            catalog.set("AcroForm", Object::Dictionary(acro));
            catalog_changed = true;
        }
    }
    // Firma de certificación: el catálogo la señala en /Perms /DocMDP.
    if certify {
        match catalog.get(b"Perms") {
            Ok(Object::Reference(pid)) => {
                let mut perms = doc.get_dictionary(*pid)?.clone();
                perms.set("DocMDP", Object::Reference(sig_id));
                up.set_object(*pid, &Object::Dictionary(perms))?;
            }
            Ok(Object::Dictionary(d)) => {
                let mut perms = d.clone();
                perms.set("DocMDP", Object::Reference(sig_id));
                catalog.set("Perms", Object::Dictionary(perms));
                catalog_changed = true;
            }
            _ => {
                let mut perms = Dictionary::new();
                perms.set("DocMDP", Object::Reference(sig_id));
                catalog.set("Perms", Object::Dictionary(perms));
                catalog_changed = true;
            }
        }
    }
    if catalog_changed {
        up.set_object(catalog_id, &Object::Dictionary(catalog))?;
    }

    let mut pdf = up.finish()?;
    // Posiciones del hueco en el fichero final.
    let contents_tag = format!("/Contents <{}", "0".repeat(16));
    let contents_at = find_last(&pdf, contents_tag.as_bytes()).ok_or_else(|| anyhow!("hueco de la firma no encontrado"))? + b"/Contents ".len();
    // Antes de " /Contents <" está el sitio reservado para /ByteRange.
    let br_at = contents_at - b" /Contents ".len() - BYTE_RANGE_LEN;
    let after = contents_at + size * 2 + 2;
    let range = format!("[0 {contents_at} {after} {}]", pdf.len() - after);
    if range.len() > BYTE_RANGE_LEN {
        bail!("ByteRange demasiado largo");
    }
    pdf[br_at..br_at + range.len()].copy_from_slice(range.as_bytes());
    let digest = Sha256::new().chain_update(&pdf[..contents_at]).chain_update(&pdf[after..]).finalize();
    let attrs = cms::signed_attributes(&digest, leaf)?;
    let to_sign = cms::to_be_signed(&attrs)?;
    Ok(Prepared { pdf, contents_at, contents_len: size * 2, chain: chain.to_vec(), attrs, to_sign, tsa: req.timestamp.clone() })
}

/// El bloqueo que tiene el documento por sus firmas: el permiso más
/// restrictivo de una firma de certificación (DocMDP) o del bloqueo de un
/// campo firmado (`/Lock` con `/P`). 1: no admite cambios ni más firmas;
/// 2: solo rellenar y firmar; 3: además comentar. None: sin bloqueo.
pub fn lock_permission(base: &[u8], password: Option<&str>) -> Option<u32> {
    let doc = pdfedit::load(base, password).ok()?;
    let p_of = |o: &Object| -> Option<u32> {
        let d = doc.dereference(o).ok()?.1.as_dict().ok()?;
        d.get(b"P").ok().and_then(|p| p.as_i64().ok()).map(|p| p as u32)
    };
    let mut best: Option<u32> = None;
    let mut take = |p: u32| best = Some(best.map_or(p, |b| b.min(p)));
    // Certificación: /Perms /DocMDP → diccionario de firma → /Reference.
    if let Ok(cat) = doc.catalog()
        && let Some((_, perms)) = cat.get(b"Perms").ok().and_then(|p| doc.dereference(p).ok())
        && let Ok(perms) = perms.as_dict()
        && let Some((_, sig)) = perms.get(b"DocMDP").ok().and_then(|s| doc.dereference(s).ok())
        && let Ok(sig) = sig.as_dict()
        && let Some((_, refs)) = sig.get(b"Reference").ok().and_then(|r| doc.dereference(r).ok())
        && let Ok(refs) = refs.as_array()
    {
        for r in refs {
            if let Ok((_, r)) = doc.dereference(r)
                && let Ok(r) = r.as_dict()
                && r.get(b"TransformMethod").ok().and_then(|m| m.as_name().ok()) == Some(b"DocMDP".as_slice())
                && let Some(p) = r.get(b"TransformParams").ok().and_then(p_of)
            {
                take(p);
            }
        }
    }
    // Campos firmados con bloqueo de documento (/Lock /P).
    for f in sig_fields(&doc).into_iter().filter(|f| f.signed) {
        for id in [f.field, f.widget] {
            if let Some(p) = doc.get_dictionary(id).ok().and_then(|d| d.get(b"Lock").ok()).and_then(p_of) {
                take(p);
            }
        }
    }
    best
}

/// Sitio reservado para `[0 a b c]`.
const BYTE_RANGE_LEN: usize = 48;

fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// Cadena de texto PDF: en PDFDocEncoding si es Latin-1, si no en UTF-16.
fn text(s: &str) -> Object {
    if s.chars().all(|c| (c as u32) < 0x80) {
        Object::string_literal(s)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, lopdf::StringFormat::Hexadecimal)
    }
}

/// Las páginas, en orden.
fn page_index(doc: &lopdf::Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

/// Un nombre de campo que no exista: "Firma1", "Firma2"...
fn new_field_name(doc: &lopdf::Document) -> String {
    let names: Vec<String> = all_fields(doc).into_iter().map(|f| f.name).collect();
    (1..).map(|n| format!("Signature{n}")).find(|n| !names.contains(n)).unwrap_or_default()
}

struct FieldRef {
    name: String,
    field: ObjectId,
    /// El widget del campo (el propio campo si están juntos).
    widget: ObjectId,
    is_sig: bool,
    signed: bool,
}

fn sig_fields(doc: &lopdf::Document) -> Vec<FieldRef> {
    all_fields(doc).into_iter().filter(|f| f.is_sig).collect()
}

/// Los campos terminales del formulario.
fn all_fields(doc: &lopdf::Document) -> Vec<FieldRef> {
    let mut out = Vec::new();
    let Ok(catalog) = doc.catalog() else { return out };
    let acro = match catalog.get(b"AcroForm") {
        Ok(Object::Reference(id)) => doc.get_dictionary(*id).ok(),
        Ok(Object::Dictionary(d)) => Some(d),
        _ => None,
    };
    let Some(fields) = acro.and_then(|a| a.get(b"Fields").ok()).and_then(|f| doc.dereference(f).ok()).and_then(|(_, o)| o.as_array().ok()) else {
        return out;
    };
    for f in fields {
        if let Ok(id) = f.as_reference() {
            walk(doc, id, "", None, &mut out, 0);
        }
    }
    out
}

fn walk(doc: &lopdf::Document, id: ObjectId, parent: &str, ft: Option<Vec<u8>>, out: &mut Vec<FieldRef>, depth: usize) {
    let Ok(d) = doc.get_dictionary(id) else { return };
    if depth > 32 {
        return;
    }
    let t = d.get(b"T").ok().and_then(|t| t.as_str().ok()).map(decode_text);
    let name = match (&t, parent.is_empty()) {
        (Some(t), true) => t.clone(),
        (Some(t), false) => format!("{parent}.{t}"),
        (None, _) => parent.to_string(),
    };
    let ft = d.get(b"FT").ok().and_then(|o| o.as_name().ok()).map(<[u8]>::to_vec).or(ft);
    let kids: Vec<ObjectId> = d.get(b"Kids").ok().and_then(|k| doc.dereference(k).ok()).and_then(|(_, o)| o.as_array().ok()).map(|a| a.iter().filter_map(|k| k.as_reference().ok()).collect()).unwrap_or_default();
    // Hijos que son campos (tienen /T); si solo hay widgets, es terminal.
    let field_kids: Vec<ObjectId> = kids.iter().copied().filter(|k| doc.get_dictionary(*k).is_ok_and(|kd| kd.has(b"T"))).collect();
    if !field_kids.is_empty() {
        for k in field_kids {
            walk(doc, k, &name, ft.clone(), out, depth + 1);
        }
        return;
    }
    let widget = kids.first().copied().unwrap_or(id);
    out.push(FieldRef { name, field: id, widget, is_sig: ft.as_deref() == Some(b"Sig"), signed: d.has(b"V") });
}

fn decode_text(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        String::from_utf16_lossy(&u)
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

/// Rectángulo y página del widget de un campo.
fn widget_place(doc: &lopdf::Document, f: &FieldRef, pages: &[ObjectId]) -> ([f32; 4], Option<usize>) {
    let Ok(w) = doc.get_dictionary(f.widget) else { return ([0.0; 4], None) };
    let num = |o: &Object| o.as_float().ok().or_else(|| o.as_i64().ok().map(|v| v as f32)).unwrap_or(0.0);
    let rect = w.get(b"Rect").ok().and_then(|r| doc.dereference(r).ok()).and_then(|(_, o)| o.as_array().ok()).filter(|a| a.len() == 4).map(|a| {
        let (x0, y0, x1, y1) = (num(&a[0]), num(&a[1]), num(&a[2]), num(&a[3]));
        [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
    });
    let page = w.get(b"P").ok().and_then(|p| p.as_reference().ok()).and_then(|p| pages.iter().position(|x| *x == p)).or_else(|| {
        pages.iter().position(|pid| {
            doc.get_dictionary(*pid).ok().and_then(|pd| pd.get(b"Annots").ok()).and_then(|a| doc.dereference(a).ok()).and_then(|(_, o)| o.as_array().ok()).is_some_and(|a| a.iter().any(|x| x.as_reference().ok() == Some(f.widget)))
        })
    });
    (rect.unwrap_or([0.0; 4]), page)
}

// ---------------------------------------------------------------------------
// Apariencia
// ---------------------------------------------------------------------------

/// El contenido del aspecto visible: la firma manuscrita (si la hay) a la
/// izquierda y el texto a la derecha. Devuelve también si usa la fuente.
fn appearance(w: f32, h: f32, signer: &str, date: &str, req: &Request) -> Result<(Vec<u8>, bool)> {
    let mut c = String::new();
    if w < 1.0 || h < 1.0 {
        return Ok((Vec::new(), false));
    }
    let pad = (h * 0.06).clamp(1.0, 4.0);
    let mut text_left = pad;
    if let Some(sig) = &req.handwritten {
        let shape = Shape::of(sig);
        let area_w = if w > h * 2.2 { w * 0.45 } else { w - 2.0 * pad };
        let area_h = if w > h * 2.2 { h - 2.0 * pad } else { h * 0.55 };
        let bottom = if w > h * 2.2 { pad } else { h - area_h - pad };
        let (paths, _) = shape.fit(pad, bottom, area_w - pad, area_h);
        let [r, g, b] = sig.color.map(|v| v as f32 / 255.0);
        c.push_str(&format!("q {r:.3} {g:.3} {b:.3} rg {r:.3} {g:.3} {b:.3} RG 1 J 1 j\n"));
        for p in &paths {
            for s in &p.segments {
                match *s {
                    Segment::MoveTo(x, y) => c.push_str(&format!("{x:.2} {y:.2} m\n")),
                    Segment::LineTo(x, y) => c.push_str(&format!("{x:.2} {y:.2} l\n")),
                    Segment::CubicTo(a, b2, c2, d, e, f) => c.push_str(&format!("{a:.2} {b2:.2} {c2:.2} {d:.2} {e:.2} {f:.2} c\n")),
                    Segment::Close => c.push_str("h\n"),
                }
            }
            match p.stroke {
                Some(sw) => c.push_str(&format!("{sw:.2} w S\n")),
                None => c.push_str("f\n"),
            }
        }
        c.push_str("Q\n");
        if w > h * 2.2 {
            text_left = w * 0.45 + pad;
        }
    }
    let text_top = if req.handwritten.is_some() && w <= h * 2.2 { h * 0.42 } else { h - pad };
    let lines = [crate::t!("pades.appearance_by"), signer.to_string(), crate::t!("pades.appearance_date", date)];
    let box_w = (w - text_left - pad).max(1.0);
    let box_h = (text_top - pad).max(1.0);
    let widest = lines.iter().map(|l| helvetica_width(l)).fold(0.1, f32::max);
    let size = (box_h / (lines.len() as f32 * 1.2)).min(box_w / widest).clamp(2.0, 14.0);
    // El bloque de texto, centrado en vertical en su espacio.
    let block = lines.len() as f32 * size * 1.2;
    let top = text_top - (box_h - block).max(0.0) / 2.0;
    c.push_str(&format!("q BT 0.1 0.1 0.1 rg /F1 {size:.2} Tf {:.2} TL {text_left:.2} {:.2} Td\n", size * 1.2, top - size));
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            c.push_str("T*\n");
        }
        c.push('(');
        c.push_str(&win_ansi(l));
        c.push_str(") Tj\n");
    }
    c.push_str("ET Q\n");
    Ok((latin1(&c), true))
}

/// Ancho de `s` en Helvetica a 1 punto (las medidas de su AFM).
fn helvetica_width(s: &str) -> f32 {
    const ASCII: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722,
        722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556,
        556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
    ];
    s.chars()
        .map(|c| {
            // Las letras con tilde miden como la letra sin ella.
            let base = match c {
                'À'..='Å' => 'A',
                'Ç' => 'C',
                'È'..='Ë' => 'E',
                'Ì'..='Ï' => 'I',
                'Ñ' => 'N',
                'Ò'..='Ö' | 'Ø' => 'O',
                'Ù'..='Ü' => 'U',
                'Ý' => 'Y',
                'à'..='å' => 'a',
                'ç' => 'c',
                'è'..='ë' => 'e',
                'ì'..='ï' => 'i',
                'ñ' => 'n',
                'ò'..='ö' | 'ø' => 'o',
                'ù'..='ü' => 'u',
                'ý' | 'ÿ' => 'y',
                c => c,
            };
            let i = base as usize;
            if (32..127).contains(&i) { ASCII[i - 32] as f32 / 1000.0 } else { 0.6 }
        })
        .sum()
}

/// Texto para una cadena literal con WinAnsiEncoding (los caracteres que no
/// tiene salen como "?"). Se devuelve en "Latin-1" dentro de un String.
fn win_ansi(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        let c = match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                ch
            }
            '€' => '\u{80}',
            c if (c as u32) < 0x100 => c,
            _ => '?',
        };
        out.push(c);
    }
    out
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' }).collect()
}
