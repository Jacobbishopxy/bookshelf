//! Application orchestration layer for Bookshelf.

use bookshelf_core::{Book, Progress, Settings};

#[derive(Debug, Clone)]
pub struct AppContext {
    pub settings: Settings,
    pub cwd: String,
    pub books: Vec<Book>,
    pub selected: usize,
}

impl AppContext {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            cwd: String::new(),
            books: Vec::new(),
            selected: 0,
        }
    }

    pub fn with_library(mut self, cwd: String, books: Vec<Book>) -> Self {
        self.cwd = cwd;
        self.books = books;
        self.selected = self.selected.min(self.books.len().saturating_sub(1));
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
        let tracker = ProgressTracker::default();
        let progress = Progress {
            current_page: 1,
            total_pages: 4,
        };
        assert_eq!(tracker.percent(&progress), 25.0);
    }
}
