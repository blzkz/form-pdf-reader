//! Certificado en un fichero PKCS#12 (`.p12`, `.pfx`), como el que exporta
//! la FNMT o un navegador: la clave privada y su cadena de certificados,
//! protegidas con una contraseña. La clave solo está en memoria mientras se
//! firma y se borra al soltar el `P12Key`.

use anyhow::{Result, anyhow, bail};
use der::{Decode, Encode};
use p12_keystore::{KeyStore, KeyStoreEntry, Pkcs12ImportPolicy};
use rsa::pkcs8::DecodePrivateKey;
use sha2::{Digest, Sha256};
use spki_compat::public_key_der;

use super::Signer;

enum Key {
    Rsa(Box<rsa::RsaPrivateKey>),
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
}

/// La contraseña del fichero no es la buena.
#[derive(Debug)]
pub struct WrongPassword;

impl std::fmt::Display for WrongPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::t!("pades.p12_password"))
    }
}

impl std::error::Error for WrongPassword {}

/// La clave y los certificados de un PKCS#12.
pub struct P12Key {
    key: Key,
    chain: Vec<Vec<u8>>,
}

impl P12Key {
    /// Abre el PKCS#12 `data` con `password`.
    pub fn open(data: &[u8], password: &str) -> Result<P12Key> {
        let store = KeyStore::from_pkcs12(data, password, Pkcs12ImportPolicy::Relaxed).map_err(|e| match e {
            // La contraseña no cuadra con el código de integridad (MAC) o el
            // descifrado da basura.
            p12_keystore::error::Error::MacError(_) | p12_keystore::error::Error::UnpadError => anyhow::Error::new(WrongPassword),
            e => anyhow!("{}", crate::t!("pades.p12_invalid", e)),
        })?;
        let chain = store
            .entries()
            .find_map(|(_, e)| match e {
                KeyStoreEntry::PrivateKeyChain(c) => Some(c),
                _ => None,
            })
            .ok_or_else(|| anyhow!("{}", crate::t!("pades.p12_no_key")))?;
        let pkcs8 = chain.key().as_der();
        let key = if let Ok(k) = rsa::RsaPrivateKey::from_pkcs8_der(pkcs8) {
            Key::Rsa(Box::new(k))
        } else if let Ok(k) = p256::ecdsa::SigningKey::from_pkcs8_der(pkcs8) {
            Key::P256(k)
        } else if let Ok(k) = p384::ecdsa::SigningKey::from_pkcs8_der(pkcs8) {
            Key::P384(k)
        } else {
            bail!("{}", crate::t!("pades.p12_key_type"));
        };
        // El certificado de la clave primero; los demás detrás.
        let public = match &key {
            Key::Rsa(k) => public_key_der(&rsa::RsaPublicKey::from(k.as_ref())),
            Key::P256(k) => public_key_der(k.verifying_key()),
            Key::P384(k) => public_key_der(k.verifying_key()),
        };
        let mut certs: Vec<Vec<u8>> = chain.certs().iter().map(|c| c.as_der().to_vec()).collect();
        let leaf = certs.iter().position(|c| spki_of(c).is_some_and(|s| Some(&s) == public.as_ref())).ok_or_else(|| anyhow!("{}", crate::t!("pades.p12_no_cert")))?;
        let leaf = certs.remove(leaf);
        certs.insert(0, leaf);
        Ok(P12Key { key, chain: certs })
    }
}

/// La clave pública (SubjectPublicKeyInfo, DER) del certificado.
fn spki_of(cert: &[u8]) -> Option<Vec<u8>> {
    x509_cert::Certificate::from_der(cert).ok()?.tbs_certificate.subject_public_key_info.to_der().ok()
}

impl Signer for P12Key {
    fn chain(&self) -> Vec<Vec<u8>> {
        self.chain.clone()
    }

    fn sign(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        use p256::ecdsa::signature::Signer as _;
        use p384::ecdsa::signature::hazmat::PrehashSigner as _;
        Ok(match &self.key {
            Key::Rsa(k) => {
                let digest = Sha256::digest(data);
                k.sign(rsa::Pkcs1v15Sign::new::<Sha256>(), &digest).map_err(|e| anyhow!("RSA: {e}"))?
            }
            Key::P256(k) => {
                let s: p256::ecdsa::Signature = k.try_sign(data).map_err(|e| anyhow!("ECDSA: {e}"))?;
                s.to_der().as_bytes().to_vec()
            }
            Key::P384(k) => {
                // Con SHA-256, como el resto de la firma (PAdES lo permite).
                let digest = Sha256::digest(data);
                let s: p384::ecdsa::Signature = k.sign_prehash(&digest).map_err(|e| anyhow!("ECDSA: {e}"))?;
                s.to_der().as_bytes().to_vec()
            }
        })
    }
}

mod spki_compat {
    use rsa::pkcs8::EncodePublicKey;

    /// La clave pública en DER (SubjectPublicKeyInfo), para compararla con
    /// la de los certificados.
    pub fn public_key_der(k: &impl EncodePublicKey) -> Option<Vec<u8>> {
        k.to_public_key_der().ok().map(|d| d.as_bytes().to_vec())
    }
}
