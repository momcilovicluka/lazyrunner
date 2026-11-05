//! Background work run several at a time - the audit view's scans, the migrations view's checks: how many at once (a
//! setting `+` and `-` change in those views), and what failed beside other work, tried once more on its own - a
//! package feed or a database may refuse a burst of requests it would answer one at a time.

use std::collections::{HashSet, VecDeque};

use serde_json::Value;

/// How many at once: at least one, at most `MOST`, `USUAL` when the setting is missing.
pub const MOST: usize = 8;
const USUAL: usize = 3;

/// A setting's number of things at once.
pub fn at_once(value: &Value) -> usize {
    value.as_u64().map_or(USUAL, |count| (count as usize).clamp(1, MOST))
}

/// What ran beside other work, what was tried again already, and what waits to be tried again on its own.
#[derive(Debug, Default)]
pub struct Retry {
    crowded: HashSet<String>,
    tried: HashSet<String>,
    waiting: VecDeque<String>,
}

impl Retry {
    /// A new batch: whatever fails in it may be tried again.
    pub fn reset(&mut self) {
        self.tried.clear();
    }

    /// These run together now: when there are several, each ran beside other work.
    pub fn together<'a>(&mut self, running: impl IntoIterator<Item = &'a str>) {
        let running: Vec<&str> = running.into_iter().collect();
        if running.len() > 1 {
            self.crowded.extend(running.into_iter().map(str::to_string));
        }
    }

    /// `key` failed: true when it's to be tried again on its own - it ran beside other work, and it wasn't tried again
    /// yet. Then it waits for [`Retry::next`] or [`Retry::take`].
    pub fn again(&mut self, key: &str) -> bool {
        let crowded = self.crowded.remove(key);
        if crowded && self.tried.insert(key.to_string()) {
            self.waiting.push_back(key.to_string());
            return true;
        }
        false
    }

    /// `key` is done: it worked, or failed for good.
    pub fn done(&mut self, key: &str) {
        self.crowded.remove(key);
    }

    /// Whether `key` waits to be tried again on its own.
    pub fn waits(&self, key: &str) -> bool {
        self.waiting.iter().any(|waiting| waiting == key)
    }

    /// `key` is tried again now.
    pub fn take(&mut self, key: &str) {
        self.waiting.retain(|waiting| waiting != key);
    }

    /// The next to be tried again, the first that failed first.
    pub fn next(&mut self) -> Option<String> {
        self.waiting.pop_front()
    }

    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    /// Everything stops: nothing waits.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_setting_says_how_many_at_once_within_bounds() {
        assert_eq!(at_once(&Value::Null), 3);
        assert_eq!(at_once(&json!(1)), 1);
        assert_eq!(at_once(&json!(0)), 1, "never none");
        assert_eq!(at_once(&json!(50)), MOST);
        assert_eq!(at_once(&json!("lots")), 3);
    }

    #[test]
    fn what_failed_beside_others_is_tried_once_more_on_its_own() {
        let mut retry = Retry::default();
        retry.together(["a"]);
        assert!(!retry.again("a"), "it ran alone: it failed for good");
        retry.together(["a", "b"]);
        assert!(retry.again("a") && retry.waits("a") && retry.len() == 1);
        retry.done("b");
        assert!(!retry.again("b"), "b worked; a later failure of it ran alone");
        assert_eq!(retry.next().as_deref(), Some("a"));
        retry.together(["a", "c"]);
        assert!(!retry.again("a"), "tried again once already");
        retry.reset();
        retry.together(["a", "c"]);
        assert!(retry.again("a"), "a new batch tries it again");
        retry.take("a");
        assert!(!retry.waits("a"));
    }
}
