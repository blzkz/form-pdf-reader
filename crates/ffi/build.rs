// En el equipo (pruebas y generación de bindings) la librería necesita
// encontrar libpdfium.so; en Android la carga la propia app.
fn main() {
    let lib_dir = std::env::var("PDFIUM_LIB_DIR").unwrap_or_else(|_| {
        let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let root = manifest.join("../..");
        let root = root.canonicalize().unwrap_or(root);
        root.join("vendor/pdfium/lib").display().to_string()
    });
    println!("cargo:rerun-if-env-changed=PDFIUM_LIB_DIR");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
    }
}
