// Enlaza con libpdfium.so (variante V8 + XFA) de vendor/pdfium, en la raíz del
// repositorio, que descarga scripts/descargar-pdfium.sh. PDFIUM_LIB_DIR permite
// usar otra ubicación.
//
// El rpath de aquí solo afecta a las pruebas y ejemplos de este crate; cada
// ejecutable que use la librería pone el suyo (ver crates/desktop/build.rs).
fn main() {
    let lib_dir = pdfium_lib_dir();
    if !std::path::Path::new(&lib_dir).join("libpdfium.so").exists() {
        panic!("libpdfium.so not found in {lib_dir}. Run scripts/descargar-pdfium.sh first (or set PDFIUM_LIB_DIR).");
    }
    println!("cargo:rerun-if-env-changed=PDFIUM_LIB_DIR");
    println!("cargo:rerun-if-changed={lib_dir}/libpdfium.so");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-search=native={lib_dir}");
    println!("cargo:rustc-link-lib=dylib=pdfium");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
}

fn pdfium_lib_dir() -> String {
    std::env::var("PDFIUM_LIB_DIR").unwrap_or_else(|_| {
        let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let root = manifest.join("../..");
        let root = root.canonicalize().unwrap_or(root);
        root.join("vendor/pdfium/lib").display().to_string()
    })
}
