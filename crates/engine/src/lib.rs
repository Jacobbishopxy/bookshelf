//! PDF engine wrapper (using `pdf` crate later).

use bookshelf_core::{Book, PreviewMode, Settings};

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
        let header = format!("{} ({})", book.title, book.path);
        format!("{header}\n{}", self.render_preview(settings))
    }
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
