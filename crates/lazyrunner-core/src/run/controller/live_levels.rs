//! Live log levels (#112): a .NET app's log category at another level for a while, through its user secrets - no
//! restart. Each goes back by itself: when its time is up, when the app stops, exits or starts again, and when
//! lazyrunner quits; what a crash left is put back when lazyrunner opens next.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::Controller;
use crate::output::Hue;
use crate::profile::{App, AppKind};
use crate::run::live_level::{self, Change, Saved};
use crate::run::secrets;

/// How soon putting one back is tried again when its file couldn't be written.
const RETRY: Duration = Duration::from_secs(60);
const MINUTE: u64 = 60;
const TIME_UP: &str = "time's up";
const LEFT_OVER: &str = "left from before lazyrunner closed";

/// A log category of an app at another level until `until`.
#[derive(Clone, Debug)]
pub struct LiveLevel {
    pub app: String,
    pub category: String,
    pub level: String,
    pub minutes: u64,
    pub until: Instant,
    changes: Vec<Change>,
}

impl LiveLevel {
    /// The minutes it has left at `now`, rounded up.
    pub fn minutes_left(&self, now: Instant) -> u64 {
        self.until.saturating_duration_since(now).as_secs().div_ceil(MINUTE)
    }

    fn file(&self) -> Option<&PathBuf> {
        self.changes.first().map(|change| &change.path)
    }
}

/// What setting one did: what the log says, whether Serilog's setting was written too, whether the user secrets
/// file was made for it, and the other apps that read the same file.
#[derive(Clone, Debug)]
pub struct LiveSet {
    pub said: String,
    pub serilog: bool,
    pub made: bool,
    pub shared: Vec<String>,
}

/// `name` owning something: "Orders'", "Shop's".
fn owner(name: &str) -> String {
    if name.ends_with('s') { format!("{name}'") } else { format!("{name}'s") }
}

impl Controller {
    /// The live log levels, oldest first.
    pub fn live_levels(&self) -> &[LiveLevel] {
        &self.live_levels
    }

    /// Puts app `key`'s `category` at `level` (one of [`live_level::LEVELS`]) for `minutes`, without a restart; again
    /// for one that's live, the time starting over. Refused, saying why, for an app that isn't a running .NET app with
    /// user secrets, or one whose command line sets that category (it wins over its user secrets).
    pub fn set_live_level(&mut self, key: &str, category: &str, level: &str, minutes: u64) -> Result<LiveSet, String> {
        let app = self.app(key).cloned().ok_or_else(|| format!("no app '{key}'"))?;
        let name = app.name.clone();
        if !matches!(app.kind, AppKind::Dotnet { .. }) {
            return Err(format!("{name} isn't a .NET app: a live log level goes through a .NET app's user secrets"));
        }
        if !self.is_running(key) {
            return Err(format!("{name} isn't running - start it first (a live log level ends when it starts again)"));
        }
        if self.on_command_line(key, category) {
            return Err(format!(
                "{name} gets {category}'s level on its command line, which wins over its user secrets - set it with \
                 Alt+l instead (that restarts it), or pick a narrower category"
            ));
        }
        let path = self.app_secrets(&app).ok_or_else(|| {
            format!(
                "{name} keeps no user secrets - `dotnet user-secrets init` in its project gives it some (it reads \
                 them from its next start)"
            )
        })?;
        let made = !path.exists();
        let serilog = live_level::uses_serilog(&self.root, &app);
        let mut changes = Vec::new();
        for (setting, value) in live_level::settings(category, level, serilog) {
            match live_level::write(&path, &setting, &value) {
                Ok(change) => changes.push(change),
                Err(error) => {
                    for change in &changes {
                        let _ = live_level::undo(change);
                    }
                    return Err(format!("{} user secrets couldn't be written: {error}", owner(&name)));
                }
            }
        }
        let same = |live: &LiveLevel| live.file() == Some(&path) && live.category.eq_ignore_ascii_case(category);
        if let Some(at) = self.live_levels.iter().position(same) {
            let old = self.live_levels.remove(at);
            for change in &mut changes {
                if let Some(first) = old.changes.iter().find(|first| first.key.eq_ignore_ascii_case(&change.key)) {
                    change.before = first.before.clone();
                }
            }
        }
        let shared = self
            .apps
            .iter()
            .filter(|other| other.key != key && other.is_dotnet())
            .filter(|other| self.app_secrets(other).as_ref() == Some(&path))
            .map(|other| other.name.clone())
            .collect();
        let until = (self.now)() + Duration::from_secs(minutes * MINUTE);
        let live =
            LiveLevel { app: key.into(), category: category.into(), level: level.into(), minutes, until, changes };
        self.live_levels.push(live);
        self.save_live_levels();
        let said = format!("{name} logs {category} at {level} for {minutes} min");
        Ok(LiveSet { said, serilog, made, shared })
    }

    /// Puts app `key`'s live levels back now: `category`'s, or every one. How many there were.
    pub fn reset_live_levels(&mut self, key: &str, category: Option<&str>) -> usize {
        let picked = |live: &LiveLevel| {
            live.app == key && category.is_none_or(|category| live.category.eq_ignore_ascii_case(category))
        };
        let count = self.live_levels.iter().filter(|live| picked(live)).count();
        self.end_live_levels(picked, "put back now");
        count
    }

    /// Those whose time is up go back.
    pub(super) fn tick_live_levels(&mut self, now: Instant) {
        if self.live_levels.iter().any(|live| now >= live.until) {
            self.end_live_levels(|live| now >= live.until, TIME_UP);
        }
    }

    /// App `key`'s go back: it stopped, exited or starts again (`why`).
    pub(super) fn end_app_live_levels(&mut self, key: &str, why: &str) {
        if self.live_levels.iter().any(|live| live.app == key) {
            self.end_live_levels(|live| live.app == key, why);
        }
    }

    /// Every one goes back: lazyrunner quits.
    pub(super) fn end_every_live_level(&mut self) {
        if !self.live_levels.is_empty() {
            self.end_live_levels(|_| true, "lazyrunner quits");
        }
    }

    /// What a lazyrunner that closed without putting them back left (it crashed): put back now.
    pub(super) fn put_back_left_over(&mut self) {
        let now = (self.now)();
        let left = live_level::load(&self.state_dir).into_iter().map(|saved: Saved| LiveLevel {
            app: saved.app,
            category: saved.category,
            level: saved.level,
            minutes: 0,
            until: now,
            changes: saved.changes,
        });
        self.live_levels.extend(left);
        if !self.live_levels.is_empty() {
            self.end_live_levels(|_| true, LEFT_OVER);
        }
    }

    /// The ones `ending` picks go back, each said in the log; one whose file couldn't be written is tried again in
    /// a minute (and kept in the record meanwhile).
    fn end_live_levels(&mut self, ending: impl Fn(&LiveLevel) -> bool, why: &str) {
        let (ended, kept): (Vec<LiveLevel>, Vec<LiveLevel>) =
            std::mem::take(&mut self.live_levels).into_iter().partition(|live| ending(live));
        self.live_levels = kept;
        let now = (self.now)();
        for live in ended {
            let whose = owner(&self.app_title(&live.app));
            let undone: Vec<anyhow::Result<bool>> = live.changes.iter().map(live_level::undo).collect();
            if let Some(Err(error)) = undone.iter().find(|undone| undone.is_err()) {
                let text =
                    format!("✗ {whose} {} couldn't be put back ({error}) - tried again in a minute", live.category);
                self.add_log(text, Hue::Red);
                self.live_levels.push(LiveLevel { until: now + RETRY, ..live });
            } else if undone.iter().any(|undone| matches!(undone, Ok(false))) {
                let text =
                    format!("{whose} {} was changed in its user secrets meanwhile - left as it is", live.category);
                self.add_log(text, Hue::Yellow);
            } else {
                self.add_log(format!("{whose} log levels are back: {} ({why})", live.category), Hue::Cyan);
            }
        }
        self.save_live_levels();
    }

    fn save_live_levels(&mut self) {
        let saved: Vec<Saved> = self
            .live_levels
            .iter()
            .map(|live| Saved {
                app: live.app.clone(),
                category: live.category.clone(),
                level: live.level.clone(),
                changes: live.changes.clone(),
            })
            .collect();
        if let Err(error) = live_level::save(&self.state_dir, &saved) {
            self.add_log(format!("✗ Couldn't record the live log levels: {error}"), Hue::Red);
        }
    }

    /// The user secrets app `app` reads (the tests' file instead, when they set one).
    fn app_secrets(&self, app: &App) -> Option<PathBuf> {
        self.secrets_at.clone().or_else(|| secrets::app_secrets_path(&self.root, app))
    }

    /// Whether app `key`'s command line or environment sets `category`'s level: lazyrunner's own arguments, a level
    /// set with Alt+l, the profile's arguments, an override on the start sheet.
    fn on_command_line(&self, key: &str, category: &str) -> bool {
        let Ok(command) = self.server_command(key) else { return false };
        let category = category.to_lowercase();
        let (argument, variable) =
            (format!("--logging:loglevel:{category}="), format!("logging__loglevel__{category}"));
        let in_arguments = command
            .spec
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().to_lowercase())
            .any(|arg| arg.starts_with(&argument) || arg.starts_with(&format!("{variable}=")));
        in_arguments || command.spec.env.iter().any(|(name, _)| name.to_string_lossy().eq_ignore_ascii_case(&variable))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::run::app::Status;
    use crate::testing::{TempDir, clock};

    type Receiver = tokio::sync::mpsc::UnboundedReceiver<crate::run::Event>;

    const SECRETS: &str = "{\n  \"DefaultConnection\": \"Server=localhost;Database=Shop\",\n  \"Logging\": {\n    \"LogLevel\": { \"Default\": \"Warning\" }\n  }\n}\n";
    const EF: &str = "Microsoft.EntityFrameworkCore.Database.Command";

    /// The sample with Orders running on the test's user secrets, and the clock in the test's hands.
    fn running(temp: &TempDir) -> (Controller, PathBuf, Receiver) {
        let (mut controller, rx) = super::super::tests::controller(temp, json!({}));
        let secrets = temp.join("secrets.json");
        std::fs::write(&secrets, SECRETS).unwrap();
        controller.secrets_at = Some(secrets.clone());
        controller.now = clock::now;
        controller.states[0].status = Status::Running;
        (controller, secrets, rx)
    }

    fn last_log(controller: &Controller) -> String {
        controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default()
    }

    fn read(path: &PathBuf) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[tokio::test]
    async fn several_live_levels_last_their_time_and_go_back_one_by_one() {
        let temp = TempDir::new("live-levels");
        let (mut controller, secrets, _rx) = running(&temp);
        let set = controller.set_live_level("orders", EF, "Debug", 10).unwrap();
        assert_eq!(set.said, format!("Orders logs {EF} at Debug for 10 min"));
        assert_eq!((set.serilog, set.made, set.shared.as_slice()), (false, false, ["Billing".to_string()].as_slice()));
        controller.set_live_level("orders", "Default", "Trace", 30).unwrap();
        let both = secrets::flatten(&read(&secrets));
        assert!(both.contains(&(format!("Logging:LogLevel:{EF}"), "Debug".into())), "{both:?}");
        assert!(both.contains(&("Logging:LogLevel:Default".into(), "Trace".into())), "{both:?}");
        assert_eq!(controller.live_levels()[0].minutes_left(clock::now()), 10);

        clock::advance(Duration::from_secs(5 * 60));
        controller.set_live_level("orders", EF, "Trace", 10).unwrap();
        assert_eq!(controller.live_levels().len(), 2, "set again, it's the same one");
        clock::advance(Duration::from_secs(9 * 60));
        controller.tick_live_levels(clock::now());
        assert_eq!(controller.live_levels().len(), 2, "its time started over");
        clock::advance(Duration::from_secs(60));
        controller.tick_live_levels(clock::now());
        assert_eq!(last_log(&controller), format!("Orders' log levels are back: {EF} (time's up)"));
        assert!(!read(&secrets).contains(EF), "taken out again, not left at Debug");
        controller.reset_live_levels("orders", None);
        assert_eq!(read(&secrets), SECRETS, "back as it was");
        assert!(live_level::load(controller.state_folder()).is_empty(), "nothing left to record");
    }

    #[tokio::test]
    async fn they_go_back_before_the_app_starts_again_and_when_lazyrunner_quits() {
        let temp = TempDir::new("live-levels-restart");
        let (mut controller, secrets, _rx) = running(&temp);
        controller.set_live_level("orders", EF, "Debug", 10).unwrap();
        controller.end_app_live_levels("billing", "it starts again");
        assert_eq!(controller.live_levels().len(), 1, "another app's start leaves it");
        controller.end_app_live_levels("orders", "it starts again");
        assert_eq!(last_log(&controller), format!("Orders' log levels are back: {EF} (it starts again)"));
        assert_eq!(read(&secrets), SECRETS);

        controller.set_live_level("orders", "Default", "Debug", 10).unwrap();
        controller.shutdown();
        assert_eq!(read(&secrets), SECRETS);
        assert!(controller.live_levels().is_empty());
    }

    #[tokio::test]
    async fn what_a_crash_left_goes_back_when_lazyrunner_opens_next() {
        let temp = TempDir::new("live-levels-crash");
        let (mut controller, secrets, _rx) = running(&temp);
        controller.set_live_level("orders", "Default", "Debug", 10).unwrap();
        assert_eq!(live_level::load(controller.state_folder()).len(), 1, "recorded while it lasts");
        drop(controller);
        let (reopened, _rx) = super::super::tests::controller(&temp, json!({}));
        assert_eq!(read(&secrets), SECRETS);
        assert!(
            reopened
                .log
                .iter()
                .any(|entry| entry.text == "Orders' log levels are back: Default (left from before lazyrunner closed)")
        );
        assert!(live_level::load(reopened.state_folder()).is_empty());
    }

    #[tokio::test]
    async fn an_app_it_cant_reach_is_refused_saying_why() {
        let temp = TempDir::new("live-levels-refused");
        let (mut controller, secrets, _rx) = running(&temp);
        let stopped = controller.set_live_level("billing", "Default", "Debug", 10).unwrap_err();
        assert!(stopped.starts_with("Billing isn't running"), "{stopped}");
        let npm = controller.set_live_level("shop", "Default", "Debug", 10).unwrap_err();
        assert!(npm.starts_with("Shop isn't a .NET app"), "{npm}");
        let own = controller.set_live_level("orders", "system.net.http.httpclient", "Debug", 10).unwrap_err();
        assert!(own.contains("on its command line"), "{own}");
        controller.states[0].status = Status::Stopped;
        controller.set_log_level("orders", EF, Some("Warning"));
        controller.states[0].status = Status::Running;
        let levelled = controller.set_live_level("orders", EF, "Debug", 10).unwrap_err();
        assert!(levelled.contains("Alt+l"), "{levelled}");
        controller.secrets_at = None;
        let none = controller.set_live_level("orders", "Orders", "Debug", 10).unwrap_err();
        assert!(none.contains("keeps no user secrets"), "{none}");
        assert_eq!(read(&secrets), SECRETS, "nothing written");
    }
}
