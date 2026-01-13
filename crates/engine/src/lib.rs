//! PDF engine wrapper.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use bookshelf_core::{Book, ReaderMode, ReaderTextMode, TocItem};
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

#[derive(Debug, Clone, Default)]
pub struct PageFurniture {
    header_lines: HashSet<String>,
    footer_lines: HashSet<String>,
}

const PAGE_FURNITURE_SAMPLE_PAGES: u32 = 8;
const PAGE_FURNITURE_TOP_K: usize = 3;
const PAGE_FURNITURE_BOTTOM_K: usize = 3;
const PAGE_FURNITURE_MIN_FRACTION: f32 = 0.6;

const TJ_INSERT_SPACE_THRESHOLD: f32 = -200.0;

impl PageFurniture {
    pub fn is_empty(&self) -> bool {
        self.header_lines.is_empty() && self.footer_lines.is_empty()
    }
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

    pub fn render_page_text_for_reader(
        &self,
        book: &Book,
        page_index: u32,
        text_mode: ReaderTextMode,
        furniture: Option<&PageFurniture>,
    ) -> anyhow::Result<String> {
        let raw = self.render_page_text(book, page_index)?;
        let trimmed = if text_mode == ReaderTextMode::Raw {
            raw
        } else if let Some(furniture) = furniture
            && !furniture.is_empty()
        {
            trim_page_furniture(&raw, furniture)
        } else {
            raw
        };

        Ok(match text_mode {
            ReaderTextMode::Raw | ReaderTextMode::Wrap => trimmed,
            ReaderTextMode::Reflow => reflow_reader_text(&trimmed),
        })
    }

    pub fn detect_page_furniture(&self, book: &Book) -> anyhow::Result<PageFurniture> {
        let total_pages = self
            .page_count(book)
            .ok()
            .unwrap_or(PAGE_FURNITURE_SAMPLE_PAGES);
        let sample_pages = total_pages.min(PAGE_FURNITURE_SAMPLE_PAGES).max(1);

        let mut sampled_pages = 0u32;
        let mut header_counts: HashMap<String, u32> = HashMap::new();
        let mut footer_counts: HashMap<String, u32> = HashMap::new();

        for page_index in 0..sample_pages {
            let text = match self.render_page_text(book, page_index) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if text.trim().is_empty() || text.trim().eq_ignore_ascii_case("no text found") {
                continue;
            }
            sampled_pages += 1;

            for line in take_top_boundary_lines(&text, PAGE_FURNITURE_TOP_K) {
                *header_counts.entry(line).or_insert(0) += 1;
            }
            for line in take_bottom_boundary_lines(&text, PAGE_FURNITURE_BOTTOM_K) {
                *footer_counts.entry(line).or_insert(0) += 1;
            }
        }

        if sampled_pages < 2 {
            return Ok(PageFurniture::default());
        }

        let min_repeats = ((sampled_pages as f32) * PAGE_FURNITURE_MIN_FRACTION).ceil() as u32;
        let min_repeats = min_repeats.max(2);

        let header_lines = header_counts
            .into_iter()
            .filter_map(|(line, count)| (count >= min_repeats).then_some(line))
            .collect::<HashSet<_>>();
        let footer_lines = footer_counts
            .into_iter()
            .filter_map(|(line, count)| (count >= min_repeats).then_some(line))
            .collect::<HashSet<_>>();

        Ok(PageFurniture {
            header_lines,
            footer_lines,
        })
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
        text_mode: ReaderTextMode,
        furniture: Option<&PageFurniture>,
        _viewport_width_chars: u16,
        _viewport_height_chars: u16,
    ) -> anyhow::Result<String> {
        match mode {
            ReaderMode::Text => {
                self.render_page_text_for_reader(book, page_index, text_mode, furniture)
            }
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
        let path = bookshelf_core::decode_path(&book.path);
        let file = FileOptions::cached()
            .open(&path)
            .with_context(|| format!("open pdf for page size: {}", path.display()))?;
        let page = file
            .get_page(page_index)
            .with_context(|| format!("get pdf page {page_index} for page size"))?;

        let rect = page
            .crop_box()
            .map_err(|err| anyhow::anyhow!(err))
            .context("get page crop box")?;
        let width = (rect.right - rect.left).abs().max(1.0);
        let height = (rect.top - rect.bottom).abs().max(1.0);
        Ok((width, height))
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
    let mut pending_space = false;

    let mut out = String::new();

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
                append_text_piece(&mut out, &s, &mut pending_space);
            }
            Op::TextDrawAdjusted { array } => {
                for item in array {
                    match item {
                        TextDrawAdjusted::Text(text) => {
                            let s = decode_pdf_string(
                                text,
                                current_font.as_ref(),
                                resolver,
                                resources,
                                &mut tounicode_cache,
                            );
                            append_text_piece(&mut out, &s, &mut pending_space);
                        }
                        TextDrawAdjusted::Spacing(spacing) => {
                            if *spacing <= TJ_INSERT_SPACE_THRESHOLD {
                                pending_space = true;
                            }
                        }
                    }
                }
            }
            Op::TextNewline => {
                out.push('\n');
                pending_space = false;
            }
            Op::MoveTextPosition { translation } => {
                if translation.y < 0.0 {
                    out.push('\n');
                    pending_space = false;
                }
            }
            _ => {}
        }
    }

    out
}

fn append_text_piece(out: &mut String, s: &str, pending_space: &mut bool) {
    let sanitized = sanitize_extracted_text(s);
    let trimmed = sanitized.trim_matches('\0');
    if trimmed.is_empty() {
        return;
    }

    if *pending_space {
        let first_non_ws = trimmed.chars().find(|ch| !ch.is_whitespace());
        let suppress_before = first_non_ws
            .is_some_and(|ch| matches!(ch, ',' | '.' | ';' | ':' | '!' | '?' | ')' | ']' | '}'));
        if !out.is_empty()
            && !suppress_before
            && !out.ends_with([' ', '\n', '\t'])
            && !trimmed.starts_with(char::is_whitespace)
        {
            out.push(' ');
        }
        *pending_space = false;
    }
    out.push_str(trimmed);
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

fn take_top_boundary_lines(text: &str, k: usize) -> Vec<String> {
    if k == 0 {
        return Vec::new();
    }

    let mut out: Vec<String> = Vec::with_capacity(k);
    for line in text.lines() {
        let normalized = normalize_line_for_reflow(line);
        if normalized.is_empty() {
            continue;
        }
        out.push(normalized);
        if out.len() >= k {
            break;
        }
    }
    out
}

fn take_bottom_boundary_lines(text: &str, k: usize) -> Vec<String> {
    if k == 0 {
        return Vec::new();
    }

    let mut out: Vec<String> = Vec::with_capacity(k);
    for line in text.lines().rev() {
        let normalized = normalize_line_for_reflow(line);
        if normalized.is_empty() {
            continue;
        }
        out.push(normalized);
        if out.len() >= k {
            break;
        }
    }
    out.reverse();
    out
}

fn trim_page_furniture(text: &str, furniture: &PageFurniture) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("no text found") {
        return text.to_string();
    }

    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return text.to_string();
    }

    let mut to_remove: HashSet<usize> = HashSet::new();

    let mut header_seen = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        let normalized = normalize_line_for_reflow(line);
        if normalized.is_empty() {
            continue;
        }
        header_seen += 1;
        if header_seen > PAGE_FURNITURE_TOP_K {
            break;
        }
        if furniture.header_lines.contains(&normalized) {
            to_remove.insert(idx);
        }
    }

    let mut footer_seen = 0usize;
    for (rev_idx, line) in lines.iter().rev().enumerate() {
        let normalized = normalize_line_for_reflow(line);
        if normalized.is_empty() {
            continue;
        }
        footer_seen += 1;
        if footer_seen > PAGE_FURNITURE_BOTTOM_K {
            break;
        }
        let idx = lines.len().saturating_sub(1 + rev_idx);
        if furniture.footer_lines.contains(&normalized) {
            to_remove.insert(idx);
        }
    }

    if to_remove.is_empty() {
        return text.to_string();
    }

    let out = lines
        .into_iter()
        .enumerate()
        .filter_map(|(idx, line)| (!to_remove.contains(&idx)).then_some(line))
        .collect::<Vec<_>>()
        .join("\n");

    if out.trim().is_empty() {
        text.to_string()
    } else {
        out
    }
}

fn reflow_reader_text(raw: &str) -> String {
    let sanitized = sanitize_extracted_text(raw);
    let mut lines: Vec<&str> = sanitized.split('\n').collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }

    let mut trimmed_lens = lines
        .iter()
        .map(|line| line.trim().len())
        .filter(|len| *len > 0)
        .collect::<Vec<_>>();
    trimmed_lens.sort_unstable();
    let typical_len = trimmed_lens
        .get(trimmed_lens.len() / 2)
        .copied()
        .unwrap_or(0);

    let short_threshold = (typical_len as f32 * 0.6).round() as usize;

    let mut out = String::new();
    let mut paragraph = String::new();
    let mut prev_len = 0usize;
    let mut prev_blank = true;

    for raw_line in lines {
        let had_indent = raw_line.starts_with("  ") || raw_line.starts_with('\t');
        let line = normalize_line_for_reflow(raw_line);
        if line.is_empty() {
            flush_paragraph(&mut out, &mut paragraph, &mut prev_blank);
            continue;
        }

        if !paragraph.is_empty() {
            let starts_new = is_bullet_start(&line)
                || (had_indent && !prev_blank)
                || (prev_len > 0
                    && short_threshold > 0
                    && prev_len <= short_threshold
                    && starts_with_uppercase_word(&line));

            if starts_new {
                flush_paragraph(&mut out, &mut paragraph, &mut prev_blank);
            }
        }

        append_reflow_line(&mut paragraph, &line);
        prev_len = line.len();
        prev_blank = false;
    }

    flush_paragraph(&mut out, &mut paragraph, &mut prev_blank);
    out
}

fn normalize_line_for_reflow(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut last_was_space = false;

    for ch in line.chars() {
        if ch == '\u{00AD}' {
            continue;
        }

        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
            continue;
        }

        out.push(ch);
        last_was_space = false;
    }

    out.trim().to_string()
}

fn flush_paragraph(out: &mut String, paragraph: &mut String, prev_blank: &mut bool) {
    let text = paragraph.trim();
    if text.is_empty() {
        return;
    }

    if !out.is_empty() {
        out.push('\n');
        out.push('\n');
    } else if *prev_blank {
        // no-op: avoid leading blank paragraphs
    }

    out.push_str(text);
    paragraph.clear();
    *prev_blank = true;
}

fn append_reflow_line(paragraph: &mut String, line: &str) {
    if paragraph.is_empty() {
        paragraph.push_str(line);
        return;
    }

    if paragraph.ends_with('-') && should_dehyphenate(paragraph, line) {
        paragraph.pop();
        paragraph.push_str(line);
        return;
    }

    if !paragraph.ends_with(' ') {
        paragraph.push(' ');
    }
    paragraph.push_str(line);
}

fn should_dehyphenate(paragraph: &str, next: &str) -> bool {
    if paragraph.ends_with("--") {
        return false;
    }

    let prev = paragraph
        .chars()
        .rev()
        .nth(1)
        .is_some_and(|ch| ch.is_alphabetic());
    let next = next.chars().next().is_some_and(|ch| ch.is_alphabetic());
    prev && next
}

fn is_bullet_start(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with(['•', '-', '*', '–', '—']) {
        return line.chars().nth(1).is_some_and(|ch| ch.is_whitespace());
    }

    let mut chars = line.chars().peekable();
    let mut saw_digit = false;
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            saw_digit = true;
            let _ = chars.next();
            continue;
        }
        break;
    }
    if !saw_digit {
        return false;
    }

    matches!(chars.next(), Some('.') | Some(')'))
        && chars.peek().copied().is_some_and(|ch| ch.is_whitespace())
}

fn starts_with_uppercase_word(line: &str) -> bool {
    for ch in line.chars() {
        if !ch.is_alphabetic() {
            continue;
        }
        return ch.is_uppercase();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf::object::NoResolve;
    use std::path::Path;

    fn empty_resources() -> Resources {
        Resources {
            graphics_states: HashMap::new(),
            color_spaces: HashMap::new(),
            pattern: HashMap::new(),
            xobjects: HashMap::new(),
            fonts: HashMap::new(),
            properties: HashMap::new(),
        }
    }

    #[test]
    fn reflow_joins_lines() {
        let input = "Hello\nworld\n";
        assert_eq!(reflow_reader_text(input), "Hello world");
    }

    #[test]
    fn reflow_preserves_blank_lines() {
        let input = "Hello\n\nWorld\n";
        assert_eq!(reflow_reader_text(input), "Hello\n\nWorld");
    }

    #[test]
    fn reflow_dehyphenates_line_breaks() {
        let input = "micro-\nscopic\n";
        assert_eq!(reflow_reader_text(input), "microscopic");
    }

    #[test]
    fn reflow_breaks_on_short_line_then_caps() {
        let input = "This is a longer line with words\nShort.\nNext Paragraph starts here\n";
        assert_eq!(
            reflow_reader_text(input),
            "This is a longer line with words Short.\n\nNext Paragraph starts here"
        );
    }

    #[test]
    fn trim_page_furniture_removes_repeated_boundary_lines() {
        let mut furniture = PageFurniture::default();
        furniture.header_lines.insert("Bookshelf".to_string());
        furniture.footer_lines.insert("Confidential".to_string());

        let input = "Bookshelf\n\nHello world\n\nConfidential";
        assert_eq!(trim_page_furniture(input, &furniture), "\nHello world\n");
    }

    #[test]
    fn trim_page_furniture_does_not_blank_entire_page() {
        let mut furniture = PageFurniture::default();
        furniture.header_lines.insert("Header".to_string());
        furniture.footer_lines.insert("Footer".to_string());

        let input = "Header\n\nFooter";
        assert_eq!(trim_page_furniture(input, &furniture), input);
    }

    #[test]
    fn boundary_lines_skip_blank_lines() {
        let input = "\n\nTop\n\nBody\n\nBottom\n\n";
        assert_eq!(
            take_top_boundary_lines(input, 2),
            vec!["Top".to_string(), "Body".to_string()]
        );
        assert_eq!(
            take_bottom_boundary_lines(input, 2),
            vec!["Body".to_string(), "Bottom".to_string()]
        );
    }

    #[test]
    fn ops_to_text_does_not_insert_spaces_between_single_chars() {
        let resources = empty_resources();
        let ops = vec![
            Op::TextDraw {
                text: PdfString::from("M"),
            },
            Op::TextDraw {
                text: PdfString::from("a"),
            },
            Op::TextDraw {
                text: PdfString::from("t"),
            },
        ];
        assert_eq!(ops_to_text(&ops, &NoResolve, &resources), "Mat");
    }

    #[test]
    fn ops_to_text_inserts_space_on_large_tj_spacing() {
        let resources = empty_resources();
        let ops = vec![Op::TextDrawAdjusted {
            array: vec![
                TextDrawAdjusted::Text(PdfString::from("Hello")),
                TextDrawAdjusted::Spacing(-300.0),
                TextDrawAdjusted::Text(PdfString::from("world")),
            ],
        }];
        assert_eq!(ops_to_text(&ops, &NoResolve, &resources), "Hello world");
    }

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
