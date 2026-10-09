// Enlaza con libpdfium.so (variante V8 + XFA) de vendor/pdfium, que descarga
// scripts/descargar-pdfium.sh.
// El rpath permite encontrar la librería:
//   - junto al ejecutable ($ORIGIN) y en $ORIGIN/lib  -> paquetes / tarball
//   - en /usr/lib/form-pdf-reader                    -> PKGBUILD
//   - en vendor/pdfium/lib del proyecto                -> desarrollo (cargo run)
fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let lib_dir = std::env::var("PDFIUM_LIB_DIR")
        .unwrap_or_else(|_| format!("{manifest}/vendor/pdfium/lib"));
    if !std::path::Path::new(&lib_dir).join("libpdfium.so").exists() {
        panic!("libpdfium.so not found in {lib_dir}. Run scripts/descargar-pdfium.sh first (or set PDFIUM_LIB_DIR).");
    }
    println!("cargo:rerun-if-env-changed=PDFIUM_LIB_DIR");
    println!("cargo:rerun-if-changed={lib_dir}/libpdfium.so");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-search=native={lib_dir}");
    println!("cargo:rustc-link-lib=dylib=pdfium");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN:$ORIGIN/lib:/usr/lib/form-pdf-reader:{lib_dir}");
}
