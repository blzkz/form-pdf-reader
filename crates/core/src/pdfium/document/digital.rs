//! Firmas digitales que ya trae el documento: quién firmó, cuándo, y si el
//! contenido firmado coincide y si se modificó después. No se comprueba si
//! el certificado es de confianza (eso llegará con la firma digital propia).
//!
//! PDFium da el diccionario de cada firma (rango de bytes, contenido CMS,
//! fecha, motivo); el CMS (PKCS#7) se lee con el crate `cms`.

use super::Document;
use cms::cert::CertificateChoices;
use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier};
use der::asn1::{ObjectIdentifier, OctetStringRef};
use der::{Decode, Encode};
use sha2::Digest;
use std::ffi::{c_char, c_int, c_ulong, c_void};

type FpdfSignature = *const c_void;

// fpdf_signature.h (no está en los bindings generados; build.rs enlaza PDFium).
unsafe extern "C" {
    fn FPDF_GetSignatureCount(document: super::sys::FPDF_DOCUMENT) -> c_int;
    fn FPDF_GetSignatureObject(document: super::sys::FPDF_DOCUMENT, index: c_int) -> FpdfSignature;
    fn FPDFSignatureObj_GetContents(signature: FpdfSignature, buffer: *mut c_void, length: c_ulong) -> c_ulong;
    fn FPDFSignatureObj_GetByteRange(signature: FpdfSignature, buffer: *mut c_int, length: c_ulong) -> c_ulong;
    fn FPDFSignatureObj_GetSubFilter(signature: FpdfSignature, buffer: *mut c_char, length: c_ulong) -> c_ulong;
    fn FPDFSignatureObj_GetReason(signature: FpdfSignature, buffer: *mut c_void, length: c_ulong) -> c_ulong;
    fn FPDFSignatureObj_GetTime(signature: FpdfSignature, buffer: *mut c_char, length: c_ulong) -> c_ulong;
    fn FPDFSignatureObj_GetDocMDPPermission(signature: FpdfSignature) -> u32;
}

/// Cómo está el contenido firmado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignedState {
    /// La firma cubre el documento entero y el contenido coincide.
    Intact,
    /// El contenido firmado coincide, pero después se añadieron cambios
    /// (otra firma, datos de formulario, anotaciones...).
    ChangedAfter,
    /// El contenido no coincide con la firma: se alteró.
    Broken,
    /// No se pudo comprobar (formato de firma no reconocido).
    Unknown,
}

#[derive(Debug, Clone)]
pub struct DigitalSignature {
    /// Titular del certificado (su nombre común, o el nombre completo).
    pub signer: Option<String>,
    /// Quién emitió el certificado.
    pub issuer: Option<String>,
    /// Validez del certificado, "AAAA-MM-DD".
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    /// Fecha de la firma, "AAAA-MM-DD HH:MM" (la declarada por el firmante).
    pub time: Option<String>,
    pub reason: Option<String>,
    /// Formato: adbe.pkcs7.detached, ETSI.CAdES.detached...
    pub sub_filter: Option<String>,
    /// Firma de certificación (DocMDP): 1 sin cambios, 2 rellenar y firmar,
    /// 3 además comentar. None: firma de aprobación.
    pub certification: Option<u32>,
    pub state: SignedState,
}

impl Document {
    /// Las firmas digitales del documento, en el orden en que se hicieron.
    /// Bloqueo del documento por sus firmas (ver `pades::lock_permission`).
    pub fn signature_lock(&self) -> Option<u32> {
        crate::pades::lock_permission(&self.base, self.password.as_deref())
    }

    /// El documento no admite cambios ni más firmas (bloqueado al firmar).
    pub fn locked_by_signature(&self) -> bool {
        self.signature_lock() == Some(1)
    }

    /// Los campos de firma del documento (vacíos o ya firmados).
    pub fn signature_fields(&self) -> Vec<crate::pades::SignatureField> {
        crate::pades::signature_fields(&self.base, self.password.as_deref()).unwrap_or_default()
    }

    /// Prepara la firma digital del documento tal como está ahora (con los
    /// datos del formulario y las firmas manuscritas): devuelve lo que hay
    /// que firmar. No escribe nada en disco.
    pub fn prepare_digital(&mut self, req: &crate::pades::Request, chain: &[Vec<u8>]) -> anyhow::Result<crate::pades::Prepared> {
        let bytes = self.bytes_for_save()?;
        crate::pades::prepare(&bytes, self.password.as_deref(), req, chain)
    }

    /// Firma el documento tal como está ahora con `signer`: devuelve el PDF
    /// firmado (no lo escribe).
    pub fn sign_digital(&mut self, req: &crate::pades::Request, signer: &mut dyn crate::pades::Signer) -> anyhow::Result<Vec<u8>> {
        let p = self.prepare_digital(req, &signer.chain())?;
        let sig = signer.sign(p.data_to_sign())?;
        p.finish(&sig)
    }

    pub fn digital_signatures(&self) -> Vec<DigitalSignature> {
        let n = unsafe { FPDF_GetSignatureCount(self.doc) };
        (0..n.max(0)).filter_map(|i| self.read_signature(i)).collect()
    }

    /// El documento lleva los derechos de uso de Adobe Reader (/Perms
    /// /UR3): firmados por Adobe, permiten rellenarlo y guardarlo en Reader.
    pub fn has_usage_rights(&self) -> bool {
        let has = |n: &[u8]| self.base.windows(n.len()).any(|w| w == n);
        has(b"/Perms") && (has(b"/UR3") || has(b"/UR "))
    }

    fn read_signature(&self, i: c_int) -> Option<DigitalSignature> {
        let sig = unsafe { FPDF_GetSignatureObject(self.doc, i) };
        if sig.is_null() {
            return None;
        }
        let contents = read_bytes(|b, l| unsafe { FPDFSignatureObj_GetContents(sig, b as *mut c_void, l) });
        let range = {
            let n = unsafe { FPDFSignatureObj_GetByteRange(sig, std::ptr::null_mut(), 0) } as usize;
            let mut v = vec![0 as c_int; n];
            if n > 0 {
                unsafe { FPDFSignatureObj_GetByteRange(sig, v.as_mut_ptr(), n as c_ulong) };
            }
            v
        };
        let ascii = |f: &dyn Fn(*mut u8, c_ulong) -> c_ulong| {
            let b = read_bytes(f);
            let s = String::from_utf8_lossy(&b).trim_end_matches('\0').trim().to_string();
            (!s.is_empty()).then_some(s)
        };
        let sub_filter = ascii(&|b, l| unsafe { FPDFSignatureObj_GetSubFilter(sig, b as *mut c_char, l) });
        let time = ascii(&|b, l| unsafe { FPDFSignatureObj_GetTime(sig, b as *mut c_char, l) }).and_then(|t| pdf_date(&t));
        let reason = {
            let b = read_bytes(|b, l| unsafe { FPDFSignatureObj_GetReason(sig, b as *mut c_void, l) });
            let u: Vec<u16> = b.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).take_while(|&c| c != 0).collect();
            let s = String::from_utf16_lossy(&u).trim().to_string();
            (!s.is_empty()).then_some(s)
        };
        let mdp = unsafe { FPDFSignatureObj_GetDocMDPPermission(sig) };
        let certification = (mdp != 0).then_some(mdp);

        let mut out = DigitalSignature {
            signer: None,
            issuer: None,
            valid_from: None,
            valid_to: None,
            time,
            reason,
            sub_filter,
            certification,
            state: SignedState::Unknown,
        };
        if let Some(cms) = parse_cms(&contents) {
            out.signer = cms.signer;
            out.issuer = cms.issuer;
            out.valid_from = cms.valid_from;
            out.valid_to = cms.valid_to;
            if out.time.is_none() {
                out.time = cms.signing_time;
            }
            if let (Some(alg), Some(expected)) = (cms.digest_alg, cms.message_digest) {
                out.state = self.check_range(&range, alg, &expected);
            }
        }
        Some(out)
    }

    /// Compara el resumen de los bytes firmados con el de la firma.
    fn check_range(&self, range: &[c_int], alg: ObjectIdentifier, expected: &[u8]) -> SignedState {
        let data = &self.base;
        let mut parts = Vec::new();
        for &[start, len] in range.as_chunks::<2>().0 {
            let (start, len) = (start.max(0) as usize, len.max(0) as usize);
            let Some(part) = data.get(start..start + len) else { return SignedState::Broken };
            parts.push(part);
        }
        let Some(digest) = hash(alg, &parts) else { return SignedState::Unknown };
        if digest != expected {
            return SignedState::Broken;
        }
        let end = range.as_chunks::<2>().0.iter().map(|&[a, l]| a.max(0) as usize + l.max(0) as usize).max().unwrap_or(0);
        if end >= data.len() || only_whitespace(&data[end..]) { SignedState::Intact } else { SignedState::ChangedAfter }
    }
}

fn only_whitespace(b: &[u8]) -> bool {
    b.iter().all(|c| c.is_ascii_whitespace() || *c == 0)
}

/// Llama dos veces a una función de PDFium de las de "dame el tamaño y luego
/// rellena el búfer".
fn read_bytes(f: impl Fn(*mut u8, c_ulong) -> c_ulong) -> Vec<u8> {
    let n = f(std::ptr::null_mut(), 0) as usize;
    if n == 0 {
        return Vec::new();
    }
    let mut v = vec![0u8; n];
    let got = f(v.as_mut_ptr(), n as c_ulong) as usize;
    v.truncate(got.min(n));
    v
}

/// "D:20261014093000+02'00'" → "2026-10-14 09:30".
fn pdf_date(s: &str) -> Option<String> {
    let d = s.trim().trim_start_matches("D:");
    if d.len() < 8 || !d[..8].bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut out = format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8]);
    if d.len() >= 12 && d[8..12].bytes().all(|c| c.is_ascii_digit()) {
        out += &format!(" {}:{}", &d[8..10], &d[10..12]);
    }
    Some(out)
}

const OID_SHA1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.14.3.2.26");
const OID_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
const OID_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.2");
const OID_SHA512: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.3");
const OID_MESSAGE_DIGEST: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
const OID_SIGNING_TIME: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.5");
const OID_CN: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.3");
const OID_O: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.10");

fn hash(alg: ObjectIdentifier, parts: &[&[u8]]) -> Option<Vec<u8>> {
    fn run<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
        let mut h = D::new();
        for p in parts {
            h.update(p);
        }
        h.finalize().to_vec()
    }
    Some(match alg {
        OID_SHA1 => run::<sha1::Sha1>(parts),
        OID_SHA256 => run::<sha2::Sha256>(parts),
        OID_SHA384 => run::<sha2::Sha384>(parts),
        OID_SHA512 => run::<sha2::Sha512>(parts),
        _ => return None,
    })
}

struct CmsInfo {
    signer: Option<String>,
    issuer: Option<String>,
    valid_from: Option<String>,
    valid_to: Option<String>,
    signing_time: Option<String>,
    digest_alg: Option<ObjectIdentifier>,
    message_digest: Option<Vec<u8>>,
}

/// Lee el CMS (SignedData) de /Contents, que viene relleno de ceros.
fn parse_cms(contents: &[u8]) -> Option<CmsInfo> {
    let mut reader = der::SliceReader::new(contents).ok()?;
    let ci = ContentInfo::decode(&mut reader).ok()?;
    let sd: SignedData = ci.content.decode_as().ok()?;
    let si = sd.signer_infos.0.iter().next()?;
    let cert = sd.certificates.as_ref().and_then(|set| {
        set.0.iter().find_map(|c| match c {
            CertificateChoices::Certificate(cert) => {
                let tbs = &cert.tbs_certificate;
                let matches = match &si.sid {
                    SignerIdentifier::IssuerAndSerialNumber(ias) => ias.serial_number == tbs.serial_number && ias.issuer == tbs.issuer,
                    SignerIdentifier::SubjectKeyIdentifier(_) => true,
                };
                matches.then_some(cert)
            }
            _ => None,
        })
    });
    let mut message_digest = None;
    let mut signing_time = None;
    if let Some(attrs) = &si.signed_attrs {
        for a in attrs.iter() {
            let Some(v) = a.values.iter().next() else { continue };
            if a.oid == OID_MESSAGE_DIGEST {
                message_digest = v.decode_as::<OctetStringRef>().ok().map(|o| o.as_bytes().to_vec());
            } else if a.oid == OID_SIGNING_TIME
                && let Ok(t) = v.to_der().and_then(|d| x509_cert::time::Time::from_der(&d))
            {
                signing_time = Some(time_str(&t, true));
            }
        }
    }
    let (signer, issuer, valid_from, valid_to) = match cert {
        Some(c) => {
            let tbs = &c.tbs_certificate;
            (
                name_of(&tbs.subject),
                name_of(&tbs.issuer),
                Some(time_str(&tbs.validity.not_before, false)),
                Some(time_str(&tbs.validity.not_after, false)),
            )
        }
        None => (None, None, None, None),
    };
    Some(CmsInfo { signer, issuer, valid_from, valid_to, signing_time, digest_alg: Some(si.digest_alg.oid), message_digest })
}

/// El nombre común (CN) de un nombre X.509, o la organización, o entero.
pub(crate) fn name_of(name: &x509_cert::name::Name) -> Option<String> {
    let find = |oid: ObjectIdentifier| {
        name.0.iter().flat_map(|rdn| rdn.0.iter()).find(|atv| atv.oid == oid).and_then(|atv| {
            let bytes = atv.value.value();
            std::str::from_utf8(bytes).ok().map(str::to_string)
        })
    };
    find(OID_CN).or_else(|| find(OID_O)).or_else(|| {
        let s = name.to_string();
        (!s.is_empty()).then_some(s)
    })
}

pub(crate) fn time_str(t: &x509_cert::time::Time, with_time: bool) -> String {
    let dt = t.to_date_time();
    if with_time {
        format!("{:04}-{:02}-{:02} {:02}:{:02}", dt.year(), dt.month(), dt.day(), dt.hour(), dt.minutes())
    } else {
        format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fechas_pdf() {
        assert_eq!(pdf_date("D:20261014093000+02'00'").as_deref(), Some("2026-10-14 09:30"));
        assert_eq!(pdf_date("D:20261014").as_deref(), Some("2026-10-14"));
        assert_eq!(pdf_date("ayer"), None);
    }
}
