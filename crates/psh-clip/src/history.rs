//! Clipboard history storage and search.
//!
//! Provides [`ClipEntry`] for representing individual clipboard items (text or image)
//! and [`ClipHistory`] for thread-safe, bounded, deduplicated history management.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// A single clipboard entry — either text or a cached image file.
///
/// Serialized with a `"kind"` tag so JSON distinguishes `{"kind":"Text",...}`
/// from `{"kind":"Image",...}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind")]
pub enum ClipEntry {
    /// Plain text copied to the clipboard.
    Text {
        /// The full clipboard text content.
        content: String,
    },
    /// An image saved to the cache directory.
    Image {
        /// Filesystem path to the cached image file.
        path: PathBuf,
        /// MIME type of the image (e.g. `"image/png"`).
        mime: String,
    },
}

impl ClipEntry {
    /// Returns a short display string for the picker UI.
    ///
    /// Text entries show the first line, truncated to 80 characters with `"..."`.
    /// Image entries show `[mime] filename`.
    pub fn display_text(&self) -> String {
        match self {
            ClipEntry::Text { content } => {
                let line = content.lines().next().unwrap_or("");
                if line.len() > 80 {
                    let boundary = line.floor_char_boundary(80);
                    format!("{}...", &line[..boundary])
                } else {
                    line.to_string()
                }
            }
            ClipEntry::Image { path, mime } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("[{mime}] {name}")
            }
        }
    }

    /// Returns true if this entry matches a search query (case-insensitive).
    ///
    /// Text entries match against their content. Image entries match against
    /// their filename and MIME type.
    pub fn matches(&self, query: &str) -> bool {
        let query_lower = query.to_lowercase();
        match self {
            ClipEntry::Text { content } => content.to_lowercase().contains(&query_lower),
            ClipEntry::Image { path, mime } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                name.contains(&query_lower) || mime.to_lowercase().contains(&query_lower)
            }
        }
    }
}

/// Inner state shared across all [`ClipHistory`] clones.
struct ClipHistoryInner {
    entries: VecDeque<ClipEntry>,
    max: usize,
}

/// Thread-safe clipboard history with deduplication and max capacity.
///
/// Entries are stored newest-first in a [`VecDeque`]. Pushing a duplicate
/// entry removes the old copy and places the new one at the front.
/// Cloning a `ClipHistory` shares the same underlying storage and capacity.
#[derive(Debug, Clone)]
pub struct ClipHistory {
    inner: Arc<Mutex<ClipHistoryInner>>,
}

// Manual Debug since ClipHistoryInner doesn't derive it
impl std::fmt::Debug for ClipHistoryInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipHistoryInner")
            .field("entries", &self.entries)
            .field("max", &self.max)
            .finish()
    }
}

impl ClipHistory {
    /// Creates an empty history with the given maximum capacity.
    #[cfg(test)]
    pub fn new(max: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ClipHistoryInner {
                entries: VecDeque::with_capacity(max),
                max,
            })),
        }
    }

    /// Creates a history pre-populated from persisted entries.
    pub fn load_from(entries: Vec<ClipEntry>, max: usize) -> Self {
        let deque: VecDeque<ClipEntry> = entries.into_iter().take(max).collect();
        Self {
            inner: Arc::new(Mutex::new(ClipHistoryInner {
                entries: deque,
                max,
            })),
        }
    }

    /// Updates the maximum capacity for all clones, trimming excess entries.
    pub fn set_max(&self, max: usize) {
        let mut inner = self.inner.lock().unwrap();
        inner.max = max;
        while inner.entries.len() > max {
            inner.entries.pop_back();
        }
    }

    /// Pushes an entry to the front, deduplicating and enforcing max capacity.
    ///
    /// If the entry already exists anywhere in the history, the old copy is
    /// removed before inserting at the front. If the history is at capacity,
    /// the oldest entry is evicted.
    pub fn push(&self, entry: ClipEntry) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(idx) = inner.entries.iter().position(|e| e == &entry) {
            inner.entries.remove(idx);
        }
        if inner.entries.len() >= inner.max {
            inner.entries.pop_back();
        }
        inner.entries.push_front(entry);
    }

    /// Returns a clone of the most recent entry, or `None` if empty.
    ///
    /// Cheaper than `items()` when you only need the head element.
    pub fn peek_first(&self) -> Option<ClipEntry> {
        let inner = self.inner.lock().unwrap();
        inner.entries.front().cloned()
    }

    /// Returns a snapshot of all entries (newest first).
    pub fn items(&self) -> Vec<ClipEntry> {
        let inner = self.inner.lock().unwrap();
        inner.entries.iter().cloned().collect()
    }

    /// Returns entries matching the query (newest first).
    pub fn search(&self, query: &str) -> Vec<ClipEntry> {
        let inner = self.inner.lock().unwrap();
        inner
            .entries
            .iter()
            .filter(|e| e.matches(query))
            .cloned()
            .collect()
    }

    /// Clears all entries.
    #[cfg(test)]
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> ClipEntry {
        ClipEntry::Text {
            content: s.to_string(),
        }
    }

    fn image(name: &str) -> ClipEntry {
        ClipEntry::Image {
            path: PathBuf::from(format!("/tmp/{name}")),
            mime: "image/png".to_string(),
        }
    }

    #[test]
    fn push_and_retrieve() {
        let h = ClipHistory::new(5);
        h.push(text("first"));
        h.push(text("second"));
        let items = h.items();
        assert_eq!(items, vec![text("second"), text("first")]);
    }

    #[test]
    fn deduplicates() {
        let h = ClipHistory::new(5);
        h.push(text("a"));
        h.push(text("b"));
        h.push(text("a"));
        let items = h.items();
        assert_eq!(items, vec![text("a"), text("b")]);
    }

    #[test]
    fn respects_max() {
        let h = ClipHistory::new(2);
        h.push(text("a"));
        h.push(text("b"));
        h.push(text("c"));
        let items = h.items();
        assert_eq!(items.len(), 2);
        assert_eq!(items, vec![text("c"), text("b")]);
    }

    #[test]
    fn load_from_persisted() {
        let entries = vec![text("one"), text("two"), text("three")];
        let h = ClipHistory::load_from(entries, 2);
        let items = h.items();
        assert_eq!(items, vec![text("one"), text("two")]);
    }

    #[test]
    fn search_text() {
        let h = ClipHistory::new(10);
        h.push(text("hello world"));
        h.push(text("goodbye world"));
        h.push(text("hello there"));
        let results = h.search("hello");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.matches("hello")));
    }

    #[test]
    fn search_case_insensitive() {
        let h = ClipHistory::new(10);
        h.push(text("Hello World"));
        let results = h.search("hello");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_image_by_mime() {
        let h = ClipHistory::new(10);
        h.push(image("screenshot.png"));
        h.push(text("some text"));
        let results = h.search("png");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn display_text_truncates() {
        let long = "a".repeat(200);
        let entry = ClipEntry::Text {
            content: long.clone(),
        };
        let display = entry.display_text();
        assert_eq!(display.len(), 83); // 80 + "..."
        assert!(display.ends_with("..."));
    }

    #[test]
    fn display_text_truncates_multibyte_safely() {
        // 40 two-byte chars = 80 bytes, but the 80th byte is within a char
        let long = "é".repeat(50); // each é is 2 bytes
        let entry = ClipEntry::Text { content: long };
        let display = entry.display_text();
        assert!(display.ends_with("..."));
        // Should not panic, and boundary should be on a char boundary
        assert!(display.is_char_boundary(display.len() - 3));
    }

    #[test]
    fn display_image_shows_mime() {
        let entry = image("shot.png");
        assert_eq!(entry.display_text(), "[image/png] shot.png");
    }

    #[test]
    fn mixed_entries() {
        let h = ClipHistory::new(5);
        h.push(text("hello"));
        h.push(image("test.png"));
        h.push(text("world"));
        let items = h.items();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], text("world"));
        assert_eq!(items[1], image("test.png"));
        assert_eq!(items[2], text("hello"));
    }

    #[test]
    fn clear_empties_history() {
        let h = ClipHistory::new(5);
        h.push(text("a"));
        h.push(text("b"));
        h.clear();
        assert!(h.items().is_empty());
    }

    #[test]
    fn search_empty_query_returns_all() {
        let h = ClipHistory::new(5);
        h.push(text("a"));
        h.push(text("b"));
        let results = h.search("");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_no_match_returns_empty() {
        let h = ClipHistory::new(5);
        h.push(text("hello"));
        let results = h.search("zzzzz");
        assert!(results.is_empty());
    }

    #[test]
    fn dedup_image_entries() {
        let h = ClipHistory::new(5);
        h.push(image("a.png"));
        h.push(text("between"));
        h.push(image("a.png"));
        let items = h.items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], image("a.png"));
        assert_eq!(items[1], text("between"));
    }

    #[test]
    fn display_text_multiline_shows_first_line() {
        let entry = ClipEntry::Text {
            content: "first line\nsecond line\nthird line".to_string(),
        };
        assert_eq!(entry.display_text(), "first line");
    }

    #[test]
    fn display_text_empty_content() {
        let entry = ClipEntry::Text {
            content: String::new(),
        };
        assert_eq!(entry.display_text(), "");
    }

    #[test]
    fn matches_image_by_filename() {
        let entry = ClipEntry::Image {
            path: PathBuf::from("/cache/screenshot_2024.png"),
            mime: "image/png".to_string(),
        };
        assert!(entry.matches("screenshot"));
        assert!(entry.matches("2024"));
        assert!(!entry.matches("jpeg"));
    }

    #[test]
    fn load_from_empty_vec() {
        let h = ClipHistory::load_from(Vec::new(), 10);
        assert!(h.items().is_empty());
    }

    #[test]
    fn push_to_full_history_evicts_oldest() {
        let h = ClipHistory::new(3);
        h.push(text("a"));
        h.push(text("b"));
        h.push(text("c"));
        h.push(text("d"));
        let items = h.items();
        assert_eq!(items, vec![text("d"), text("c"), text("b")]);
    }

    #[test]
    fn search_preserves_newest_first_order() {
        let h = ClipHistory::new(10);
        h.push(text("match old"));
        h.push(text("no hit"));
        h.push(text("match new"));
        let results = h.search("match");
        assert_eq!(results, vec![text("match new"), text("match old")]);
    }

    #[test]
    fn serde_roundtrip_text() {
        let entry = text("hello world");
        let json = serde_json::to_string(&entry).unwrap();
        let loaded: ClipEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, entry);
        // Verify tagged format
        assert!(json.contains(r#""kind":"Text""#));
    }

    #[test]
    fn serde_roundtrip_image() {
        let entry = image("test.png");
        let json = serde_json::to_string(&entry).unwrap();
        let loaded: ClipEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, entry);
        assert!(json.contains(r#""kind":"Image""#));
    }

    #[test]
    fn set_max_trims_excess() {
        let h = ClipHistory::new(5);
        h.push(text("a"));
        h.push(text("b"));
        h.push(text("c"));
        h.set_max(2);
        let items = h.items();
        assert_eq!(items.len(), 2);
        assert_eq!(items, vec![text("c"), text("b")]);
    }

    #[test]
    fn set_max_shared_across_clones() {
        let h = ClipHistory::new(10);
        h.push(text("a"));
        h.push(text("b"));
        h.push(text("c"));

        let h2 = h.clone();
        h.set_max(2);

        // Both clones should see the trimmed result
        assert_eq!(h2.items().len(), 2);
        // Pushing via the clone should also respect the new max
        h2.push(text("d"));
        assert_eq!(h.items().len(), 2);
        assert_eq!(h.items(), vec![text("d"), text("c")]);
    }
}
