use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock},
};

#[derive(Clone, Default)]
pub struct DrainList {
    inner: Arc<RwLock<BTreeSet<String>>>,
}

impl DrainList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let set: BTreeSet<String> = entries.into_iter().map(Into::into).collect();
        Self {
            inner: Arc::new(RwLock::new(set)),
        }
    }

    pub fn drain(&self, key: &str) -> bool {
        match self.inner.write() {
            Ok(mut set) => set.insert(key.to_string()),
            Err(_) => false,
        }
    }

    pub fn undrain(&self, key: &str) -> bool {
        match self.inner.write() {
            Ok(mut set) => set.remove(key),
            Err(_) => false,
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        match self.inner.read() {
            Ok(set) => set.contains(key),
            Err(_) => false,
        }
    }

    pub fn entries(&self) -> Vec<String> {
        match self.inner.read() {
            Ok(set) => set.iter().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_is_idempotent() {
        let list = DrainList::new();
        assert!(list.drain("10.0.0.11:8080"));
        assert!(!list.drain("10.0.0.11:8080"));
        assert!(list.contains("10.0.0.11:8080"));
    }

    #[test]
    fn undrain_reports_whether_anything_changed() {
        let list = DrainList::seed(["10.0.0.11:8080"]);
        assert!(list.undrain("10.0.0.11:8080"));
        assert!(!list.undrain("10.0.0.11:8080"));
        assert!(!list.contains("10.0.0.11:8080"));
    }

    #[test]
    fn entries_are_sorted_for_stable_output() {
        let list = DrainList::seed(["10.0.0.12:8080", "10.0.0.11:8080"]);
        assert_eq!(list.entries(), ["10.0.0.11:8080", "10.0.0.12:8080"]);
    }

    #[test]
    fn clones_share_one_set() {
        let list = DrainList::new();
        let clone = list.clone();
        clone.drain("10.0.0.11:8080");
        assert!(list.contains("10.0.0.11:8080"));
    }
}
