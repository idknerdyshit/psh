use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct ClipHistory {
    inner: Arc<Mutex<VecDeque<String>>>,
    max: usize,
}

impl ClipHistory {
    pub fn new(max: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(max))),
            max,
        }
    }

    pub fn push(&self, item: String) {
        let mut history = self.inner.lock().unwrap();
        // Deduplicate: remove if already present
        history.retain(|i| i != &item);
        if history.len() >= self.max {
            history.pop_back();
        }
        history.push_front(item);
    }

    pub fn items(&self) -> Vec<String> {
        let history = self.inner.lock().unwrap();
        history.iter().cloned().collect()
    }

    pub fn clear(&self) {
        let mut history = self.inner.lock().unwrap();
        history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_retrieve() {
        let h = ClipHistory::new(5);
        h.push("first".into());
        h.push("second".into());
        let items = h.items();
        assert_eq!(items, vec!["second", "first"]);
    }

    #[test]
    fn deduplicates() {
        let h = ClipHistory::new(5);
        h.push("a".into());
        h.push("b".into());
        h.push("a".into());
        let items = h.items();
        assert_eq!(items, vec!["a", "b"]);
    }

    #[test]
    fn respects_max() {
        let h = ClipHistory::new(2);
        h.push("a".into());
        h.push("b".into());
        h.push("c".into());
        let items = h.items();
        assert_eq!(items.len(), 2);
        assert_eq!(items, vec!["c", "b"]);
    }
}
