//! Datos de un certificado X.509 para enseñarlo al elegir con cuál firmar.

use der::Decode;
use der::asn1::ObjectIdentifier;
use x509_cert::Certificate;

const OID_CN: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.3");
const OID_O: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.10");
const OID_SERIAL_NUMBER: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.5");
const OID_KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.15");
const OID_BASIC_CONSTRAINTS: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.19");

/// Un certificado, resumido.
#[derive(Debug, Clone, PartialEq)]
pub struct CertInfo {
    /// Titular (el nombre común, o la organización).
    pub subject: String,
    /// Quien lo emitió.
    pub issuer: String,
    /// Número de serie del titular (el NIF en los de la FNMT y el DNIe).
    pub holder_id: Option<String>,
    /// Válido desde / hasta, como "2026-10-14".
    pub valid_from: String,
    pub valid_to: String,
    /// Caducado o aún no válido.
    pub expired: bool,
    /// Sirve para firmar: su uso de clave permite la firma (o no lo limita)
    /// y no es de una autoridad de certificación.
    pub for_signing: bool,
}

/// Resume el certificado DER `der`.
pub fn cert_info(der: &[u8]) -> Option<CertInfo> {
    let cert = Certificate::from_der(der).ok()?;
    let tbs = &cert.tbs_certificate;
    let v = &tbs.validity;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?;
    let expired = now < v.not_before.to_unix_duration() || now > v.not_after.to_unix_duration();
    let ext = |oid: ObjectIdentifier| tbs.extensions.as_ref().and_then(|e| e.iter().find(|x| x.extn_id == oid));
    // keyUsage: BIT STRING; bit 0 digitalSignature, bit 1 nonRepudiation.
    let can_sign = ext(OID_KEY_USAGE).is_none_or(|e| {
        der::asn1::BitString::from_der(e.extn_value.as_bytes()).is_ok_and(|b| b.bits().take(2).any(|x| x))
    });
    // basicConstraints con cA = TRUE.
    let is_ca = ext(OID_BASIC_CONSTRAINTS).is_some_and(|e| {
        x509_cert::ext::pkix::BasicConstraints::from_der(e.extn_value.as_bytes()).is_ok_and(|b| b.ca)
    });
    Some(CertInfo {
        subject: name_of(&tbs.subject).unwrap_or_default(),
        issuer: name_of(&tbs.issuer).unwrap_or_default(),
        holder_id: attr(&tbs.subject, OID_SERIAL_NUMBER),
        valid_from: date(&v.not_before),
        valid_to: date(&v.not_after),
        expired,
        for_signing: can_sign && !is_ca,
    })
}

/// Emisor y titular del certificado (DER), para montar la cadena.
pub fn issuer_and_subject(der: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    use der::Encode;
    let cert = Certificate::from_der(der).ok()?;
    Some((cert.tbs_certificate.issuer.to_der().ok()?, cert.tbs_certificate.subject.to_der().ok()?))
}

/// Ordena una cadena: `leaf` y detrás, de los `others`, su emisor, el
/// emisor de este... (sin la raíz autofirmada).
pub fn build_chain(leaf: &[u8], others: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut chain = vec![leaf.to_vec()];
    while chain.len() < 8 {
        let Some((issuer, subject)) = issuer_and_subject(chain.last().expect("hay hoja")) else { break };
        if issuer == subject {
            break;
        }
        let next = others.iter().find(|c| issuer_and_subject(c).is_some_and(|(i, s)| s == issuer && i != s));
        match next {
            Some(c) if !chain.contains(c) => chain.push(c.clone()),
            _ => break,
        }
    }
    chain
}

fn attr(name: &x509_cert::name::Name, oid: ObjectIdentifier) -> Option<String> {
    name.0.iter().flat_map(|rdn| rdn.0.iter()).find(|atv| atv.oid == oid).and_then(|atv| std::str::from_utf8(atv.value.value()).ok().map(str::to_string))
}

/// El nombre común (CN) de un nombre X.509, o la organización, o entero.
pub fn name_of(name: &x509_cert::name::Name) -> Option<String> {
    attr(name, OID_CN).or_else(|| attr(name, OID_O)).or_else(|| {
        let s = name.to_string();
        (!s.is_empty()).then_some(s)
    })
}

fn date(t: &x509_cert::time::Time) -> String {
    let d = t.to_date_time();
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificado_de_prueba() {
        let pem = include_str!("../../tests/fixtures/certs/ca.pem");
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = base64_decode(&b64);
        let info = cert_info(&der).expect("certificado");
        assert_eq!(info.subject, "Autoridad de certificacion de PRUEBA");
        assert!(!info.for_signing, "una CA no firma documentos");
    }

    fn base64_decode(s: &str) -> Vec<u8> {
        let val = |c: u8| match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        let bytes: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
        let mut out = Vec::new();
        for chunk in bytes.chunks(4) {
            let mut n = 0u32;
            for (i, &c) in chunk.iter().enumerate() {
                n |= (val(c) as u32) << (18 - 6 * i);
            }
            for i in 0..chunk.len().saturating_sub(1) {
                out.push((n >> (16 - 8 * i)) as u8);
            }
        }
        out
    }
}
