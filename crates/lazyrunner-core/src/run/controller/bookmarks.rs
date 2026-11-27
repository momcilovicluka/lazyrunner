//! Bookmarks (#84): a marked line kept, with a name and a note, in the state folder; after a restart it finds its line
//! again - the line is marked again - when its app prints one of the same shape.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;

use super::Controller;
use crate::output::Hue;
use crate::run::bookmarks::{self, AROUND, Bookmark};

impl Controller {
    /// The bookmarks kept, oldest first.
    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    pub fn bookmark(&self, id: &str) -> Option<&Bookmark> {
        self.bookmarks.iter().find(|bookmark| bookmark.id == id)
    }

    /// Where bookmark `id` has its line this run: its app's index and the line's number.
    pub fn bookmark_line(&self, id: &str) -> Option<(usize, u64)> {
        self.states.iter().enumerate().find_map(|(app, state)| {
            state.bookmarked.iter().find(|(_, of)| *of == id).map(|(number, _)| (app, *number))
        })
    }

    /// How many bookmarks have their line in a column now.
    pub fn bookmarks_attached(&self) -> usize {
        self.states.iter().map(|state| state.bookmarked.len()).sum()
    }

    /// Keeps app `app`'s line `number` as a bookmark named `name` - its mark's note comes along - and marks it, if it
    /// wasn't; its id. None when the line is gone from the buffer.
    pub fn keep_bookmark(&mut self, app: usize, number: u64, name: &str) -> Option<String> {
        let state = self.states.get(app)?;
        let text = |number: u64| state.output.numbered(number, number).next().map(|line| line.text.clone());
        let line = text(number)?;
        let first = state.run_from.max(1);
        let around = AROUND as u64;
        let before = (number.saturating_sub(around)..number).filter(|at| *at >= first).filter_map(text).collect();
        let after = (number + 1..=number + around).filter_map(text).collect();
        let mut id = format!("{:x}", jiff::Timestamp::now().as_nanosecond());
        while self.bookmark(&id).is_some() {
            id.push('0');
        }
        let state = &mut self.states[app];
        let note = state.notes.remove(&number).unwrap_or_default();
        if let Err(at) = state.marks.binary_search(&number) {
            state.marks.insert(at, number);
        }
        state.bookmarked.insert(number, id.clone());
        self.bookmarks.push(Bookmark {
            id: id.clone(),
            name: name.to_string(),
            note,
            app: state.key.clone(),
            line,
            before,
            after,
            kept: crate::time::now_rfc3339(),
            branch: self.branch.clone(),
            found: None,
        });
        self.save_bookmarks();
        Some(id)
    }

    /// Deletes bookmark `id`; its line, when it has one this run, is unmarked. The bookmark deleted.
    pub fn delete_bookmark(&mut self, id: &str) -> Option<Bookmark> {
        let at = self.bookmarks.iter().position(|bookmark| bookmark.id == id)?;
        let deleted = self.bookmarks.remove(at);
        for state in &mut self.states {
            let lines: Vec<u64> =
                state.bookmarked.iter().filter(|(_, of)| *of == id).map(|(number, _)| *number).collect();
            for number in lines {
                state.bookmarked.remove(&number);
                state.marks.retain(|mark| *mark != number);
            }
        }
        self.save_bookmarks();
        Some(deleted)
    }

    /// Bookmark `id`'s note (empty: none).
    pub fn set_bookmark_note(&mut self, id: &str, note: &str) {
        if let Some(bookmark) = self.bookmarks.iter_mut().find(|bookmark| bookmark.id == id) {
            bookmark.note = note.to_string();
            self.save_bookmarks();
        }
    }

    /// Writes the bookmarks as Markdown into the state folder; the file.
    pub fn export_bookmarks(&self) -> Result<PathBuf> {
        let name_of = |key: &str| self.app(key).map_or_else(|| key.to_string(), |app| app.name.clone());
        let file = self.state_dir.join(bookmarks::MARKDOWN);
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::write(&file, bookmarks::markdown(&self.bookmarks, name_of))?;
        Ok(file)
    }

    /// App `index` printed the lines after line `from`: a bookmark of its that hasn't its line this run finds it in
    /// one of the same shape - marked again, the bookmark found now.
    pub(super) fn find_bookmarks(&mut self, index: usize, from: u64, now: Instant) {
        let state = &self.states[index];
        let total = state.output.total();
        if total <= from || self.bookmarks.is_empty() {
            return;
        }
        let waiting: Vec<usize> = (0..self.bookmarks.len())
            .filter(|&at| {
                let bookmark = &self.bookmarks[at];
                bookmark.app == state.key && !state.bookmarked.values().any(|id| *id == bookmark.id)
            })
            .collect();
        if waiting.is_empty() {
            return;
        }
        let mut found = Vec::new();
        for number in from + 1..=total {
            let Some(line) = self.states[index].output.numbered(number, number).next() else { continue };
            let hit = waiting
                .iter()
                .find(|&&at| !found.iter().any(|(of, _)| *of == at) && self.bookmarks[at].finds(&line.text));
            if let Some(&at) = hit {
                found.push((at, number));
            }
        }
        for (at, number) in found {
            self.attach(index, at, number, now);
        }
    }

    /// Bookmark `at` found its line: line `number` of app `index`.
    fn attach(&mut self, index: usize, at: usize, number: u64, now: Instant) {
        let state = &mut self.states[index];
        if let Err(place) = state.marks.binary_search(&number) {
            state.marks.insert(place, number);
        }
        state.bookmarked.insert(number, self.bookmarks[at].id.clone());
        state.found += 1;
        state.found_at = Some((number, now));
        self.bookmarks[at].found = Some(crate::time::now_rfc3339());
        let name = self.apps.get(index).map_or(String::new(), |app| app.name.clone());
        let said = format!(
            "▸ Bookmark \"{}\" found its line again in {name} (' lists the bookmarks)",
            self.bookmarks[at].name
        );
        self.add_log(said, Hue::Cyan);
        self.save_bookmarks();
    }

    fn save_bookmarks(&mut self) {
        if let Err(error) = bookmarks::save(&self.state_dir, &self.bookmarks) {
            self.add_log(format!("✗ Couldn't keep the bookmarks: {error}"), Hue::Red);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::tests::controller;
    use crate::run::{Event, Tag};
    use crate::testing::TempDir;

    fn print(controller: &mut super::Controller, lines: &[&str]) {
        let generation = controller.states[0].generation;
        for line in lines {
            let tag = Tag::App { key: "orders".into(), generation };
            controller.handle(Event::Output { tag, line: (*line).into() });
        }
    }

    #[tokio::test]
    async fn a_bookmark_finds_its_line_again_in_the_next_run_and_after_lazyrunner_opens_again() {
        let temp = TempDir::new("bookmarks-found");
        let (mut controller, _rx) = controller(&temp, json!({}));
        controller.states[0].reset_for_start();
        print(
            &mut controller,
            &[
                "info: Checkout started",
                "09:12:44 warn: Payment 4f2a9c1e timed out on http://localhost:47202 after 30000 ms",
                "      at Shop.Billing.Charge()",
            ],
        );
        let number = controller.states[0].output.total() - 1;
        controller.states[0].notes.insert(number, "the first sign".into());
        let id = controller.keep_bookmark(0, number, "Payment timeout").unwrap();
        let kept = controller.bookmark(&id).unwrap().clone();
        assert_eq!((kept.note.as_str(), kept.branch.as_str()), ("the first sign", controller.branch.as_str()));
        assert_eq!(kept.before, ["info: Checkout started"], "its run's lines only");
        assert_eq!(kept.after, ["      at Shop.Billing.Charge()"]);
        assert!(controller.states[0].notes.is_empty(), "the note went with it");
        assert_eq!(controller.bookmark_line(&id), Some((0, number)), "its own line, marked");
        assert!(controller.states[0].marks.contains(&number));

        controller.states[0].reset_for_start();
        assert_eq!(controller.bookmark_line(&id), None, "a new run: from an earlier run");
        print(
            &mut controller,
            &[
                "info: Application started.",
                "10:01:05 warn: Payment 77bd0e12 timed out on \
                                  http://localhost:51990 after 30000 ms",
            ],
        );
        let again = controller.states[0].output.total();
        assert_eq!(controller.bookmark_line(&id), Some((0, again)), "the same shape, another time, id and port");
        assert!(controller.states[0].marks.contains(&again), "marked again");
        assert_eq!(controller.states[0].found, 1);
        assert!(controller.bookmark(&id).unwrap().found.is_some());
        let said = controller.log.back().unwrap().text.clone();
        assert!(said.starts_with("▸ Bookmark \"Payment timeout\" found its line again in Orders"), "{said}");
        print(&mut controller, &["10:01:09 warn: Payment 1a2b3c4d timed out on http://localhost:51990 after 30000 ms"]);
        assert_eq!(controller.bookmark_line(&id), Some((0, again)), "once a run");

        let (mut reopened, _rx) = super::super::tests::controller(&temp, json!({}));
        assert_eq!(reopened.bookmarks().len(), 1, "kept in the state folder");
        reopened.states[0].reset_for_start();
        print(&mut reopened, &["11:30:00 warn: Payment 00ff00ff timed out on http://localhost:5000 after 30000 ms"]);
        assert!(reopened.bookmark_line(&id).is_some(), "found after lazyrunner opened again");

        let file = reopened.export_bookmarks().unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("## Payment timeout"));
        assert_eq!(reopened.delete_bookmark(&id).map(|gone| gone.name), Some("Payment timeout".into()));
        assert!(reopened.states[0].marks.is_empty() && reopened.states[0].bookmarked.is_empty(), "unmarked");
        assert!(crate::run::bookmarks::load(reopened.state_folder()).is_empty());
    }
}
