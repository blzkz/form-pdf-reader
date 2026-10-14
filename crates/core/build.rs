// Enlaza con PDFium (variante V8 + XFA) de vendor/pdfium, en la raíz del
// repositorio, que descarga scripts/descargar-pdfium.sh. PDFIUM_LIB_DIR permite
// usar otra ubicación.
//
// - Linux: libpdfium.so. El rpath de aquí solo afecta a las pruebas y
//   ejemplos de este crate; cada ejecutable que use la librería pone el suyo
//   (ver crates/desktop/build.rs).
// - Windows: pdfium.dll.lib (la DLL va junto al ejecutable; para las pruebas,
//   vendor/pdfium/bin debe estar en el PATH).
fn main() {
    let lib_dir = pdfium_lib_dir();
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let file = if windows { "pdfium.dll.lib" } else { "libpdfium.so" };
    if !std::path::Path::new(&lib_dir).join(file).exists() {
        panic!("{file} not found in {lib_dir}. Run scripts/descargar-pdfium.sh first (or set PDFIUM_LIB_DIR).");
    }
    println!("cargo:rerun-if-env-changed=PDFIUM_LIB_DIR");
    println!("cargo:rerun-if-changed={lib_dir}/{file}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-search=native={lib_dir}");
    if windows {
        println!("cargo:rustc-link-lib=dylib=pdfium.dll");
    } else {
        println!("cargo:rustc-link-lib=dylib=pdfium");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
    }
}

fn pdfium_lib_dir() -> String {
    std::env::var("PDFIUM_LIB_DIR").unwrap_or_else(|_| {
        let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let root = manifest.join("../..");
        let root = root.canonicalize().unwrap_or(root);
        root.join("vendor/pdfium/lib").display().to_string()
    })
}
