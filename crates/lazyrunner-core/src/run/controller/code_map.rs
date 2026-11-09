//! The code map (#89), read in the background as the Code tab opens, and again a moment after a save while it's
//! open: each file only when it changed (the cache kept in the state folder), then Nx's implicit dependencies from
//! its own project graph - the map shows as soon as the files are read, the implicit links join it when Nx is done.

use std::time::{Duration, Instant};

use super::Controller;
use crate::run::code::{self, CodeMap, FileCache};

/// How long after the last save it's read again: a formatter pass or a checkout is one read.
const SETTLE: Duration = Duration::from_secs(2);
/// How long one ask keeps it fresh: the Code tab asks each frame it's open.
const WANTED_FOR: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct CodeMapJob {
    map: Option<CodeMap>,
    /// The files read, between reads.
    cache: Option<FileCache>,
    running: bool,
    due: Option<Instant>,
    wanted_until: Option<Instant>,
    /// Asking Nx for its implicit dependencies.
    asking_nx: bool,
}

impl Controller {
    /// The Code tab is open (or the agent asks): read the code, and again after saves, for a while.
    pub fn want_code(&mut self) {
        let now = (self.now)();
        self.code_map.wanted_until = Some(now + WANTED_FOR);
        if self.code_map.map.is_none() && self.code_map.due.is_none() {
            self.code_map.due = Some(now);
        }
    }

    /// The code map, once read.
    pub fn code_map(&self) -> Option<&CodeMap> {
        self.code_map.map.as_ref()
    }

    /// Whether the code is being read.
    pub fn reading_code(&self) -> bool {
        self.code_map.running
    }

    /// Whether Nx is being asked for its implicit dependencies.
    pub fn asking_nx(&self) -> bool {
        self.code_map.asking_nx
    }

    /// The files read, as kept: in the state folder until this session has read them.
    fn take_cache(&mut self) -> FileCache {
        self.code_map.cache.take().unwrap_or_else(|| FileCache::load(&self.state_dir))
    }

    /// Reads the code now, waiting for it (the agent asks); Nx's implicit dependencies aren't asked for.
    pub fn read_code_now(&mut self) {
        let mut cache = self.take_cache();
        let map = code::scan(&self.root, &self.apps, &self.project.layers, &mut cache);
        cache.save(&self.state_dir);
        self.code_arrived(map, Some(cache), true);
    }

    /// For tests: the code map, as if read.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_code_map(&mut self, map: CodeMap) {
        self.code_arrived(map, None, true);
    }

    /// A save: the map is read again in a quiet moment (once there is one).
    pub(super) fn code_soon(&mut self, now: Instant) {
        if self.code_map.map.is_some() {
            self.code_map.due = Some(now + SETTLE);
        }
    }

    /// Reads it in the background when it's due, while it's wanted: the files, then what Nx adds.
    pub(super) fn tick_code(&mut self, now: Instant) {
        let wanted = self.code_map.wanted_until.is_some_and(|until| now < until);
        if self.code_map.running || !wanted || !self.code_map.due.is_some_and(|due| now >= due) {
            return;
        }
        (self.code_map.running, self.code_map.asking_nx, self.code_map.due) = (true, true, None);
        let mut cache = self.take_cache();
        let (root, apps, layers) = (self.root.clone(), self.apps.clone(), self.project.layers.clone());
        let (state, events) = (self.state_dir.clone(), self.events.clone());
        tokio::task::spawn_blocking(move || {
            let mut map = code::scan(&root, &apps, &layers, &mut cache);
            cache.save(&state);
            let read =
                crate::run::Event::Code { map: Box::new(map.clone()), cache: Some(Box::new(cache)), done: false };
            let _ = events.send(read);
            code::nx_links(&root, &apps, &mut map);
            let _ = events.send(crate::run::Event::Code { map: Box::new(map), cache: None, done: true });
        });
    }

    /// The map read; `done` once Nx has been asked too (until then it's still being read).
    pub(super) fn code_arrived(&mut self, map: CodeMap, cache: Option<FileCache>, done: bool) {
        self.code_map.map = Some(map);
        if cache.is_some() {
            self.code_map.cache = cache;
        }
        self.code_map.running = !done;
        self.code_map.asking_nx = !done;
    }
}
