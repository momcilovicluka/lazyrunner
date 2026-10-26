//! Who makes lazyrunner, for the About box: contributors.json, which a GitHub Action keeps in step with the repo (who
//! wrote code, who reported bugs and ideas). The copy built in is the fallback; next to the update check lazyrunner
//! fetches the current one into its state folder, at most once a day, and keeps the one it has when that fails.

use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::config::Toggle;
use crate::update::{self, Source};

/// The list as it was when this version was built.
pub const BUILT_IN: &str = include_str!("../../../contributors.json");
const FILE: &str = "contributors.json";
/// A list kept this long is fetched again.
const REFRESH_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// The list is a few KB; more is not contributors.json.
const MAX_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Contributors {
    /// The day the list last changed (YYYY-MM-DD).
    pub updated: String,
    pub people: Vec<Person>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Person {
    pub login: String,
    pub name: String,
    /// What they are to lazyrunner, when the maintainer says ("creator · maintainer").
    pub role: String,
    pub commits: u32,
    pub prs: u32,
    pub bugs: u32,
    pub ideas: u32,
    pub issues: u32,
}

impl Person {
    /// Wrote code for it, or has a role in it (the rest reported bugs and ideas).
    pub fn builds(&self) -> bool {
        !self.role.is_empty() || self.prs > 0 || self.commits > 0
    }

    /// What they did, in a few words: their role, or their pull requests and issues.
    pub fn summary(&self) -> String {
        if !self.role.is_empty() {
            return self.role.clone();
        }
        let count = |count: u32, one: &str, many: &str| match count {
            0 => None,
            1 => Some(format!("1 {one}")),
            count => Some(format!("{count} {many}")),
        };
        let other = self.issues.saturating_sub(self.bugs + self.ideas);
        let parts: Vec<String> = [
            count(self.prs, "PR", "PRs"),
            count(self.bugs, "bug report", "bug reports"),
            count(self.ideas, "idea", "ideas"),
            count(other, "issue", "issues"),
        ]
        .into_iter()
        .flatten()
        .collect();
        parts.join(" · ")
    }
}

fn parse(text: &str) -> Option<Contributors> {
    serde_json::from_str::<Contributors>(text).ok().filter(|list| !list.people.is_empty())
}

pub fn built_in() -> Contributors {
    parse(BUILT_IN).unwrap_or_default()
}

/// The newest list there is: the one fetched last, or the one built in when that is newer.
pub fn load(state_dir: &Path) -> Contributors {
    let built_in = built_in();
    match fs::read_to_string(state_dir.join(FILE)).ok().and_then(|text| parse(&text)) {
        Some(fetched) if fetched.updated >= built_in.updated => fetched,
        _ => built_in,
    }
}

/// Where the release repo keeps contributors.json: raw.githubusercontent.com for a GitHub repo, `<repo>/contributors.json`
/// elsewhere (the tests' local stand-in).
pub fn url(source: &Source) -> String {
    match source.url.strip_prefix("https://github.com/") {
        Some(repo) => format!("https://raw.githubusercontent.com/{repo}/main/{FILE}"),
        None => format!("{}/{FILE}", source.url),
    }
}

/// Fetches the current list when the one kept is a day old or missing, as the update check runs (not when updates
/// are off, or there is no release repo). A failed fetch keeps what there is.
pub fn refresh(config: &Value, state_dir: &Path) {
    if !Toggle::AutoUpdate.read(config) {
        return;
    }
    let Some(source) = update::source(config) else { return };
    let path = state_dir.join(FILE);
    let age = fs::metadata(&path).and_then(|meta| meta.modified()).ok().and_then(|at| at.elapsed().ok());
    if age.is_some_and(|age| age < REFRESH_AFTER) {
        return;
    }
    let Some(text) = update::fetch_text(&url(&source), &source, MAX_BYTES) else { return };
    if parse(&text).is_none() {
        return;
    }
    let partial = state_dir.join(format!("{FILE}.part"));
    if fs::create_dir_all(state_dir).is_ok() && fs::write(&partial, text).is_ok() {
        let _ = fs::rename(&partial, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn person(login: &str) -> Person {
        Person { login: login.into(), name: login.into(), ..Person::default() }
    }

    #[test]
    fn the_built_in_list_has_the_maintainer() {
        let list = built_in();
        assert!(!list.updated.is_empty());
        assert!(list.people.iter().any(|one| one.role.contains("maintainer")), "{list:?}");
    }

    #[test]
    fn a_summary_is_the_role_or_what_they_did() {
        let maintainer = Person { role: "creator · maintainer".into(), commits: 90, ..person("a") };
        assert_eq!(maintainer.summary(), "creator · maintainer");
        let reporter = Person { prs: 1, bugs: 2, ideas: 1, issues: 4, ..person("b") };
        assert_eq!(reporter.summary(), "1 PR · 2 bug reports · 1 idea · 1 issue");
        assert!(reporter.builds());
        assert!(!Person { ideas: 3, issues: 3, ..person("c") }.builds());
    }

    #[test]
    fn the_newer_list_wins_and_a_broken_one_is_ignored() {
        let temp = TempDir::new("contributors-load");
        let dir = temp.join("state");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir), built_in(), "nothing fetched yet");
        fs::write(dir.join(FILE), r#"{"updated":"2999-01-01","people":[{"login":"x","name":"X"}]}"#).unwrap();
        assert_eq!(load(&dir).people[0].login, "x");
        fs::write(dir.join(FILE), r#"{"updated":"2000-01-01","people":[{"login":"x","name":"X"}]}"#).unwrap();
        assert_eq!(load(&dir), built_in(), "older than the one built in");
        fs::write(dir.join(FILE), "not json").unwrap();
        assert_eq!(load(&dir), built_in());
    }

    /// A release repo that answers `answers` requests for contributors.json, then stops listening.
    fn serve(answers: usize) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(answers) {
                let mut stream = stream.unwrap();
                let mut request = [0; 2048];
                let _ = stream.read(&mut request);
                let body = r#"{"updated":"2999-01-01","people":[{"login":"new","name":"New One","prs":1}]}"#;
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(format!("{head}{body}").as_bytes());
            }
        });
        format!("http://{address}/o/r")
    }

    #[test]
    fn refresh_fetches_the_list_once_a_day_and_not_with_updates_off() {
        let temp = TempDir::new("contributors-refresh");
        let dir = temp.join("state");
        let repo = serve(1);
        refresh(&serde_json::json!({ "updateRepoUrl": repo, "autoUpdate": false }), &dir);
        assert_eq!(load(&dir), built_in(), "updates off: nothing fetched");
        refresh(&serde_json::json!({ "updateRepoUrl": repo, "autoUpdate": true }), &dir);
        assert_eq!(load(&dir).people[0].name, "New One");
        // The server is gone: a second fetch today would find nothing, and the list stays.
        refresh(&serde_json::json!({ "updateRepoUrl": repo, "autoUpdate": true }), &dir);
        assert_eq!(load(&dir).people[0].name, "New One");
        assert!(!dir.join("contributors.json.part").exists());
    }

    #[test]
    fn a_github_repo_keeps_it_on_raw_githubusercontent() {
        let github = Source { url: "https://github.com/o/r".into() };
        assert_eq!(url(&github), "https://raw.githubusercontent.com/o/r/main/contributors.json");
        let local = Source { url: "http://127.0.0.1:1234/o/r".into() };
        assert_eq!(url(&local), "http://127.0.0.1:1234/o/r/contributors.json");
    }
}
