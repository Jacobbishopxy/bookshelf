use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR missing"));
    let workspace_root = manifest_dir.join("..").join("..");

    let pdfium_dir = workspace_root.join(".pdfium");
    let extracted_dir = pdfium_dir.join("extract").join("lib");
    let extracted_bin_dir = pdfium_dir.join("extract").join("bin");

    let lib_name = pdfium_library_filename();
    let candidates = [
        pdfium_dir.join(lib_name),
        extracted_dir.join(lib_name),
        extracted_bin_dir.join(lib_name),
    ];

    let lib = candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "\n\
Pdfium shared library not found.\n\
\n\
Expected to find `{}` in one of:\n\
- {}\n\
- {}\n\
\n\
Fix:\n\
  bash scripts/pdfium/fetch_and_probe.sh\n\
",
                lib_name,
                pdfium_dir.display(),
                extracted_dir.display(),
            );
        });

    println!(
        "cargo:rustc-env=BOOKSHELF_PDFIUM_LIB_PATH={}",
        lib.display()
    );
    println!("cargo:rerun-if-changed={}", lib.display());
}

fn pdfium_library_filename() -> &'static str {
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => "libpdfium.so",
        Ok("macos") => "libpdfium.dylib",
        Ok("windows") => "pdfium.dll",
        other => panic!("unsupported target os for pdfium: {other:?}"),
    }
}
