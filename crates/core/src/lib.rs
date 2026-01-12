//! Core domain types for Bookshelf.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookId(pub String);

const UTF8_PATH_PREFIX: &str = "utf8:";
const BYTES_PATH_PREFIX: &str = "osbytes:";

pub fn encode_path(path: &Path) -> String {
    match path.to_str() {
        Some(s) => {
            if s.starts_with(UTF8_PATH_PREFIX) || s.starts_with(BYTES_PATH_PREFIX) {
                format!("{UTF8_PATH_PREFIX}{s}")
            } else {
                s.to_string()
            }
        }
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt as _;
                let bytes = path.as_os_str().as_bytes();
                format!("{BYTES_PATH_PREFIX}{}", hex_encode(bytes))
            }
            #[cfg(not(unix))]
            {
                path.to_string_lossy().to_string()
            }
        }
    }
}

pub fn decode_path(encoded: &str) -> PathBuf {
    if let Some(rest) = encoded.strip_prefix(UTF8_PATH_PREFIX) {
        return PathBuf::from(rest);
    }

    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        if let Some(hex) = encoded.strip_prefix(BYTES_PATH_PREFIX)
            && let Some(bytes) = hex_decode(hex)
        {
            return PathBuf::from(OsString::from_vec(bytes));
        }
    }

    PathBuf::from(encoded)
}

pub fn display_path(encoded: &str) -> String {
    decode_path(encoded).display().to_string()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }

    let bytes = hex.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }

    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = val(pair[0])?;
        let lo = val(pair[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub reader_mode: ReaderMode,
    pub kitty_image_quality: KittyImageQuality,
    pub scan_scope: ScanScope,
    pub library_roots: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReaderMode {
    Text,
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KittyImageQuality {
    Fast,
    Balanced,
    Sharp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanScope {
    Direct,
    Recursive,
}

impl ReaderMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReaderMode::Text => "text",
            ReaderMode::Image => "image",
        }
    }
}

impl KittyImageQuality {
    pub fn as_str(&self) -> &'static str {
        match self {
            KittyImageQuality::Fast => "fast",
            KittyImageQuality::Balanced => "balanced",
            KittyImageQuality::Sharp => "sharp",
        }
    }

    pub fn max_transmit_pixels(&self) -> u64 {
        match self {
            KittyImageQuality::Fast => 750_000,
            KittyImageQuality::Balanced => 1_250_000,
            KittyImageQuality::Sharp => 2_500_000,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            KittyImageQuality::Fast => KittyImageQuality::Balanced,
            KittyImageQuality::Balanced => KittyImageQuality::Sharp,
            KittyImageQuality::Sharp => KittyImageQuality::Fast,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            KittyImageQuality::Fast => KittyImageQuality::Sharp,
            KittyImageQuality::Balanced => KittyImageQuality::Fast,
            KittyImageQuality::Sharp => KittyImageQuality::Balanced,
        }
    }
}

impl std::fmt::Display for ReaderMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Display for KittyImageQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ReaderMode {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(ReaderMode::Text),
            "image" => Ok(ReaderMode::Image),
            _ => Err("unknown reader mode"),
        }
    }
}

impl std::str::FromStr for KittyImageQuality {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fast" => Ok(KittyImageQuality::Fast),
            "balanced" => Ok(KittyImageQuality::Balanced),
            "sharp" => Ok(KittyImageQuality::Sharp),
            _ => Err("unknown kitty image quality"),
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
            reader_mode: ReaderMode::Text,
            kitty_image_quality: KittyImageQuality::Balanced,
            scan_scope: ScanScope::Recursive,
            library_roots: Vec::new(),
        }
    }
}

impl Settings {
    pub fn normalize(&mut self) {
        self.library_roots = self
            .library_roots
            .iter()
            .map(|root| root.trim().to_string())
            .filter(|root| !root.is_empty())
            .collect();
        self.library_roots.sort();
        self.library_roots.dedup();
    }

    pub fn cycle_reader_mode(&mut self) {
        self.reader_mode = match self.reader_mode {
            ReaderMode::Text => ReaderMode::Image,
            ReaderMode::Image => ReaderMode::Text,
        };
    }

    pub fn cycle_kitty_image_quality_next(&mut self) {
        self.kitty_image_quality = self.kitty_image_quality.next();
    }

    pub fn cycle_kitty_image_quality_prev(&mut self) {
        self.kitty_image_quality = self.kitty_image_quality.prev();
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
    pub last_opened: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub page: u32,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub page: u32,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocItem {
    pub title: String,
    pub page: Option<u32>,
    pub depth: usize,
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
    fn non_utf8_paths_roundtrip_through_encoding() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::ffi::OsStringExt as _;

        let original = OsString::from_vec(vec![b'a', 0xa1, 0xaf, b'b']);
        let original_path = PathBuf::from(original.clone());
        let encoded = encode_path(&original_path);
        let decoded = decode_path(&encoded);
        assert_eq!(decoded.as_os_str().as_bytes(), original.as_bytes());
    }

    #[test]
    fn cycle_reader_mode_rotates() {
        let mut settings = Settings::default();
        assert_eq!(settings.reader_mode, ReaderMode::Text);
        settings.cycle_reader_mode();
        assert_eq!(settings.reader_mode, ReaderMode::Image);
        settings.cycle_reader_mode();
        assert_eq!(settings.reader_mode, ReaderMode::Text);
    }

    #[test]
    fn reader_mode_parses_strings() {
        assert_eq!("text".parse::<ReaderMode>().unwrap(), ReaderMode::Text);
        assert_eq!(" IMAGE ".parse::<ReaderMode>().unwrap(), ReaderMode::Image);
        assert!("nope".parse::<ReaderMode>().is_err());
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
            reader_mode: ReaderMode::Text,
            kitty_image_quality: KittyImageQuality::Balanced,
            scan_scope: ScanScope::Direct,
            library_roots: vec![
                " ".to_string(),
                "/a".to_string(),
                "/a".to_string(),
                " /b ".to_string(),
            ],
        };
        settings.normalize();
        assert_eq!(
            settings.library_roots,
            vec!["/a".to_string(), "/b".to_string()]
        );
    }

    #[test]
    fn kitty_image_quality_parses_strings() {
        assert_eq!(
            "fast".parse::<KittyImageQuality>().unwrap(),
            KittyImageQuality::Fast
        );
        assert_eq!(
            " Sharp ".parse::<KittyImageQuality>().unwrap(),
            KittyImageQuality::Sharp
        );
        assert!("nope".parse::<KittyImageQuality>().is_err());
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
