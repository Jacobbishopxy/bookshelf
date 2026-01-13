//! Test helpers and fixtures.

use bookshelf_core::{KittyImageQuality, ReaderMode, ReaderTextMode, ScanScope, Settings};

pub fn make_settings() -> Settings {
    Settings {
        reader_mode: ReaderMode::Text,
        reader_text_mode: ReaderTextMode::Reflow,
        reader_trim_headers_footers: true,
        kitty_image_quality: KittyImageQuality::Balanced,
        scan_scope: ScanScope::Recursive,
        library_roots: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_settings() {
        let settings = make_settings();
        assert_eq!(settings.reader_mode, ReaderMode::Text);
    }
}
