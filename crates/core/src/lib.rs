//! Core domain types for Bookshelf.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub preview_mode: PreviewMode,
    pub preview_depth: usize,
    pub scan_scope: ScanScope,
    pub library_roots: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewMode {
    Text,
    Braille,
    Blocks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanScope {
    Direct,
    Recursive,
}

impl PreviewMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            PreviewMode::Text => "text",
            PreviewMode::Braille => "braille",
            PreviewMode::Blocks => "blocks",
        }
    }
}

impl std::fmt::Display for PreviewMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PreviewMode {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(PreviewMode::Text),
            "braille" => Ok(PreviewMode::Braille),
            "blocks" => Ok(PreviewMode::Blocks),
            _ => Err("unknown preview mode"),
        }
    }
}

impl ScanScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScanScope::Direct => "direct",
            ScanScope::Recursive => "recursive",
        }
    }
}

impl std::fmt::Display for ScanScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ScanScope {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "direct" => Ok(ScanScope::Direct),
            "recursive" => Ok(ScanScope::Recursive),
            _ => Err("unknown scan scope"),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            preview_mode: PreviewMode::Text,
            preview_depth: 5,
            scan_scope: ScanScope::Recursive,
            library_roots: Vec::new(),
        }
    }
}

impl Settings {
    pub fn normalize(&mut self) {
        self.preview_depth = self.preview_depth.clamp(1, 200);
        self.library_roots = self
            .library_roots
            .iter()
            .map(|root| root.trim().to_string())
            .filter(|root| !root.is_empty())
            .collect();
        self.library_roots.sort();
        self.library_roots.dedup();
    }

    pub fn cycle_preview_mode(&mut self) {
        self.preview_mode = match self.preview_mode {
            PreviewMode::Text => PreviewMode::Braille,
            PreviewMode::Braille => PreviewMode::Blocks,
            PreviewMode::Blocks => PreviewMode::Text,
        };
    }

    pub fn cycle_scan_scope(&mut self) {
        self.scan_scope = match self.scan_scope {
            ScanScope::Direct => ScanScope::Recursive,
            ScanScope::Recursive => ScanScope::Direct,
        };
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    pub path: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub current_page: u32,
    pub total_pages: u32,
}

impl Progress {
    pub fn percent(&self) -> f32 {
        if self.total_pages == 0 {
            0.0
        } else {
            (self.current_page as f32 / self.total_pages as f32) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_use_preview_depth() {
        let settings = Settings::default();
        assert!(settings.preview_depth > 0);
    }

    #[test]
    fn cycle_preview_mode_rotates() {
        let mut settings = Settings::default();
        assert_eq!(settings.preview_mode, PreviewMode::Text);
        settings.cycle_preview_mode();
        assert_eq!(settings.preview_mode, PreviewMode::Braille);
        settings.cycle_preview_mode();
        assert_eq!(settings.preview_mode, PreviewMode::Blocks);
        settings.cycle_preview_mode();
        assert_eq!(settings.preview_mode, PreviewMode::Text);
    }

    #[test]
    fn preview_mode_parses_strings() {
        assert_eq!("text".parse::<PreviewMode>().unwrap(), PreviewMode::Text);
        assert_eq!(
            "Braille".parse::<PreviewMode>().unwrap(),
            PreviewMode::Braille
        );
        assert_eq!(
            " BLOCKS ".parse::<PreviewMode>().unwrap(),
            PreviewMode::Blocks
        );
        assert!("nope".parse::<PreviewMode>().is_err());
    }

    #[test]
    fn scan_scope_parses_strings() {
        assert_eq!("direct".parse::<ScanScope>().unwrap(), ScanScope::Direct);
        assert_eq!(
            "Recursive".parse::<ScanScope>().unwrap(),
            ScanScope::Recursive
        );
        assert!("nope".parse::<ScanScope>().is_err());
    }

    #[test]
    fn settings_normalizes_depth() {
        let mut settings = Settings {
            preview_mode: PreviewMode::Text,
            preview_depth: 0,
            scan_scope: ScanScope::Direct,
            library_roots: vec![" ".to_string(), "/a".to_string(), "/a".to_string(), " /b ".to_string()],
        };
        settings.normalize();
        assert_eq!(settings.preview_depth, 1);
        assert_eq!(settings.library_roots, vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn progress_handles_zero_pages() {
        let progress = Progress {
            current_page: 1,
            total_pages: 0,
        };
        assert_eq!(progress.percent(), 0.0);
    }
}
