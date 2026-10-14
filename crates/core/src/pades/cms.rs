//! La firma CMS (PKCS#7) separada que va en `/Contents`, con los atributos
//! firmados que pide PAdES básico (B-B): tipo de contenido, resumen del
//! documento y el certificado del firmante (signing-certificate-v2). La
//! hora no va en los atributos (PAdES la pone en `/M` del diccionario).

use anyhow::{Result, anyhow, bail};
use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::{CmsVersion, ContentInfo};
use cms::signed_data::{EncapsulatedContentInfo, SignedData, SignerIdentifier, SignerInfo, SignerInfos};
use der::asn1::{Any, ObjectIdentifier, OctetString, SetOfVec};
use der::{Decode, Encode, Sequence};
use sha2::{Digest, Sha256};
use x509_cert::Certificate;
use x509_cert::attr::Attribute;
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::AlgorithmIdentifierOwned;

const OID_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
const OID_SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");
const OID_CONTENT_TYPE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
const OID_MESSAGE_DIGEST: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
const OID_SIGNING_CERT_V2: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");
const OID_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
const OID_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const OID_EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
const OID_ECDSA_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");

/// ESSCertIDv2 (RFC 5035), con SHA-256 (el algoritmo por defecto, que no se
/// escribe).
#[derive(Sequence)]
struct EssCertIdV2 {
    cert_hash: OctetString,
    issuer_serial: IssuerSerial,
}

#[derive(Sequence)]
struct IssuerSerial {
    issuer: Vec<GeneralName>,
    serial_number: SerialNumber,
}

#[derive(Sequence)]
struct SigningCertificateV2 {
    certs: Vec<EssCertIdV2>,
}

/// Tipo de clave del certificado del firmante.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Rsa,
    Ec,
}

pub fn key_kind(cert_der: &[u8]) -> Result<KeyKind> {
    let cert = Certificate::from_der(cert_der).map_err(|e| anyhow!("{e}"))?;
    match cert.tbs_certificate.subject_public_key_info.algorithm.oid {
        OID_RSA => Ok(KeyKind::Rsa),
        OID_EC_PUBLIC_KEY => Ok(KeyKind::Ec),
        o => bail!("{}", crate::t!("pades.key_type", o)),
    }
}

fn attribute(oid: ObjectIdentifier, value: Any) -> Result<Attribute> {
    Ok(Attribute { oid, values: set(vec![value])? })
}

/// Un SET OF en orden DER (lo exige la codificación y la firma).
fn set<T: der::DerOrd>(items: Vec<T>) -> Result<SetOfVec<T>> {
    SetOfVec::try_from(items).map_err(|e| anyhow!("{e}"))
}

fn any<T: Encode>(v: &T) -> Result<Any> {
    Any::from_der(&v.to_der().map_err(|e| anyhow!("{e}"))?).map_err(|e| anyhow!("{e}"))
}

/// Los atributos firmados para el resumen `digest` (SHA-256 de los rangos
/// del PDF) y el certificado `cert_der`.
pub fn signed_attributes(digest: &[u8], cert_der: &[u8]) -> Result<SetOfVec<Attribute>> {
    let cert = Certificate::from_der(cert_der).map_err(|e| anyhow!("{e}"))?;
    let ess = SigningCertificateV2 {
        certs: vec![EssCertIdV2 {
            cert_hash: OctetString::new(Sha256::digest(cert_der).to_vec())?,
            issuer_serial: IssuerSerial {
                issuer: vec![GeneralName::DirectoryName(cert.tbs_certificate.issuer.clone())],
                serial_number: cert.tbs_certificate.serial_number.clone(),
            },
        }],
    };
    set(vec![
        attribute(OID_CONTENT_TYPE, any(&OID_DATA)?)?,
        attribute(OID_MESSAGE_DIGEST, any(&OctetString::new(digest.to_vec())?)?)?,
        attribute(OID_SIGNING_CERT_V2, any(&ess)?)?,
    ])
}

/// Lo que firma la clave: los atributos firmados codificados como SET.
pub fn to_be_signed(attrs: &SetOfVec<Attribute>) -> Result<Vec<u8>> {
    attrs.to_der().map_err(|e| anyhow!("{e}"))
}

/// Pasa una firma ECDSA "r||s" (la de PKCS#11 o Windows) a DER; las que ya
/// están en DER se quedan igual.
pub fn ecdsa_der(sig: &[u8]) -> Vec<u8> {
    if sig.first() == Some(&0x30) && sig.len() > 8 && sig[1] as usize + 2 == sig.len() {
        return sig.to_vec();
    }
    let half = sig.len() / 2;
    let int = |b: &[u8]| {
        let b: Vec<u8> = b.iter().copied().skip_while(|&x| x == 0).collect();
        let mut v = if b.first().is_some_and(|&x| x & 0x80 != 0) || b.is_empty() { vec![0] } else { vec![] };
        v.extend(b);
        let mut out = vec![0x02, v.len() as u8];
        out.extend(v);
        out
    };
    let mut body = int(&sig[..half]);
    body.extend(int(&sig[half..]));
    let mut out = vec![0x30, body.len() as u8];
    out.extend(body);
    out
}

/// El ContentInfo SignedData completo (DER).
pub fn signed_data(chain: &[Vec<u8>], attrs: SetOfVec<Attribute>, signature: &[u8]) -> Result<Vec<u8>> {
    let leaf = Certificate::from_der(chain.first().ok_or_else(|| anyhow!("sin certificado"))?).map_err(|e| anyhow!("{e}"))?;
    let kind = key_kind(&chain[0])?;
    let (sig_alg, signature) = match kind {
        KeyKind::Rsa => (AlgorithmIdentifierOwned { oid: OID_RSA, parameters: Some(Any::null()) }, signature.to_vec()),
        KeyKind::Ec => (AlgorithmIdentifierOwned { oid: OID_ECDSA_SHA256, parameters: None }, ecdsa_der(signature)),
    };
    let sha256 = AlgorithmIdentifierOwned { oid: OID_SHA256, parameters: None };
    let signer = SignerInfo {
        version: CmsVersion::V1,
        sid: SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
            issuer: leaf.tbs_certificate.issuer.clone(),
            serial_number: leaf.tbs_certificate.serial_number.clone(),
        }),
        digest_alg: sha256.clone(),
        signed_attrs: Some(attrs),
        signature_algorithm: sig_alg,
        signature: OctetString::new(signature)?,
        unsigned_attrs: None,
    };
    let mut certs = Vec::new();
    for c in chain {
        certs.push(CertificateChoices::Certificate(Certificate::from_der(c).map_err(|e| anyhow!("{e}"))?));
    }
    let sd = SignedData {
        version: CmsVersion::V1,
        digest_algorithms: set(vec![sha256])?,
        encap_content_info: EncapsulatedContentInfo { econtent_type: OID_DATA, econtent: None },
        certificates: Some(cms::signed_data::CertificateSet(set(certs)?)),
        crls: None,
        signer_infos: SignerInfos(set(vec![signer])?),
    };
    let ci = ContentInfo { content_type: OID_SIGNED_DATA, content: any(&sd)? };
    ci.to_der().map_err(|e| anyhow!("{e}"))
}
