// rpath del ejecutable para encontrar libpdfium.so:
//   - junto al ejecutable ($ORIGIN) y en $ORIGIN/lib  -> tarball
//   - en /usr/lib/form-pdf-reader                     -> paquetes .deb/.rpm/Arch
//   - en vendor/pdfium/lib del repositorio            -> desarrollo (cargo run)
// El enlace con la librería lo declara form-pdf-reader-core.
fn main() {
    let lib_dir = std::env::var("PDFIUM_LIB_DIR").unwrap_or_else(|_| {
        let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let root = manifest.join("../..");
        let root = root.canonicalize().unwrap_or(root);
        root.join("vendor/pdfium/lib").display().to_string()
    });
    println!("cargo:rerun-if-env-changed=PDFIUM_LIB_DIR");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN:$ORIGIN/lib:/usr/lib/form-pdf-reader:{lib_dir}");
}
