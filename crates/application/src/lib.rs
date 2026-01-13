//! Application orchestration layer for Bookshelf.

use std::collections::HashMap;
use std::collections::HashSet;

use bookshelf_core::{Book, BookLabels, Bookmark, Note, Progress, Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagMatchMode {
    And,
    Or,
}

impl Default for TagMatchMode {
    fn default() -> Self {
        Self::Or
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionFilter {
    Any,
    None,
    Selected(String),
}

impl Default for CollectionFilter {
    fn default() -> Self {
        Self::Any
    }
}

#[derive(Debug, Clone)]
pub struct AppContext {
    pub settings: Settings,
    pub cwd: String,
    pub books: Vec<Book>,
    pub selected: usize,
    pub library_query: String,
    pub favorites_only: bool,
    pub collection_filter: CollectionFilter,
    pub tag_filters: Vec<String>,
    pub tag_match_mode: TagMatchMode,
    pub progress_by_path: HashMap<String, u32>,
    pub opened_at_by_path: HashMap<String, i64>,
    pub labels_by_path: HashMap<String, BookLabels>,
    pub bookmarks_by_path: HashMap<String, Vec<Bookmark>>,
    pub notes_by_path: HashMap<String, Vec<Note>>,
    pub dirty_favorite_paths: HashSet<String>,
    pub dirty_label_paths: HashSet<String>,
    pub dirty_bookmark_paths: HashSet<String>,
    pub dirty_note_paths: HashSet<String>,
}

impl AppContext {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            cwd: String::new(),
            books: Vec::new(),
            selected: 0,
            library_query: String::new(),
            favorites_only: false,
            collection_filter: CollectionFilter::Any,
            tag_filters: Vec::new(),
            tag_match_mode: TagMatchMode::Or,
            progress_by_path: HashMap::new(),
            opened_at_by_path: HashMap::new(),
            labels_by_path: HashMap::new(),
            bookmarks_by_path: HashMap::new(),
            notes_by_path: HashMap::new(),
            dirty_favorite_paths: HashSet::new(),
            dirty_label_paths: HashSet::new(),
            dirty_bookmark_paths: HashSet::new(),
            dirty_note_paths: HashSet::new(),
        }
    }

    pub fn with_library(mut self, cwd: String, books: Vec<Book>) -> Self {
        self.cwd = cwd;
        self.books = books;
        self.selected = self.selected.min(self.books.len().saturating_sub(1));
        self
    }

    pub fn with_progress(mut self, progress_by_path: HashMap<String, u32>) -> Self {
        self.progress_by_path = progress_by_path;
        self
    }

    pub fn with_labels(mut self, labels_by_path: HashMap<String, BookLabels>) -> Self {
        self.labels_by_path = labels_by_path;
        self
    }

    pub fn with_bookmarks(mut self, bookmarks_by_path: HashMap<String, Vec<Bookmark>>) -> Self {
        self.bookmarks_by_path = bookmarks_by_path;
        self
    }

    pub fn with_notes(mut self, notes_by_path: HashMap<String, Vec<Note>>) -> Self {
        self.notes_by_path = notes_by_path;
        self
    }
}

#[derive(Debug, Default)]
pub struct ProgressTracker;

impl ProgressTracker {
    pub fn percent(&self, progress: &Progress) -> f32 {
        progress.percent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_uses_progress() {
        let tracker = ProgressTracker;
        let progress = Progress {
            current_page: 1,
            total_pages: 4,
        };
        assert_eq!(tracker.percent(&progress), 25.0);
    }
}
