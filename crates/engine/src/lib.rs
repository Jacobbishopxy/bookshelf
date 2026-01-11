//! PDF engine wrapper.

use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use bookshelf_core::{Book, ReaderMode, TocItem};
use pdf::content::{Op, TextDrawAdjusted};
use pdf::file::FileOptions;
use pdf::font::ToUnicodeMap;
use pdf::object::{
    Action, Dest, MaybeNamedDest, OutlineItem, PageTree, PagesNode, PlainRef, RcRef, Resolve,
    Resources,
};
use pdf::primitive::{Name, PdfString, Primitive};
use pdfium_render::prelude::{PdfBitmapFormat, PdfRenderConfig, Pdfium};

#[derive(Debug, Default)]
pub struct Engine {
    pdfium: RefCell<PdfiumState>,
}

#[derive(Debug, Default)]
enum PdfiumState {
    #[default]
    Uninitialized,
    Available(Pdfium),
    Unavailable(String),
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check_pdfium(&self) -> anyhow::Result<()> {
        let _ = self.pdfium()?;
        Ok(())
    }

    pub fn page_count(&self, book: &Book) -> anyhow::Result<u32> {
        let path = bookshelf_core::decode_path(&book.path);
        let file = FileOptions::cached().open(path)?;
        Ok(file.num_pages())
    }

    pub fn toc(&self, book: &Book) -> anyhow::Result<Vec<TocItem>> {
        let path = bookshelf_core::decode_path(&book.path);
        let file = FileOptions::cached().open(&path)?;
        let resolver = file.resolver();
        let catalog = file.get_root();

        let mut dest_pages_by_name: HashMap<String, PlainRef> = HashMap::new();
        if let Some(ref names) = catalog.names
            && let Some(ref dests) = names.dests
        {
            dests.walk(&resolver, &mut |key: &PdfString, val: &Option<Dest>| {
                if let Some(Dest {
                    page: Some(page), ..
                }) = val
                {
                    dest_pages_by_name.insert(key.to_string_lossy(), page.get_inner());
                }
            })?;
        }

        let mut pages_by_ref: HashMap<PlainRef, usize> = HashMap::new();
        fn add_tree(
            r: &impl Resolve,
            pages: &mut HashMap<PlainRef, usize>,
            tree: &PageTree,
            current_page: &mut usize,
        ) {
            for &node_ref in &tree.kids {
                let node = match r.get(node_ref) {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                match *node {
                    PagesNode::Tree(ref tree) => add_tree(r, pages, tree, current_page),
                    PagesNode::Leaf(ref _page) => {
                        pages.insert(node_ref.get_inner(), *current_page);
                        *current_page += 1;
                    }
                }
            }
        }
        add_tree(&resolver, &mut pages_by_ref, &catalog.pages, &mut 0);

        fn page_index_to_page_number(page_index: usize) -> Option<u32> {
            u32::try_from(page_index).ok().map(|n| n.saturating_add(1))
        }

        let page_for_ref = |r: PlainRef| -> Option<u32> {
            pages_by_ref
                .get(&r)
                .copied()
                .and_then(page_index_to_page_number)
        };

        let page_for_name = |name: &str| -> Option<u32> {
            let page_ref = dest_pages_by_name.get(name).copied()?;
            page_for_ref(page_ref)
        };

        fn walk_outline(
            r: &impl Resolve,
            mut node: RcRef<OutlineItem>,
            depth: usize,
            page_for_name: &impl Fn(&str) -> Option<u32>,
            page_for_ref: &impl Fn(PlainRef) -> Option<u32>,
            out: &mut Vec<TocItem>,
        ) {
            loop {
                let title = node
                    .title
                    .as_ref()
                    .map(|t| t.to_string_lossy())
                    .unwrap_or_else(|| "(untitled)".to_string());

                let mut page: Option<u32> = None;
                if let Some(ref dest) = node.dest {
                    match dest {
                        Primitive::String(s) => {
                            page = page_for_name(&s.to_string_lossy());
                        }
                        Primitive::Array(a) => {
                            if let Some(Primitive::Reference(r)) = a.first() {
                                page = page_for_ref(*r);
                            }
                        }
                        _ => {}
                    }
                }

                if page.is_none()
                    && let Some(Action::Goto(dest)) = node.action.clone()
                {
                    match dest {
                        MaybeNamedDest::Named(s) => {
                            page = page_for_name(&s.to_string_lossy());
                        }
                        MaybeNamedDest::Direct(Dest { page: Some(p), .. }) => {
                            page = page_for_ref(p.get_inner());
                        }
                        _ => {}
                    }
                }

                out.push(TocItem { title, page, depth });

                if let Some(entry_ref) = node.first
                    && let Ok(entry) = r.get(entry_ref)
                {
                    walk_outline(r, entry, depth + 1, page_for_name, page_for_ref, out);
                }

                if let Some(entry_ref) = node.next
                    && let Ok(entry) = r.get(entry_ref)
                {
                    node = entry;
                    continue;
                }

                break;
            }
        }

        let mut out = Vec::new();
        if let Some(ref outlines) = catalog.outlines
            && let Some(entry_ref) = outlines.first
        {
            let entry = resolver.get(entry_ref)?;
            walk_outline(&resolver, entry, 0, &page_for_name, &page_for_ref, &mut out);
        }
        Ok(out)
    }

    pub fn render_page_text(&self, book: &Book, page_index: u32) -> anyhow::Result<String> {
        let path = bookshelf_core::decode_path(&book.path);
        let file = FileOptions::cached().open(path)?;
        let resolver = file.resolver();
        let page = file.get_page(page_index)?;
        let resources = page.resources()?;
        let Some(content) = &page.contents else {
            return Ok("no text found".to_string());
        };
        let ops = content.operations(&resolver)?;
        let text = ops_to_text(&ops, &resolver, resources);
        let text = text.trim().to_string();
        if text.is_empty() {
            Ok("no text found".to_string())
        } else {
            Ok(text)
        }
    }

    pub fn debug_page_text(&self, book: &Book, page_index: u32) -> anyhow::Result<String> {
        let path = bookshelf_core::decode_path(&book.path);
        let file = FileOptions::cached().open(&path)?;
        let resolver = file.resolver();
        let page = file.get_page(page_index)?;
        let resources = page.resources()?;
        let display_path = path.display();

        let Some(content) = &page.contents else {
            return Ok(format!(
                "book: {}\npage: {}\n(no page contents)\n",
                display_path,
                page_index + 1
            ));
        };

        let ops = content.operations(&resolver)?;

        let mut out = String::new();
        writeln!(&mut out, "book: {}", display_path)?;
        writeln!(&mut out, "page: {}", page_index + 1)?;
        writeln!(&mut out, "ops: {}", ops.len())?;
        writeln!(&mut out)?;

        let mut current_font: Option<Name> = None;
        let mut tounicode_cache: HashMap<Name, Option<ToUnicodeMap>> = HashMap::new();
        let mut fonts_used: std::collections::BTreeSet<Name> = std::collections::BTreeSet::new();

        let mut text_ops = 0usize;
        let max_text_ops = 300usize;

        for (idx, op) in ops.iter().enumerate() {
            match op {
                Op::TextFont { name, size } => {
                    current_font = Some(name.clone());
                    fonts_used.insert(name.clone());
                    writeln!(
                        &mut out,
                        "{idx:04} TextFont {} size={}",
                        name.as_str(),
                        size
                    )?;
                }
                Op::TextDraw { text } => {
                    text_ops += 1;
                    if text_ops > max_text_ops {
                        writeln!(
                            &mut out,
                            "\n(truncated after {max_text_ops} text ops; remaining omitted)"
                        )?;
                        break;
                    }
                    dump_text_op(
                        &mut out,
                        idx,
                        current_font.as_ref(),
                        text,
                        &resolver,
                        resources,
                        &mut tounicode_cache,
                    )?;
                }
                Op::TextDrawAdjusted { array } => {
                    for item in array {
                        if let TextDrawAdjusted::Text(text) = item {
                            text_ops += 1;
                            if text_ops > max_text_ops {
                                writeln!(
                                    &mut out,
                                    "\n(truncated after {max_text_ops} text ops; remaining omitted)"
                                )?;
                                break;
                            }
                            dump_text_op(
                                &mut out,
                                idx,
                                current_font.as_ref(),
                                text,
                                &resolver,
                                resources,
                                &mut tounicode_cache,
                            )?;
                        }
                    }
                }
                _ => {}
            }
        }

        writeln!(&mut out)?;
        writeln!(&mut out, "fonts used: {}", fonts_used.len())?;
        for font_name in fonts_used {
            let map = tounicode_for_font(&font_name, &resolver, resources, &mut tounicode_cache);
            let status = match map {
                Some(map) => format!("ToUnicode entries={}", map.len()),
                None => "ToUnicode (none)".to_string(),
            };
            writeln!(&mut out, "- {}: {}", font_name.as_str(), status)?;
        }

        writeln!(&mut out)?;
        let page_text = self.render_page_text(book, page_index)?;
        writeln!(&mut out, "=== rendered page text (sanitized) ===")?;
        writeln!(&mut out, "{page_text}")?;

        Ok(out)
    }

    pub fn render_page_for_reader(
        &self,
        book: &Book,
        page_index: u32,
        mode: ReaderMode,
        _viewport_width_chars: u16,
        _viewport_height_chars: u16,
    ) -> anyhow::Result<String> {
        match mode {
            ReaderMode::Text => self.render_page_text(book, page_index),
            ReaderMode::Image => {
                anyhow::bail!("image mode is rendered in the UI (ratatui-image), not as text")
            }
        }
    }

    pub fn render_page_bitmap_rgba(
        &self,
        book: &Book,
        page_index: u32,
        target_width: i32,
        max_height: i32,
    ) -> anyhow::Result<RgbaBitmap> {
        if self.pdfium_disabled() {
            anyhow::bail!("pdfium disabled via BOOKSHELF_DISABLE_PDFIUM");
        }

        let pdfium = self.pdfium()?;
        let path = bookshelf_core::decode_path(&book.path);
        let document = pdfium
            .load_pdf_from_file(&path, None)
            .map_err(|err| anyhow::anyhow!(err))?;

        let page_index =
            u16::try_from(page_index).map_err(|_| anyhow::anyhow!("page index out of range"))?;
        let page = document
            .pages()
            .get(page_index)
            .map_err(|err| anyhow::anyhow!(err))?;

        let render_config = PdfRenderConfig::new()
            .set_target_width(target_width.max(1))
            .set_maximum_width(target_width.max(1))
            .set_maximum_height(max_height.max(1))
            .render_form_data(false)
            .render_annotations(false)
            .use_grayscale_rendering(false)
            .set_reverse_byte_order(false)
            .set_format(PdfBitmapFormat::BGRA);

        let bitmap = page
            .render_with_config(&render_config)
            .map_err(|err| anyhow::anyhow!(err))?;

        let width = bitmap.width().max(0) as usize;
        let height = bitmap.height().max(0) as usize;
        let src_pixels = bitmap.as_raw_bytes();

        let src_stride = if height == 0 {
            0
        } else {
            src_pixels.len() / height
        };

        let mut pixels = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
        for y in 0..height {
            let base = y.saturating_mul(src_stride);
            for x in 0..width {
                let idx = base.saturating_add(x.saturating_mul(4));
                let b = src_pixels.get(idx).copied().unwrap_or(255);
                let g = src_pixels.get(idx + 1).copied().unwrap_or(255);
                let r = src_pixels.get(idx + 2).copied().unwrap_or(255);
                let a = src_pixels.get(idx + 3).copied().unwrap_or(255);
                pixels.extend_from_slice(&[r, g, b, a]);
            }
        }

        Ok(RgbaBitmap {
            width,
            height,
            stride: width.saturating_mul(4),
            pixels,
        })
    }

    pub fn page_size_points(&self, book: &Book, page_index: u32) -> anyhow::Result<(f32, f32)> {
        if self.pdfium_disabled() {
            anyhow::bail!("pdfium disabled via BOOKSHELF_DISABLE_PDFIUM");
        }

        let pdfium = self.pdfium()?;
        let path = bookshelf_core::decode_path(&book.path);
        let document = pdfium
            .load_pdf_from_file(&path, None)
            .map_err(|err| anyhow::anyhow!(err))?;

        let page_index =
            u16::try_from(page_index).map_err(|_| anyhow::anyhow!("page index out of range"))?;
        let page = document
            .pages()
            .get(page_index)
            .map_err(|err| anyhow::anyhow!(err))?;

        Ok((page.width().value, page.height().value))
    }

    fn pdfium(&self) -> anyhow::Result<Ref<'_, Pdfium>> {
        let init_error = {
            let mut state = self.pdfium.borrow_mut();
            match &*state {
                PdfiumState::Available(_) => None,
                PdfiumState::Unavailable(err) => Some(err.clone()),
                PdfiumState::Uninitialized => match bind_pdfium() {
                    Ok(pdfium) => {
                        *state = PdfiumState::Available(pdfium);
                        None
                    }
                    Err(err) => {
                        let msg = err.to_string();
                        *state = PdfiumState::Unavailable(msg.clone());
                        Some(msg)
                    }
                },
            }
        };

        if let Some(err) = init_error {
            return Err(anyhow::anyhow!(err));
        }

        let state = self.pdfium.borrow();
        match &*state {
            PdfiumState::Available(_) => Ok(Ref::map(state, |state| match state {
                PdfiumState::Available(pdfium) => pdfium,
                _ => unreachable!("pdfium state checked above"),
            })),
            PdfiumState::Unavailable(err) => Err(anyhow::anyhow!(err.clone())),
            PdfiumState::Uninitialized => unreachable!("pdfium state initialized above"),
        }
    }

    fn pdfium_disabled(&self) -> bool {
        std::env::var("BOOKSHELF_DISABLE_PDFIUM")
            .map(|v| !v.trim().is_empty() && v.trim() != "0")
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone)]
pub struct RgbaBitmap {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub pixels: Vec<u8>,
}

fn bind_pdfium() -> anyhow::Result<Pdfium> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(path) = std::env::var("BOOKSHELF_PDFIUM_LIB_PATH") {
        let path = PathBuf::from(path);
        let bindings = Pdfium::bind_to_library(&path)
            .map_err(|err| anyhow::anyhow!(err))
            .map_err(|err| {
                anyhow::anyhow!(
                    "{err}\n\nFailed to load Pdfium from BOOKSHELF_PDFIUM_LIB_PATH={}.",
                    path.display()
                )
            })?;
        return Ok(Pdfium::new(bindings));
    }

    if let Some(path) = option_env!("BOOKSHELF_PDFIUM_LIB_PATH") {
        let path = PathBuf::from(path);
        if path.is_file()
            && let Ok(bindings) = Pdfium::bind_to_library(&path)
        {
            return Ok(Pdfium::new(bindings));
        }
    }

    if let Ok(dir) = std::env::var("BOOKSHELF_PDFIUM_DIR") {
        candidates.push(Pdfium::pdfium_platform_library_name_at_path(Path::new(
            &dir,
        )));
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(Pdfium::pdfium_platform_library_name_at_path(dir));
    }

    candidates.push(Pdfium::pdfium_platform_library_name_at_path(Path::new(
        ".pdfium",
    )));
    candidates.push(Pdfium::pdfium_platform_library_name_at_path(Path::new(".")));

    for path in candidates {
        if let Ok(bindings) = Pdfium::bind_to_library(&path) {
            return Ok(Pdfium::new(bindings));
        }
    }

    let bindings = Pdfium::bind_to_system_library()
        .map_err(|err| anyhow::anyhow!(err))
        .map_err(|err| {
            let lib_name = Pdfium::pdfium_platform_library_name();
            anyhow::anyhow!(
                "{err}\n\nPdfium library not found.\n- Install it system-wide, or\n- Place {} next to the executable.\n",
                lib_name.to_string_lossy()
            )
        })?;

    Ok(Pdfium::new(bindings))
}

fn dump_text_op(
    out: &mut String,
    idx: usize,
    font: Option<&Name>,
    text: &PdfString,
    resolver: &impl Resolve,
    resources: &Resources,
    tounicode_cache: &mut HashMap<Name, Option<ToUnicodeMap>>,
) -> anyhow::Result<()> {
    let bytes = text.as_bytes();
    let mut bytes_hex = String::new();
    let max_bytes = 64usize;
    for (i, b) in bytes.iter().take(max_bytes).enumerate() {
        if i > 0 {
            bytes_hex.push(' ');
        }
        write!(&mut bytes_hex, "{:02X}", b)?;
    }
    if bytes.len() > max_bytes {
        write!(&mut bytes_hex, " …(+{} bytes)", bytes.len() - max_bytes)?;
    }

    let lossy = text.to_string_lossy();
    let decoded = decode_pdf_string(text, font, resolver, resources, tounicode_cache);
    let sanitized = sanitize_extracted_text(&decoded);

    writeln!(
        out,
        "{idx:04} TextDraw font={} bytes={} hex={}",
        font.map(|f| f.as_str()).unwrap_or("(none)"),
        bytes.len(),
        bytes_hex
    )?;
    writeln!(out, "      lossy={:?}", lossy)?;
    writeln!(out, "      decoded={:?}", decoded)?;
    writeln!(out, "      sanitized={:?}", sanitized)?;
    Ok(())
}

fn ops_to_text(ops: &[Op], resolver: &impl Resolve, resources: &Resources) -> String {
    let mut tounicode_cache: HashMap<Name, Option<ToUnicodeMap>> = HashMap::new();
    let mut current_font: Option<Name> = None;

    let mut out = String::new();
    let mut needs_space = false;

    for op in ops {
        match op {
            Op::TextFont { name, .. } => {
                current_font = Some(name.clone());
            }
            Op::TextDraw { text } => {
                let s = decode_pdf_string(
                    text,
                    current_font.as_ref(),
                    resolver,
                    resources,
                    &mut tounicode_cache,
                );
                append_text(&mut out, &s, &mut needs_space);
            }
            Op::TextDrawAdjusted { array } => {
                for item in array {
                    if let TextDrawAdjusted::Text(text) = item {
                        let s = decode_pdf_string(
                            text,
                            current_font.as_ref(),
                            resolver,
                            resources,
                            &mut tounicode_cache,
                        );
                        append_text(&mut out, &s, &mut needs_space);
                    }
                }
            }
            Op::TextNewline => {
                out.push('\n');
                needs_space = false;
            }
            Op::MoveTextPosition { translation } => {
                if translation.y < 0.0 {
                    out.push('\n');
                    needs_space = false;
                }
            }
            _ => {}
        }
    }

    out
}

fn append_text(out: &mut String, s: &str, needs_space: &mut bool) {
    let sanitized = sanitize_extracted_text(s);
    let trimmed = sanitized.trim_matches('\0');
    if trimmed.is_empty() {
        return;
    }

    if *needs_space && !out.ends_with([' ', '\n', '\t']) {
        out.push(' ');
    }
    out.push_str(trimmed);
    *needs_space = true;
}

fn decode_pdf_string(
    text: &PdfString,
    font_name: Option<&Name>,
    resolver: &impl Resolve,
    resources: &Resources,
    tounicode_cache: &mut HashMap<Name, Option<ToUnicodeMap>>,
) -> String {
    let Some(font_name) = font_name else {
        return text.to_string_lossy();
    };

    let map = tounicode_for_font(font_name, resolver, resources, tounicode_cache);
    let Some(map) = map else {
        return text.to_string_lossy();
    };

    decode_with_tounicode(text.as_bytes(), map).unwrap_or_else(|| text.to_string_lossy())
}

fn tounicode_for_font<'a>(
    font_name: &Name,
    resolver: &impl Resolve,
    resources: &Resources,
    cache: &'a mut HashMap<Name, Option<ToUnicodeMap>>,
) -> Option<&'a ToUnicodeMap> {
    if !cache.contains_key(font_name) {
        let map = resources
            .fonts
            .get(font_name)
            .and_then(|lazy| lazy.load(resolver).ok())
            .and_then(|font| font.to_unicode(resolver))
            .and_then(|res| res.ok());
        cache.insert(font_name.clone(), map);
    }
    cache.get(font_name).and_then(|opt| opt.as_ref())
}

fn decode_with_tounicode(bytes: &[u8], map: &ToUnicodeMap) -> Option<String> {
    let (s1, m1, t1) = decode_bytes(bytes, 1, map);
    let mut best = (s1, m1, t1);

    if bytes.len().is_multiple_of(2) {
        let (s2, m2, t2) = decode_bytes(bytes, 2, map);
        if m2 > best.1 || (m2 == best.1 && s2.len() > best.0.len()) {
            best = (s2, m2, t2);
        }
    }

    if best.2 == 0 {
        return None;
    }

    let match_ratio = best.1 as f32 / best.2 as f32;
    if best.1 < 2 && match_ratio < 0.3 {
        return None;
    }
    if match_ratio < 0.05 {
        return None;
    }

    Some(best.0)
}

fn decode_bytes(bytes: &[u8], width: usize, map: &ToUnicodeMap) -> (String, usize, usize) {
    let mut out = String::new();
    let mut matches = 0usize;
    let mut total = 0usize;

    match width {
        1 => {
            for &b in bytes {
                total += 1;
                let code = b as u16;
                if let Some(s) = map.get(code) {
                    out.push_str(s);
                    matches += 1;
                } else {
                    out.push('\u{FFFD}');
                }
            }
        }
        2 => {
            for chunk in bytes.chunks_exact(2) {
                total += 1;
                let code = u16::from_be_bytes([chunk[0], chunk[1]]);
                if let Some(s) = map.get(code) {
                    out.push_str(s);
                    matches += 1;
                } else {
                    out.push('\u{FFFD}');
                }
            }
        }
        _ => return (String::new(), 0, 0),
    }

    (out, matches, total)
}

fn sanitize_extracted_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\n' | '\t' => out.push(ch),
            '\r' => out.push('\n'),
            '\u{FFFD}' => {}
            _ if ch.is_control() => {}
            _ => {
                let code = ch as u32;
                if is_private_use(code) || is_noncharacter(code) {
                    continue;
                }
                out.push(ch);
            }
        }
    }
    out
}

fn is_private_use(code: u32) -> bool {
    (0xE000..=0xF8FF).contains(&code)
        || (0xF0000..=0xFFFFD).contains(&code)
        || (0x100000..=0x10FFFD).contains(&code)
}

fn is_noncharacter(code: u32) -> bool {
    (0xFDD0..=0xFDEF).contains(&code) || (code & 0xFFFF == 0xFFFE) || (code & 0xFFFF == 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[cfg(unix)]
    #[test]
    #[ignore]
    fn can_open_pdfs_in_repo_tmp_dir() -> anyhow::Result<()> {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let tmp_dir = workspace_root.join("tmp");
        if !tmp_dir.is_dir() {
            return Ok(());
        }

        let engine = Engine::new();
        for entry in std::fs::read_dir(&tmp_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let is_pdf = path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false);
            if !is_pdf {
                continue;
            }

            let book = Book {
                path: bookshelf_core::encode_path(&path),
                title: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "untitled".to_string()),
                last_opened: None,
            };

            let pages = engine.page_count(&book)?;
            if pages == 0 {
                continue;
            }
            engine.render_page_text(&book, 0)?;
        }

        Ok(())
    }
}
