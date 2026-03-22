//! Frecency (frequency + recency) tracking for application launches.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Tracks how frequently and recently each application has been launched.
/// Persists to a JSON file at `$XDG_DATA_HOME/psh/launch_history.json`.
#[derive(Debug, Clone)]
pub struct FrecencyTracker {
    entries: HashMap<String, FrecencyEntry>,
    path: PathBuf,
}

/// Per-application launch statistics stored in the history file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FrecencyEntry {
    /// Total number of times this application has been launched.
    count: u32,
    /// Unix timestamp of the most recent launch.
    last_used: u64,
}

impl FrecencyTracker {
    /// Load launch history from disk, returning an empty tracker on any error.
    pub fn load() -> Self {
        let path = Self::history_path();
        let entries = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { entries, path }
    }

    /// Record a launch event for the given exec command.
    pub fn record(&mut self, exec: &str) {
        let now = now_secs();
        let entry = self
            .entries
            .entry(exec.to_string())
            .or_insert(FrecencyEntry {
                count: 0,
                last_used: now,
            });
        entry.count += 1;
        entry.last_used = now;
        self.save();
    }

    /// Compute a frecency score for the given exec command.
    /// Returns 0.0 if the app has never been launched.
    /// Use `score_at` when scoring many entries to avoid repeated syscalls.
    #[cfg(test)]
    pub fn score(&self, exec: &str) -> f64 {
        self.score_at(exec, now_secs())
    }

    /// Compute a frecency score using a pre-captured timestamp.
    pub fn score_at(&self, exec: &str, now: u64) -> f64 {
        let Some(entry) = self.entries.get(exec) else {
            return 0.0;
        };
        let age_secs = now.saturating_sub(entry.last_used);
        f64::from(entry.count) * recency_weight(age_secs)
    }

    /// Write current state to disk.
    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(&self.entries) {
            let _ = fs::write(&self.path, json);
        }
    }

    /// Returns the path to the launch history file.
    fn history_path() -> PathBuf {
        directories::BaseDirs::new()
            .map(|dirs| dirs.data_dir().join("psh").join("launch_history.json"))
            .or_else(|| {
                std::env::var("XDG_RUNTIME_DIR")
                    .ok()
                    .map(|d| PathBuf::from(d).join("psh_launch_history.json"))
            })
            .expect("cannot determine XDG_DATA_HOME or XDG_RUNTIME_DIR for launch history")
    }
}

/// Returns the current time as seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Compute a recency weight based on how long ago the app was last used.
fn recency_weight(age_secs: u64) -> f64 {
    const HOUR: u64 = 3600;
    const DAY: u64 = 86400;
    const WEEK: u64 = 604800;

    if age_secs < HOUR {
        10.0
    } else if age_secs < DAY {
        5.0
    } else if age_secs < WEEK {
        2.0
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tracker_scores_zero() {
        let tracker = FrecencyTracker {
            entries: HashMap::new(),
            path: PathBuf::from("/dev/null"),
        };
        assert_eq!(tracker.score("firefox"), 0.0);
    }

    #[test]
    fn record_increases_score() {
        let mut tracker = FrecencyTracker {
            entries: HashMap::new(),
            path: PathBuf::from("/dev/null"),
        };
        tracker.record("firefox");
        assert!(tracker.score("firefox") > 0.0);
    }

    #[test]
    fn recency_weight_decays() {
        assert!(recency_weight(0) > recency_weight(3601));
        assert!(recency_weight(3601) > recency_weight(86401));
        assert!(recency_weight(86401) > recency_weight(604801));
    }

    #[test]
    fn score_at_uses_provided_timestamp() {
        let mut tracker = FrecencyTracker {
            entries: HashMap::new(),
            path: PathBuf::from("/dev/null"),
        };
        tracker.record("firefox");
        let launch_time = now_secs();

        // Score right after launch (within the hour) should use weight 10.0.
        let score_now = tracker.score_at("firefox", launch_time);
        // Score a week later should use weight 1.0.
        let score_later = tracker.score_at("firefox", launch_time + 604_801);
        assert!(score_now > score_later);
    }

    #[test]
    fn multiple_records_increase_score() {
        let mut tracker = FrecencyTracker {
            entries: HashMap::new(),
            path: PathBuf::from("/dev/null"),
        };
        tracker.record("firefox");
        let score1 = tracker.score("firefox");
        tracker.record("firefox");
        let score2 = tracker.score("firefox");
        assert!(score2 > score1);
    }
}
