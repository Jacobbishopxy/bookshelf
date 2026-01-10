use std::path::PathBuf;

use anyhow::Context as _;
use pdfium_render::prelude::{PdfBitmapFormat, PdfRenderConfig, Pdfium};

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
    let mut page_number: u32 = 1;

    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let arg_str = arg.to_string_lossy();
        match arg_str.as_ref() {
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
            "--page" => {
                let value = args.next().context("missing value for --page")?;
                let value_str = value.to_string_lossy();
                page_number = value_str
                    .parse::<u32>()
                    .with_context(|| format!("invalid --page value: {value_str}"))?;
                if page_number == 0 {
                    anyhow::bail!("--page must be >= 1");
                }
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

        // Exercise the rendering path; this is where a bad/broken Pdfium binary may crash.
        let page = doc
            .pages()
            .get(u16::try_from(page_number.saturating_sub(1)).context("page out of range")?)
            .map_err(|err| anyhow::anyhow!(err))
            .with_context(|| format!("get page {page_number}"))?;
        let render_config = PdfRenderConfig::new()
            .set_target_width(600)
            .set_maximum_width(600)
            .set_maximum_height(800)
            .render_form_data(false)
            .render_annotations(false)
            .use_grayscale_rendering(true)
            .set_reverse_byte_order(false)
            .set_format(PdfBitmapFormat::Gray);
        let bitmap = page
            .render_with_config(&render_config)
            .map_err(|err| anyhow::anyhow!(err))
            .with_context(|| format!("render page {page_number}"))?;
        println!(
            "render: ok (page={} {}x{})",
            page_number,
            bitmap.width(),
            bitmap.height()
        );
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
  --page <n>     Page number to render (1-based, default: 1)
  --help         Show this help
"
    );
}
