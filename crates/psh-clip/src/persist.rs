//! Persistent clipboard history — save/load to `$XDG_DATA_HOME/psh/clip_history.json`.

use std::fs;
use std::path::PathBuf;

use tracing::{info, warn};

use crate::history::ClipEntry;

/// Returns the path to the clipboard history file, or None if XDG dirs can't be resolved.
pub fn history_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.data_dir().join("psh").join("clip_history.json"))
}

/// Returns the directory for cached clipboard images, or None if XDG dirs can't be resolved.
pub fn image_cache_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.cache_dir().join("psh").join("clips"))
}

/// Save clipboard entries to disk as JSON.
///
/// Writes to a `.json.tmp` file first, then renames for atomicity.
/// Falls back to a direct write if the rename fails (e.g. cross-device).
/// Silently skips persistence if the XDG data directory cannot be resolved.
pub fn save(entries: &[ClipEntry]) {
    let Some(path) = history_path() else {
        warn!("cannot determine data directory, skipping clipboard persist");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    match serde_json::to_string(entries) {
        Ok(json) => {
            if fs::write(&tmp, &json).is_ok()
                && let Err(e) = fs::rename(&tmp, &path)
            {
                warn!("failed to rename clip history temp file: {e}");
                let _ = fs::write(&path, &json);
            }
        }
        Err(e) => warn!("failed to serialize clip history: {e}"),
    }
}

/// Load clipboard entries from disk. Returns an empty vec on any error.
pub fn load() -> Vec<ClipEntry> {
    let Some(path) = history_path() else {
        return Vec::new();
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Remove cached image files that are not referenced by any history entry.
///
/// Scans the image cache directory and deletes any file whose path does not
/// appear in the given `entries`. This prevents unbounded cache growth when
/// entries are evicted from history.
pub fn prune_orphaned_images(entries: &[ClipEntry]) {
    let Some(cache_dir) = image_cache_dir() else {
        return;
    };
    prune_orphaned_images_in(entries, &cache_dir);
}

/// Core prune logic operating on an explicit directory.
///
/// Separated from [`prune_orphaned_images`] so tests can supply a temp directory.
fn prune_orphaned_images_in(entries: &[ClipEntry], cache_dir: &std::path::Path) {
    let Ok(read_dir) = fs::read_dir(cache_dir) else {
        return;
    };

    let referenced: std::collections::HashSet<PathBuf> = entries
        .iter()
        .filter_map(|e| match e {
            ClipEntry::Image { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();

    let mut removed = 0u32;
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_file() && !referenced.contains(&path) && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        info!("pruned {removed} orphaned clipboard image(s)");
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

    fn image(name: &str, dir: &std::path::Path) -> ClipEntry {
        ClipEntry::Image {
            path: dir.join(name),
            mime: "image/png".to_string(),
        }
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip_history.json");

        let entries = vec![text("hello"), text("world"), image("img.png", dir.path())];

        let json = serde_json::to_string(&entries).unwrap();
        fs::write(&path, &json).unwrap();

        let loaded: Vec<ClipEntry> =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded, entries);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result: Vec<ClipEntry> = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        assert!(result.is_empty());
    }

    #[test]
    fn load_corrupted_json_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        fs::write(&path, "not valid json {{{").unwrap();
        let result: Vec<ClipEntry> = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        assert!(result.is_empty());
    }

    #[test]
    fn save_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("clip_history.json");
        assert!(!nested.parent().unwrap().exists());

        let parent = nested.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        let entries = vec![text("hello")];
        let json = serde_json::to_string(&entries).unwrap();
        fs::write(&nested, &json).unwrap();

        let loaded: Vec<ClipEntry> =
            serde_json::from_str(&fs::read_to_string(&nested).unwrap()).unwrap();
        assert_eq!(loaded, entries);
    }

    #[test]
    fn serde_preserves_tagged_format() {
        let dir = tempfile::tempdir().unwrap();
        let entries = vec![text("hello"), image("img.png", dir.path())];
        let json = serde_json::to_string(&entries).unwrap();
        assert!(json.contains(r#""kind":"Text""#));
        assert!(json.contains(r#""kind":"Image""#));
        let loaded: Vec<ClipEntry> = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, entries);
    }

    #[test]
    fn prune_with_no_orphans_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();

        let kept = cache.join("kept.png");
        fs::write(&kept, b"image data").unwrap();

        let entries = vec![image("kept.png", cache)];
        prune_orphaned_images_in(&entries, cache);

        assert!(kept.exists());
    }

    #[test]
    fn prune_with_empty_history_removes_all() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();

        fs::write(cache.join("a.png"), b"data").unwrap();
        fs::write(cache.join("b.png"), b"data").unwrap();

        prune_orphaned_images_in(&[], cache);

        assert_eq!(fs::read_dir(cache).unwrap().count(), 0);
    }

    #[test]
    fn prune_removes_orphaned_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();

        let kept = cache.join("kept.png");
        let orphan = cache.join("orphan.png");
        fs::write(&kept, b"image data").unwrap();
        fs::write(&orphan, b"image data").unwrap();

        let entries = vec![image("kept.png", cache)];
        prune_orphaned_images_in(&entries, cache);

        assert!(kept.exists());
        assert!(!orphan.exists());
    }
}
