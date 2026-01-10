//! Test helpers and fixtures.

use bookshelf_core::{PreviewMode, ReaderMode, ScanScope, Settings};

pub fn make_settings(preview_depth: usize) -> Settings {
    Settings {
        preview_mode: PreviewMode::Text,
        reader_mode: ReaderMode::Text,
        preview_depth,
        preview_pages: 2,
        scan_scope: ScanScope::Recursive,
        library_roots: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_settings() {
        let settings = make_settings(12);
        assert_eq!(settings.preview_depth, 12);
    }
}
