use std::path::PathBuf;

use anyhow::Context as _;
use pdfium_render::prelude::Pdfium;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let mut lib_path: Option<PathBuf> = None;
    let mut dir_path: Option<PathBuf> = None;
    let mut pdf_path: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--lib" => {
                let value = args.next().context("missing value for --lib")?;
                lib_path = Some(PathBuf::from(value));
            }
            "--dir" => {
                let value = args.next().context("missing value for --dir")?;
                dir_path = Some(PathBuf::from(value));
            }
            "--pdf" => {
                let value = args.next().context("missing value for --pdf")?;
                pdf_path = Some(PathBuf::from(value));
            }
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            other => anyhow::bail!("unknown arg: {other} (try --help)"),
        }
    }

    if lib_path.is_some() && dir_path.is_some() {
        anyhow::bail!("pass only one of --lib or --dir");
    }

    let resolved_lib_path = if let Some(dir) = dir_path.as_ref() {
        Pdfium::pdfium_platform_library_name_at_path(dir)
    } else if let Some(path) = lib_path.as_ref() {
        path.clone()
    } else {
        anyhow::bail!("must pass --lib <path> or --dir <dir> (try --help)");
    };

    println!("pdfium: trying {}", resolved_lib_path.display());
    let bindings = Pdfium::bind_to_library(&resolved_lib_path)
        .map_err(|err| anyhow::anyhow!(err))
        .with_context(|| format!("bind_to_library {}", resolved_lib_path.display()))?;
    let pdfium = Pdfium::new(bindings);
    println!("pdfium: ok");

    if let Some(pdf) = pdf_path.as_ref() {
        let doc = pdfium
            .load_pdf_from_file(pdf, None)
            .map_err(|err| anyhow::anyhow!(err))
            .with_context(|| format!("load_pdf_from_file {}", pdf.display()))?;
        println!("pdf: ok (pages={})", doc.pages().len());
    }

    unsafe {
        std::env::set_var("BOOKSHELF_PDFIUM_LIB_PATH", &resolved_lib_path);
    }
    let engine = engine::Engine::new();
    engine
        .check_pdfium()
        .context("engine::Engine::check_pdfium failed")?;
    println!("engine: ok");

    Ok(())
}

fn print_help() {
    println!(
        "\
pdfium_probe

Usage:
  cargo run -p engine --bin pdfium_probe -- --lib <path-to-libpdfium>
  cargo run -p engine --bin pdfium_probe -- --dir <dir-containing-libpdfium>

Options:
  --lib <path>   Path to shared library (e.g. libpdfium.so/libpdfium.dylib/pdfium.dll)
  --dir <dir>    Directory containing the platform-specific library name
  --pdf <path>   Optional PDF to open as a smoke test
  --help         Show this help
"
    );
}
