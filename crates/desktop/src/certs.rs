//! Certificados con los que firmar: los del sistema (en Windows, su almacén,
//! que incluye el DNIe y las tarjetas con lector; en Linux, las tarjetas y el
//! DNIe por PKCS#11 y los certificados de Chrome y Firefox) y los de un
//! fichero `.p12`/`.pfx`.
//!
//! La clave privada no sale de donde está: el sistema o la tarjeta firman.
//! La de un fichero se lee al firmar y se borra de memoria al terminar.

use std::path::{Path, PathBuf};

use anyhow::Result;
use form_pdf_reader::pades::{self, CertInfo, P12Key, Signer};
use form_pdf_reader::t;

#[cfg(not(windows))]
#[path = "certs/pkcs11.rs"]
mod backend;
#[cfg(windows)]
#[path = "certs/winstore.rs"]
mod backend;

/// Qué hay que pedir antes de firmar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Secret {
    /// Nada (o lo pide el propio sistema, como Windows con el DNIe).
    None,
    /// El PIN de la tarjeta (en Windows lo pide el sistema).
    #[cfg_attr(windows, allow(dead_code))]
    Pin,
    /// La contraseña del fichero, o la maestra del navegador.
    Password,
}

#[derive(Debug, Clone)]
pub enum Source {
    File(PathBuf),
    System(backend::Handle),
}

/// Un certificado que se puede elegir.
#[derive(Debug, Clone)]
pub struct CertEntry {
    pub info: CertInfo,
    /// El del firmante y sus intermedios (DER).
    pub chain: Vec<Vec<u8>>,
    pub source: Source,
    pub secret: Secret,
    /// De dónde viene, para enseñarlo ("Almacén de Windows", "DNIe"...).
    pub origin: String,
    /// En pruebas: tarjetas y DNIe (no se ha podido probar con todas).
    pub beta: bool,
}

/// Los certificados del sistema (puede tardar: tarjetas, bases de datos).
pub fn system_certificates() -> Vec<CertEntry> {
    let mut v = backend::list();
    v.sort_by(|a, b| a.info.expired.cmp(&b.info.expired).then(a.info.subject.to_lowercase().cmp(&b.info.subject.to_lowercase())));
    v
}

/// Lee un fichero `.p12`/`.pfx` con su contraseña.
pub fn open_file(path: &Path, password: &str) -> Result<CertEntry> {
    let data = std::fs::read(path)?;
    let key = P12Key::open(&data, password)?;
    let chain = key.chain();
    let info = pades::cert_info(&chain[0]).ok_or_else(|| anyhow::anyhow!("{}", t!("pades.bad_cert")))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(CertEntry { info, chain, source: Source::File(path.to_path_buf()), secret: Secret::Password, origin: t!("dsig.origin_file", name), beta: false })
}

/// Quien firma con este certificado; `secret` es el PIN o la contraseña.
pub fn signer(e: &CertEntry, secret: &str) -> Result<Box<dyn Signer>> {
    match &e.source {
        Source::File(p) => Ok(Box::new(P12Key::open(&std::fs::read(p)?, secret)?)),
        Source::System(h) => backend::signer(h, &e.chain, secret),
    }
}
