// Linux: rpath del ejecutable para encontrar libpdfium.so:
//   - junto al ejecutable ($ORIGIN) y en $ORIGIN/lib  -> tarball
//   - en /usr/lib/form-pdf-reader                     -> paquetes .deb/.rpm/Arch
//   - en vendor/pdfium/lib del repositorio            -> desarrollo (cargo run)
//     (no con FORM_PDF_READER_PACKAGE=1: los paquetes no deben llevar la
//     ruta de la máquina donde se compilaron)
// Windows: pdfium.dll va junto al ejecutable; aquí se le pone el icono y los
// datos de versión.
// El enlace con la librería lo declara form-pdf-reader-core.
fn main() {
    println!("cargo:rerun-if-env-changed=PDFIUM_LIB_DIR");
    println!("cargo:rerun-if-env-changed=FORM_PDF_READER_PACKAGE");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
        return;
    }
    let lib_dir = std::env::var("PDFIUM_LIB_DIR").unwrap_or_else(|_| {
        let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let root = manifest.join("../..");
        let root = root.canonicalize().unwrap_or(root);
        root.join("vendor/pdfium/lib").display().to_string()
    });
    let package = std::env::var("FORM_PDF_READER_PACKAGE").is_ok_and(|v| v == "1");
    let dev = if package { String::new() } else { format!(":{lib_dir}") };
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN:$ORIGIN/lib:/usr/lib/form-pdf-reader{dev}");
}

#[cfg(windows)]
fn windows_resources() {
    let icon = "../../packaging/windows/form-pdf-reader.ico";
    println!("cargo:rerun-if-changed={icon}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon)
        .set("FileDescription", "Form PDF Reader")
        .set("ProductName", "Form PDF Reader")
        .set("CompanyName", "blzkz")
        .set("LegalCopyright", "© 2026 blzkz. MIT License.");
    res.compile().expect("Windows resources");
}

#[cfg(not(windows))]
fn windows_resources() {}
