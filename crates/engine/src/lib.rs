//! PDF engine wrapper (using `pdf` crate later).

use std::collections::HashMap;
use std::fmt::Write as _;

use bookshelf_core::{Book, PreviewMode, Settings};
use pdf::content::{Op, TextDrawAdjusted};
use pdf::file::FileOptions;
use pdf::font::ToUnicodeMap;
use pdf::object::{Resolve, Resources};
use pdf::primitive::{Name, PdfString};

#[derive(Debug, Default)]
pub struct Engine;

impl Engine {
    pub fn new() -> Self {
        Self
    }

    pub fn preview_depth(&self, settings: &Settings) -> usize {
        settings.preview_depth
    }

    pub fn render_preview(&self, settings: &Settings) -> String {
        let depth = settings.preview_depth.max(1);
        match settings.preview_mode {
            PreviewMode::Text => (1..=depth)
                .map(|i| format!("text preview line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            PreviewMode::Braille => (1..=depth)
                .map(|i| format!("braille preview row {i}: ⣿⣿⣷⣄…"))
                .collect::<Vec<_>>()
                .join("\n"),
            PreviewMode::Blocks => (1..=depth)
                .map(|i| format!("blocks preview row {i}: ███▓▒░…"))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    pub fn render_preview_for(&self, book: &Book, settings: &Settings) -> String {
        match settings.preview_mode {
            PreviewMode::Text => match self.render_text_preview(book, settings) {
                Ok(text) => text,
                Err(err) => format!("(error reading pdf: {err})"),
            },
            _ => self.render_preview(settings),
        }
    }

    pub fn page_count(&self, book: &Book) -> anyhow::Result<u32> {
        let file = FileOptions::cached().open(&book.path)?;
        Ok(file.num_pages())
    }

    pub fn render_page_text(&self, book: &Book, page_index: u32) -> anyhow::Result<String> {
        let file = FileOptions::cached().open(&book.path)?;
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
        let file = FileOptions::cached().open(&book.path)?;
        let resolver = file.resolver();
        let page = file.get_page(page_index)?;
        let resources = page.resources()?;

        let Some(content) = &page.contents else {
            return Ok(format!(
                "book: {}\npage: {}\n(no page contents)\n",
                book.path,
                page_index + 1
            ));
        };

        let ops = content.operations(&resolver)?;

        let mut out = String::new();
        writeln!(&mut out, "book: {}", book.path)?;
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
                    dump_text_op(&mut out, idx, current_font.as_ref(), text, &resolver, resources, &mut tounicode_cache)?;
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

    fn render_text_preview(&self, book: &Book, settings: &Settings) -> anyhow::Result<String> {
        let file = FileOptions::cached().open(&book.path)?;
        let resolver = file.resolver();

        let mut out = String::new();
        let mut any_text = false;
        let max_pages = settings.preview_pages.max(1) as usize;
        let max_lines = settings.preview_depth.max(1);

        for (idx, page_res) in file.pages().take(max_pages).enumerate() {
            let page = match page_res {
                Ok(p) => p,
                Err(_) => continue,
            };

            let Some(content) = &page.contents else {
                continue;
            };

            let ops = match content.operations(&resolver) {
                Ok(ops) => ops,
                Err(_) => continue,
            };

            let resources = match page.resources() {
                Ok(r) => r,
                Err(_) => continue,
            };
            let text = ops_to_text(&ops, &resolver, resources);
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }

            any_text = true;
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format!("--- Page {} ---\n", idx + 1));
            out.push_str(&text);

            if out.lines().count() >= max_lines {
                break;
            }
        }

        if !any_text {
            Ok("no text found".to_string())
        } else {
            Ok(out.lines().take(max_lines).collect::<Vec<_>>().join("\n"))
        }
    }
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
                let s = decode_pdf_string(text, current_font.as_ref(), resolver, resources, &mut tounicode_cache);
                append_text(&mut out, &s, &mut needs_space);
            }
            Op::TextDrawAdjusted { array } => {
                for item in array {
                    if let TextDrawAdjusted::Text(text) = item {
                        let s = decode_pdf_string(text, current_font.as_ref(), resolver, resources, &mut tounicode_cache);
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

    if bytes.len() % 2 == 0 {
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

    #[test]
    fn propagates_preview_depth() {
        let settings = Settings {
            preview_mode: PreviewMode::Text,
            preview_depth: 8,
            ..Settings::default()
        };
        let engine = Engine::new();
        assert_eq!(engine.preview_depth(&settings), 8);
    }

    #[test]
    fn renders_preview_for_each_mode() {
        let engine = Engine::new();

        let settings = Settings {
            preview_mode: PreviewMode::Text,
            preview_depth: 2,
            ..Settings::default()
        };
        assert!(engine.render_preview(&settings).contains("text preview"));

        let settings = Settings {
            preview_mode: PreviewMode::Braille,
            preview_depth: 2,
            ..Settings::default()
        };
        assert!(engine.render_preview(&settings).contains("braille preview"));

        let settings = Settings {
            preview_mode: PreviewMode::Blocks,
            preview_depth: 2,
            ..Settings::default()
        };
        assert!(engine.render_preview(&settings).contains("blocks preview"));
    }
}
