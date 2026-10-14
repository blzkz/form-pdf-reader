//! Linux: certificados por PKCS#11.
//!
//! - Tarjetas y DNIe: el módulo de OpenSC o el oficial del DNIe
//!   (`libpkcs11-dnie.so`), si están instalados.
//! - Chrome/Chromium (`~/.pki/nssdb`) y Firefox (sus perfiles): sus bases de
//!   datos NSS, con el módulo de NSS (`libsoftokn3.so`), solo para leer.
//!
//! `FORM_PDF_READER_NSSDB` (carpetas separadas por `:`) sustituye la lista
//! de bases NSS, y `FORM_PDF_READER_PKCS11` añade módulos (para pruebas).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use anyhow::{Result, anyhow, bail};
use cryptoki_sys::*;
use form_pdf_reader::pades::{self, KeyKind, Signer};
use form_pdf_reader::t;

use super::{CertEntry, Secret, Source};

/// Dónde está un certificado.
#[derive(Debug, Clone)]
pub struct Handle {
    module: PathBuf,
    /// Base de datos NSS (con `libsoftokn3`).
    nss_dir: Option<PathBuf>,
    /// Etiqueta de la tarjeta (los números de ranura cambian al
    /// reconectarla).
    token: String,
    id: Vec<u8>,
}

/// Un módulo PKCS#11 cargado e inicializado.
struct Module {
    f: *const CK_FUNCTION_LIST,
    _lib: libloading::Library,
}

/// Argumentos de inicio de NSS: los de PKCS#11 más la configuración.
#[repr(C)]
struct NssInitArgs {
    create_mutex: CK_CREATEMUTEX,
    destroy_mutex: CK_DESTROYMUTEX,
    lock_mutex: CK_LOCKMUTEX,
    unlock_mutex: CK_UNLOCKMUTEX,
    flags: CK_FLAGS,
    library_parameters: *const u8,
    reserved: *mut c_void,
}

macro_rules! call {
    ($m:expr, $name:ident ( $($arg:expr),* )) => {{
        match unsafe { (*$m.f).$name } {
            None => Err(Pkcs11Error(CKR_FUNCTION_NOT_SUPPORTED)),
            Some(f) => {
                let rv = unsafe { f($($arg),*) };
                if rv != CKR_OK { Err(Pkcs11Error(rv)) } else { Ok(()) }
            }
        }
    }};
}

#[derive(Debug)]
struct Pkcs11Error(CK_RV);

impl std::fmt::Display for Pkcs11Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            CKR_PIN_INCORRECT => f.write_str(&t!("dsig.pin_wrong")),
            CKR_PIN_LOCKED => f.write_str(&t!("dsig.pin_locked")),
            CKR_TOKEN_NOT_PRESENT | CKR_DEVICE_REMOVED => f.write_str(&t!("dsig.card_removed")),
            CKR_FUNCTION_CANCELED => f.write_str(&t!("print.cancelled")),
            rv => write!(f, "PKCS#11 0x{rv:08X}"),
        }
    }
}

impl std::error::Error for Pkcs11Error {}

impl Module {
    fn load(path: &Path, nss_dir: Option<&Path>) -> Result<Module> {
        let lib = unsafe { libloading::Library::new(path)? };
        let get: libloading::Symbol<unsafe extern "C" fn(*mut *mut CK_FUNCTION_LIST) -> CK_RV> =
            unsafe { lib.get(b"C_GetFunctionList").or_else(|_| lib.get(b"NSC_GetFunctionList"))? };
        let mut f: *mut CK_FUNCTION_LIST = null_mut();
        if unsafe { get(&mut f) } != CKR_OK || f.is_null() {
            bail!("{}: sin lista de funciones", path.display());
        }
        let m = Module { f, _lib: lib };
        let params = nss_dir.map(|d| format!("configdir='sql:{}' certPrefix='' keyPrefix='' secmod='secmod.db' flags='readOnly,noModDB'\0", d.display()));
        let mut args = NssInitArgs {
            create_mutex: None,
            destroy_mutex: None,
            lock_mutex: None,
            unlock_mutex: None,
            flags: CKF_OS_LOCKING_OK,
            library_parameters: params.as_ref().map_or(null(), |p| p.as_ptr()),
            reserved: null_mut(),
        };
        match call!(m, C_Initialize(&mut args as *mut NssInitArgs as *mut c_void)) {
            Ok(()) => {}
            Err(Pkcs11Error(CKR_CRYPTOKI_ALREADY_INITIALIZED)) => {}
            Err(e) => bail!("{}: {e}", path.display()),
        }
        Ok(m)
    }

    fn slots(&self) -> Result<Vec<CK_SLOT_ID>> {
        let mut n: CK_ULONG = 0;
        call!(self, C_GetSlotList(CK_TRUE, null_mut(), &mut n))?;
        let mut v = vec![0; n as usize];
        call!(self, C_GetSlotList(CK_TRUE, v.as_mut_ptr(), &mut n))?;
        v.truncate(n as usize);
        Ok(v)
    }

    /// Etiqueta y si pide PIN.
    fn token(&self, slot: CK_SLOT_ID) -> Result<(String, bool)> {
        let mut info: CK_TOKEN_INFO = unsafe { std::mem::zeroed() };
        call!(self, C_GetTokenInfo(slot, &mut info))?;
        let label = String::from_utf8_lossy(&info.label).trim_end().to_string();
        Ok((label, info.flags & CKF_LOGIN_REQUIRED != 0 && info.flags & CKF_USER_PIN_INITIALIZED != 0))
    }

    fn open(&self, slot: CK_SLOT_ID) -> Result<CK_SESSION_HANDLE> {
        let mut s: CK_SESSION_HANDLE = 0;
        call!(self, C_OpenSession(slot, CKF_SERIAL_SESSION, null_mut(), None, &mut s))?;
        Ok(s)
    }

    fn find(&self, s: CK_SESSION_HANDLE, class: CK_OBJECT_CLASS, id: Option<&[u8]>) -> Result<Vec<CK_OBJECT_HANDLE>> {
        let mut class = class;
        let mut tmpl = vec![CK_ATTRIBUTE { type_: CKA_CLASS, pValue: &mut class as *mut _ as *mut c_void, ulValueLen: size_of::<CK_OBJECT_CLASS>() as CK_ULONG }];
        if let Some(id) = id {
            tmpl.push(CK_ATTRIBUTE { type_: CKA_ID, pValue: id.as_ptr() as *mut c_void, ulValueLen: id.len() as CK_ULONG });
        }
        call!(self, C_FindObjectsInit(s, tmpl.as_mut_ptr(), tmpl.len() as CK_ULONG))?;
        let mut out = Vec::new();
        loop {
            let mut buf = [0 as CK_OBJECT_HANDLE; 32];
            let mut n: CK_ULONG = 0;
            if call!(self, C_FindObjects(s, buf.as_mut_ptr(), buf.len() as CK_ULONG, &mut n)).is_err() || n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
        }
        let _ = call!(self, C_FindObjectsFinal(s));
        Ok(out)
    }

    fn attr(&self, s: CK_SESSION_HANDLE, o: CK_OBJECT_HANDLE, ty: CK_ATTRIBUTE_TYPE) -> Option<Vec<u8>> {
        let mut a = CK_ATTRIBUTE { type_: ty, pValue: null_mut(), ulValueLen: 0 };
        call!(self, C_GetAttributeValue(s, o, &mut a, 1)).ok()?;
        if a.ulValueLen == CK_UNAVAILABLE_INFORMATION {
            return None;
        }
        let mut v = vec![0u8; a.ulValueLen as usize];
        a.pValue = v.as_mut_ptr() as *mut c_void;
        call!(self, C_GetAttributeValue(s, o, &mut a, 1)).ok()?;
        v.truncate(a.ulValueLen as usize);
        Some(v)
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        if let Some(f) = unsafe { (*self.f).C_Finalize } {
            unsafe { f(null_mut()) };
        }
    }
}

/// Las bases de datos NSS: (carpeta, nombre para enseñar).
fn nss_dbs() -> Vec<(PathBuf, String)> {
    if let Ok(list) = std::env::var("FORM_PDF_READER_NSSDB") {
        return list.split(':').filter(|s| !s.is_empty()).map(|s| (PathBuf::from(s), "NSS".to_string())).collect();
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let mut v = Vec::new();
    let chrome = home.join(".pki/nssdb");
    if chrome.join("cert9.db").exists() {
        v.push((chrome, t!("dsig.origin_chrome")));
    }
    for base in [".mozilla/firefox", ".var/app/org.mozilla.firefox/.mozilla/firefox", "snap/firefox/common/.mozilla/firefox"] {
        let Ok(rd) = std::fs::read_dir(home.join(base)) else { continue };
        for e in rd.flatten() {
            if e.path().join("cert9.db").exists() {
                v.push((e.path(), t!("dsig.origin_firefox")));
            }
        }
    }
    v
}

fn find_lib(names: &[&str]) -> Vec<PathBuf> {
    let dirs = ["/usr/lib", "/usr/lib64", "/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib/pkcs11", "/usr/lib64/pkcs11", "/usr/lib/x86_64-linux-gnu/pkcs11", "/usr/local/lib"];
    let mut out = Vec::new();
    for n in names {
        if let Some(p) = dirs.iter().map(|d| Path::new(d).join(n)).find(|p| p.exists()) {
            out.push(p);
        }
    }
    out
}

/// Módulos de tarjetas: OpenSC y el oficial del DNIe.
fn card_modules() -> Vec<PathBuf> {
    let mut v = find_lib(&["opensc-pkcs11.so", "libpkcs11-dnie.so"]);
    if let Ok(extra) = std::env::var("FORM_PDF_READER_PKCS11") {
        v.extend(extra.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    }
    v
}

pub fn list() -> Vec<CertEntry> {
    let mut out = Vec::new();
    if let Some(softokn) = find_lib(&["libsoftokn3.so"]).into_iter().next() {
        for (dir, origin) in nss_dbs() {
            match Module::load(&softokn, Some(&dir)) {
                Ok(m) => collect(&m, &softokn, Some(&dir), &origin, &mut out),
                Err(e) => log::info!("NSS {}: {e:#}", dir.display()),
            }
        }
    }
    for module in card_modules() {
        match Module::load(&module, None) {
            Ok(m) => collect(&m, &module, None, "", &mut out),
            Err(e) => log::info!("PKCS#11 {}: {e:#}", module.display()),
        }
    }
    out
}

fn collect(m: &Module, module: &Path, nss_dir: Option<&Path>, origin: &str, out: &mut Vec<CertEntry>) {
    let Ok(slots) = m.slots() else { return };
    for slot in slots {
        let Ok((token, login)) = m.token(slot) else { continue };
        let Ok(s) = m.open(slot) else { continue };
        let certs: Vec<(Vec<u8>, Vec<u8>)> = m
            .find(s, CKO_CERTIFICATE, None)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|o| Some((m.attr(s, o, CKA_VALUE)?, m.attr(s, o, CKA_ID).unwrap_or_default())))
            .collect();
        // Sin PIN se ven las claves: solo los certificados que tienen la suya.
        let keys: Option<Vec<Vec<u8>>> = (!login).then(|| m.find(s, CKO_PRIVATE_KEY, None).unwrap_or_default().into_iter().filter_map(|o| m.attr(s, o, CKA_ID)).collect());
        let all: Vec<Vec<u8>> = certs.iter().map(|(v, _)| v.clone()).collect();
        for (value, id) in &certs {
            let Some(info) = pades::cert_info(value) else { continue };
            if !info.for_signing || keys.as_ref().is_some_and(|k| !k.contains(id)) {
                continue;
            }
            let shown = if nss_dir.is_some() { origin.to_string() } else { t!("dsig.origin_card", &token) };
            out.push(CertEntry {
                info,
                chain: pades::build_chain(value, &all),
                source: Source::System(Handle { module: module.to_path_buf(), nss_dir: nss_dir.map(Path::to_path_buf), token: token.clone(), id: id.clone() }),
                secret: match (login, nss_dir.is_some()) {
                    (false, _) => Secret::None,
                    (true, true) => Secret::Password,
                    (true, false) => Secret::Pin,
                },
                origin: shown,
                // Las tarjetas (OpenSC, DNIe) están en pruebas; las bases NSS no.
                beta: nss_dir.is_none(),
            });
        }
        let _ = call!(m, C_CloseSession(s));
    }
}

/// Firma con la clave de la tarjeta o de la base NSS.
struct Pkcs11Signer {
    m: Module,
    s: CK_SESSION_HANDLE,
    key: CK_OBJECT_HANDLE,
    kind: KeyKind,
    chain: Vec<Vec<u8>>,
}

pub fn signer(h: &Handle, chain: &[Vec<u8>], pin: &str) -> Result<Box<dyn Signer>> {
    let m = Module::load(&h.module, h.nss_dir.as_deref())?;
    let slot = m.slots()?.into_iter().find(|&sl| m.token(sl).is_ok_and(|(l, _)| l == h.token)).ok_or_else(|| anyhow!("{}", t!("dsig.card_removed")))?;
    let (_, login) = m.token(slot)?;
    let s = m.open(slot)?;
    if login || !pin.is_empty() {
        match call!(m, C_Login(s, CKU_USER, pin.as_ptr() as *mut u8, pin.len() as CK_ULONG)) {
            Ok(()) | Err(Pkcs11Error(CKR_USER_ALREADY_LOGGED_IN)) => {}
            Err(e) => return Err(e.into()),
        }
    }
    let key = *m.find(s, CKO_PRIVATE_KEY, Some(&h.id))?.first().ok_or_else(|| anyhow!("{}", t!("dsig.no_key")))?;
    let kind = pades::key_kind(&chain[0])?;
    Ok(Box::new(Pkcs11Signer { m, s, key, kind, chain: chain.to_vec() }))
}

/// DigestInfo de SHA-256 (para firmar con RSA el resumen ya hecho).
const SHA256_DIGEST_INFO: [u8; 19] = [0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20];

impl Signer for Pkcs11Signer {
    fn chain(&self) -> Vec<Vec<u8>> {
        self.chain.clone()
    }

    fn sign(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(data);
        let (mech, mut input) = match self.kind {
            KeyKind::Rsa => (CKM_RSA_PKCS, [&SHA256_DIGEST_INFO[..], &digest[..]].concat()),
            KeyKind::Ec => (CKM_ECDSA, digest.to_vec()),
        };
        let mut mechanism = CK_MECHANISM { mechanism: mech, pParameter: null_mut(), ulParameterLen: 0 };
        call!(self.m, C_SignInit(self.s, &mut mechanism, self.key))?;
        let mut len: CK_ULONG = 0;
        call!(self.m, C_Sign(self.s, input.as_mut_ptr(), input.len() as CK_ULONG, null_mut(), &mut len))?;
        let mut sig = vec![0u8; len as usize];
        call!(self.m, C_Sign(self.s, input.as_mut_ptr(), input.len() as CK_ULONG, sig.as_mut_ptr(), &mut len))?;
        sig.truncate(len as usize);
        Ok(sig)
    }
}

impl Drop for Pkcs11Signer {
    fn drop(&mut self) {
        let _ = call!(self.m, C_Logout(self.s));
        let _ = call!(self.m, C_CloseSession(self.s));
    }
}
