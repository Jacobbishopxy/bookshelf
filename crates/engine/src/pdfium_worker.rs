use std::path::PathBuf;

use anyhow::Context as _;
use bookshelf_core::{Book, PreviewMode, encode_path};

use crate::Engine;

pub fn run_pdfium_worker_from_env() -> anyhow::Result<()> {
    unsafe {
        std::env::set_var("BOOKSHELF_PDFIUM_WORKER", "1");
        std::env::set_var("BOOKSHELF_PDFIUM_ISOLATE", "0");
    }

    let mut pdf_path: Option<PathBuf> = None;
    let mut page_index: Option<u32> = None;
    let mut mode: Option<PreviewMode> = None;
    let mut width_chars: Option<u16> = None;
    let mut max_height_chars: Option<u16> = None;

    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let arg_str = arg.to_string_lossy();
        match arg_str.as_ref() {
            "--pdfium-worker" => {}
            "--pdf" => {
                let value = args.next().context("missing value for --pdf")?;
                pdf_path = Some(PathBuf::from(value));
            }
            "--page-index" => {
                let value = args.next().context("missing value for --page-index")?;
                let value_str = value.to_string_lossy();
                let parsed = value_str
                    .parse::<u32>()
                    .with_context(|| format!("invalid --page-index value: {value_str}"))?;
                page_index = Some(parsed);
            }
            "--mode" => {
                let value = args.next().context("missing value for --mode")?;
                let value_str = value.to_string_lossy();
                let parsed = match value_str.trim().to_ascii_lowercase().as_str() {
                    "braille" => PreviewMode::Braille,
                    "blocks" => PreviewMode::Blocks,
                    other => anyhow::bail!("invalid --mode value: {other}"),
                };
                mode = Some(parsed);
            }
            "--width-chars" => {
                let value = args.next().context("missing value for --width-chars")?;
                let value_str = value.to_string_lossy();
                width_chars = Some(
                    value_str
                        .parse::<u16>()
                        .with_context(|| format!("invalid --width-chars value: {value_str}"))?,
                );
            }
            "--max-height-chars" => {
                let value = args
                    .next()
                    .context("missing value for --max-height-chars")?;
                let value_str = value.to_string_lossy();
                max_height_chars =
                    Some(value_str.parse::<u16>().with_context(|| {
                        format!("invalid --max-height-chars value: {value_str}")
                    })?);
            }
            other => anyhow::bail!("unknown arg: {other}"),
        }
    }

    let pdf_path = pdf_path.context("missing --pdf")?;
    let page_index = page_index.context("missing --page-index")?;
    let mode = mode.context("missing --mode")?;
    let width_chars = width_chars.context("missing --width-chars")?;
    let max_height_chars = max_height_chars.context("missing --max-height-chars")?;

    let title = pdf_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let book = Book {
        path: encode_path(&pdf_path),
        title,
        last_opened: None,
    };

    let engine = Engine::new();
    let text = engine.render_page_raster_in_process(
        &book,
        page_index,
        mode,
        width_chars,
        max_height_chars,
    )?;
    print!("{text}");
    Ok(())
}
