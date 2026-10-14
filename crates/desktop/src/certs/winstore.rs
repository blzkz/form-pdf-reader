//! Windows: el almacén de certificados del usuario ("Personal"). Ahí están
//! los certificados instalados (FNMT, de empresa...) y los del DNIe y otras
//! tarjetas con lector, que pone su controlador. Firma Windows (CNG, o
//! CryptoAPI con los proveedores antiguos) y pide él el PIN si hace falta.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use anyhow::{Result, anyhow, bail};
use form_pdf_reader::pades::{self, KeyKind, Signer};
use form_pdf_reader::t;

use super::{CertEntry, Secret, Source};

type Handle0 = isize;

/// Dónde está un certificado: su huella (SHA-1) en el almacén.
#[derive(Debug, Clone)]
pub struct Handle {
    thumbprint: Vec<u8>,
}

#[repr(C)]
struct CertContext {
    encoding: u32,
    encoded: *const u8,
    encoded_len: u32,
    info: *const c_void,
    store: Handle0,
}

#[repr(C)]
struct HashBlob {
    len: u32,
    data: *const u8,
}

#[repr(C)]
struct ChainPara {
    size: u32,
    // CERT_USAGE_MATCH: dwType + CERT_ENHKEY_USAGE { cUsageIdentifier, rgpszUsageIdentifier }.
    usage_type: u32,
    usage_count: u32,
    usage_ids: *const c_void,
}

#[repr(C)]
struct ChainContext {
    size: u32,
    trust: [u32; 2],
    chain_count: u32,
    chains: *const *const SimpleChain,
}

#[repr(C)]
struct SimpleChain {
    size: u32,
    trust: [u32; 2],
    element_count: u32,
    elements: *const *const ChainElement,
}

#[repr(C)]
struct ChainElement {
    size: u32,
    cert: *const CertContext,
}

#[repr(C)]
struct Pkcs1Padding {
    alg: *const u16,
}

#[link(name = "crypt32")]
unsafe extern "system" {
    fn CertOpenStore(provider: *const i8, encoding: u32, prov: isize, flags: u32, para: *const c_void) -> Handle0;
    fn CertCloseStore(store: Handle0, flags: u32) -> i32;
    fn CertEnumCertificatesInStore(store: Handle0, prev: *const CertContext) -> *const CertContext;
    fn CertFindCertificateInStore(store: Handle0, encoding: u32, flags: u32, find_type: u32, para: *const c_void, prev: *const CertContext) -> *const CertContext;
    fn CertFreeCertificateContext(ctx: *const CertContext) -> i32;
    fn CertGetCertificateContextProperty(ctx: *const CertContext, prop: u32, data: *mut c_void, len: *mut u32) -> i32;
    fn CertGetCertificateChain(engine: isize, ctx: *const CertContext, time: *const c_void, store: Handle0, para: *const ChainPara, flags: u32, reserved: *const c_void, chain: *mut *const ChainContext) -> i32;
    fn CertFreeCertificateChain(chain: *const ChainContext);
    fn CryptAcquireCertificatePrivateKey(ctx: *const CertContext, flags: u32, para: *const c_void, key: *mut usize, key_spec: *mut u32, free: *mut i32) -> i32;
}

#[link(name = "ncrypt")]
unsafe extern "system" {
    fn NCryptSignHash(key: usize, padding: *const c_void, hash: *const u8, hash_len: u32, sig: *mut u8, sig_len: u32, result: *mut u32, flags: u32) -> i32;
    fn NCryptSetProperty(obj: usize, name: *const u16, data: *const u8, len: u32, flags: u32) -> i32;
    fn NCryptFreeObject(obj: usize) -> i32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn CryptCreateHash(prov: usize, alg: u32, key: usize, flags: u32, hash: *mut usize) -> i32;
    fn CryptSetHashParam(hash: usize, param: u32, data: *const u8, flags: u32) -> i32;
    fn CryptSignHashW(hash: usize, key_spec: u32, desc: *const u16, flags: u32, sig: *mut u8, len: *mut u32) -> i32;
    fn CryptDestroyHash(hash: usize) -> i32;
    fn CryptReleaseContext(prov: usize, flags: u32) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

const CERT_STORE_PROV_SYSTEM_W: *const i8 = 10 as *const i8;
const CERT_SYSTEM_STORE_CURRENT_USER: u32 = 1 << 16;
const CERT_STORE_READONLY_FLAG: u32 = 0x8000;
const ENCODING: u32 = 0x0001_0001;
const CERT_FIND_HASH: u32 = 0x10000;
const CERT_KEY_PROV_INFO_PROP_ID: u32 = 2;
const CERT_HASH_PROP_ID: u32 = 3;
const CRYPT_ACQUIRE_CACHE_FLAG: u32 = 0x1;
const CRYPT_ACQUIRE_WINDOW_HANDLE_FLAG: u32 = 0x80;
const CRYPT_ACQUIRE_PREFER_NCRYPT_KEY_FLAG: u32 = 0x20000;
const CERT_NCRYPT_KEY_SPEC: u32 = 0xFFFF_FFFF;
const BCRYPT_PAD_PKCS1: u32 = 2;
const CALG_SHA_256: u32 = 0x800c;
const HP_HASHVAL: u32 = 2;
/// HRESULT de "operación cancelada por el usuario" (SCARD_W_CANCELLED_BY_USER y similares).
const CANCELLED: [u32; 3] = [0x8010_006E, 0x8009_0036, 0x800704C7];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct Store(Handle0);

impl Store {
    fn open() -> Result<Store> {
        let name = wide("MY");
        let h = unsafe { CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, CERT_SYSTEM_STORE_CURRENT_USER | CERT_STORE_READONLY_FLAG, name.as_ptr() as *const c_void) };
        if h == 0 {
            bail!("{}", t!("dsig.store_failed", unsafe { GetLastError() }));
        }
        Ok(Store(h))
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        unsafe { CertCloseStore(self.0, 0) };
    }
}

unsafe fn der_of(ctx: *const CertContext) -> Vec<u8> {
    let c = unsafe { &*ctx };
    unsafe { std::slice::from_raw_parts(c.encoded, c.encoded_len as usize) }.to_vec()
}

fn prop(ctx: *const CertContext, id: u32) -> Option<Vec<u8>> {
    let mut len = 0u32;
    if unsafe { CertGetCertificateContextProperty(ctx, id, null_mut(), &mut len) } == 0 {
        return None;
    }
    let mut v = vec![0u8; len as usize];
    (unsafe { CertGetCertificateContextProperty(ctx, id, v.as_mut_ptr() as *mut c_void, &mut len) } != 0).then_some(v)
}

/// La cadena del certificado según Windows (sin la raíz).
fn chain_of(ctx: *const CertContext) -> Vec<Vec<u8>> {
    let mut out = vec![unsafe { der_of(ctx) }];
    let para = ChainPara { size: size_of::<ChainPara>() as u32, usage_type: 0, usage_count: 0, usage_ids: null() };
    let mut chain: *const ChainContext = null();
    if unsafe { CertGetCertificateChain(0, ctx, null(), 0, &para, 0, null(), &mut chain) } == 0 || chain.is_null() {
        return out;
    }
    unsafe {
        let c = &*chain;
        if c.chain_count > 0 {
            let simple = &**c.chains;
            for i in 1..simple.element_count as usize {
                let el = &**simple.elements.add(i);
                let der = der_of(el.cert);
                if pades::cert::issuer_and_subject(&der).is_some_and(|(i, s)| i == s) {
                    break;
                }
                out.push(der);
            }
        }
        CertFreeCertificateChain(chain);
    }
    out
}

pub fn list() -> Vec<CertEntry> {
    let Ok(store) = Store::open() else { return Vec::new() };
    let mut out = Vec::new();
    let mut ctx: *const CertContext = null();
    loop {
        ctx = unsafe { CertEnumCertificatesInStore(store.0, ctx) };
        if ctx.is_null() {
            break;
        }
        // Solo los que tienen clave privada.
        if prop(ctx, CERT_KEY_PROV_INFO_PROP_ID).is_none() {
            continue;
        }
        let der = unsafe { der_of(ctx) };
        let Some(info) = pades::cert_info(&der) else { continue };
        let Some(thumbprint) = prop(ctx, CERT_HASH_PROP_ID) else { continue };
        if !info.for_signing {
            continue;
        }
        out.push(CertEntry { info, chain: chain_of(ctx), source: Source::System(Handle { thumbprint }), secret: Secret::None, origin: t!("dsig.origin_windows") });
    }
    out
}

/// Firma con la clave del almacén (o de la tarjeta, a través de él).
struct StoreSigner {
    _store: Store,
    ctx: *const CertContext,
    chain: Vec<Vec<u8>>,
    kind: KeyKind,
}

pub fn signer(h: &Handle, chain: &[Vec<u8>], _pin: &str) -> Result<Box<dyn Signer>> {
    let store = Store::open()?;
    let blob = HashBlob { len: h.thumbprint.len() as u32, data: h.thumbprint.as_ptr() };
    let ctx = unsafe { CertFindCertificateInStore(store.0, ENCODING, 0, CERT_FIND_HASH, &blob as *const HashBlob as *const c_void, null()) };
    if ctx.is_null() {
        bail!("{}", t!("dsig.no_key"));
    }
    let kind = pades::key_kind(&chain[0])?;
    Ok(Box::new(StoreSigner { _store: store, ctx, chain: chain.to_vec(), kind }))
}

impl Drop for StoreSigner {
    fn drop(&mut self) {
        unsafe { CertFreeCertificateContext(self.ctx) };
    }
}

fn win_error(code: u32) -> anyhow::Error {
    if CANCELLED.contains(&code) { anyhow!("{}", t!("print.cancelled")) } else { anyhow!("{}", t!("dsig.sign_failed", format!("0x{code:08X}"))) }
}

impl Signer for StoreSigner {
    fn chain(&self) -> Vec<Vec<u8>> {
        self.chain.clone()
    }

    fn sign(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(data);
        let hwnd = crate::snap::hwnd();
        let (mut key, mut spec, mut free) = (0usize, 0u32, 0i32);
        let flags = CRYPT_ACQUIRE_PREFER_NCRYPT_KEY_FLAG | CRYPT_ACQUIRE_CACHE_FLAG | if hwnd != 0 { CRYPT_ACQUIRE_WINDOW_HANDLE_FLAG } else { 0 };
        if unsafe { CryptAcquireCertificatePrivateKey(self.ctx, flags, &hwnd as *const isize as *const c_void, &mut key, &mut spec, &mut free) } == 0 {
            return Err(win_error(unsafe { GetLastError() }));
        }
        let result = if spec == CERT_NCRYPT_KEY_SPEC {
            // Ventana de la que cuelga el diálogo del PIN.
            let name = wide("HWND Handle");
            unsafe { NCryptSetProperty(key, name.as_ptr(), &hwnd as *const isize as *const u8, size_of::<isize>() as u32, 0) };
            let alg = wide("SHA256");
            let pad = Pkcs1Padding { alg: alg.as_ptr() };
            let (padding, pflags) = match self.kind {
                KeyKind::Rsa => (&pad as *const Pkcs1Padding as *const c_void, BCRYPT_PAD_PKCS1),
                KeyKind::Ec => (null(), 0),
            };
            let mut len = 0u32;
            let st = unsafe { NCryptSignHash(key, padding, digest.as_ptr(), 32, null_mut(), 0, &mut len, pflags) };
            if st != 0 {
                Err(win_error(st as u32))
            } else {
                let mut sig = vec![0u8; len as usize];
                let st = unsafe { NCryptSignHash(key, padding, digest.as_ptr(), 32, sig.as_mut_ptr(), len, &mut len, pflags) };
                sig.truncate(len as usize);
                if st != 0 { Err(win_error(st as u32)) } else { Ok(sig) }
            }
        } else {
            // CryptoAPI antigua: firma al revés (little-endian).
            let mut hash = 0usize;
            let r = unsafe {
                if CryptCreateHash(key, CALG_SHA_256, 0, 0, &mut hash) == 0 || CryptSetHashParam(hash, HP_HASHVAL, digest.as_ptr(), 0) == 0 {
                    Err(win_error(GetLastError()))
                } else {
                    let mut len = 0u32;
                    if CryptSignHashW(hash, spec, null(), 0, null_mut(), &mut len) == 0 {
                        Err(win_error(GetLastError()))
                    } else {
                        let mut sig = vec![0u8; len as usize];
                        if CryptSignHashW(hash, spec, null(), 0, sig.as_mut_ptr(), &mut len) == 0 {
                            Err(win_error(GetLastError()))
                        } else {
                            sig.truncate(len as usize);
                            sig.reverse();
                            Ok(sig)
                        }
                    }
                }
            };
            if hash != 0 {
                unsafe { CryptDestroyHash(hash) };
            }
            r
        };
        if free != 0 {
            unsafe {
                if spec == CERT_NCRYPT_KEY_SPEC {
                    NCryptFreeObject(key);
                } else {
                    CryptReleaseContext(key, 0);
                }
            }
        }
        result
    }
}
