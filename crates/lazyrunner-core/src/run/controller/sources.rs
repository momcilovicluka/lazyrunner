//! Database sources in the controller (#114): the team's (the profile's `databaseSources`) and this machine's own
//! (its project's section of lazyrunner.config.json) in one list, yours winning by name; their passwords, one per
//! server and login; a server's databases listed in the background and kept with when; a new source tried before
//! it's saved; a source written where it belongs - into the profile with only its key changed and the profile
//! trusted as written, or into the config; and the links between local databases and their remote twins. Nothing ever
//! run at a source changes it: a check, a listing, an export.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;

use super::Controller;
use crate::output::Hue;
use crate::profile::sources::{self, DatabaseSource, MINE};
use crate::profile::{self, Source};
use crate::run::Event;
use crate::run::catalog::{self, Catalog, Listed};
use crate::run::credentials::{self, Password};
use crate::run::db_links::{self, Link, Remote};
use crate::run::pull::{self, MASTER};

/// The setting that dims a linked database older than so many days, and puts `U` in its hint.
pub const STALE_PULLED: &str = "stalePulled";
/// A day, in seconds.
const DAY_SECONDS: i64 = 24 * 60 * 60;

/// What a listing or a try in the background says when it's done.
#[derive(Debug)]
pub enum News {
    Listed { source: String, result: Result<Vec<Listed>, String> },
    Tried { job: u64, result: Result<Tried, String> },
}

/// A new source tried: who it signed in as, how long that took, and the databases its login may see.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tried {
    pub login: Option<String>,
    pub ms: u128,
    pub databases: Vec<Listed>,
}

/// Where a source is written: the profile (the team's, usually committed) or this machine's config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Team,
    Mine,
}

/// What reads an environment variable for a source's connection string.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

impl Controller {
    /// Every source this machine has: the profile's, then this machine's own (winning by name).
    pub fn sources(&self) -> Vec<DatabaseSource> {
        sources::merged(&self.project.pulling.sources, &self.config[MINE])
    }

    /// A source by its name.
    pub fn source(&self, name: &str) -> Option<DatabaseSource> {
        self.sources().into_iter().find(|source| source.name.eq_ignore_ascii_case(name))
    }

    pub(super) fn known_source(&self, name: &str) -> Result<DatabaseSource, String> {
        self.source(name).ok_or_else(|| {
            let known: Vec<String> = self.sources().into_iter().map(|source| source.name).collect();
            if known.is_empty() {
                "no databaseSources yet - a on the database screen's Sources adds one".to_string()
            } else {
                format!("no source '{name}' - the ones there: {}", known.join(", "))
            }
        })
    }

    // ---------------------------------------------------------------- passwords

    /// Where a source's password is kept now (its server and login's), and where 1.118.0 kept it (its name's).
    fn password_places(&self, source: &DatabaseSource) -> (Option<String>, String) {
        let shared = source.login().map(|login| credentials::shared_target(&self.project.id, &login));
        (shared, credentials::target(&self.project.id, &source.name))
    }

    /// A source's kept password: its server and login's, else one 1.118.0 kept under its name - moved to its server
    /// and login's quietly (the old one forgotten only once the new one reads back).
    pub fn kept_password(&self, source: &DatabaseSource) -> Result<Option<Password>, String> {
        let (shared, legacy) = self.password_places(source);
        let Some(shared) = shared else { return self.credentials.get(&legacy) };
        if let Some(found) = self.credentials.get(&shared)? {
            return Ok(Some(found));
        }
        let Some(old) = self.credentials.get(&legacy)? else { return Ok(None) };
        if let Some(user) = source.login().and_then(|login| login.split('@').next().map(String::from))
            && self.credentials.set(&shared, &user, &old).is_ok()
            && self.credentials.get(&shared).ok().flatten().as_ref() == Some(&old)
        {
            let _ = self.credentials.forget(&legacy);
        }
        Ok(Some(old))
    }

    /// Whether a source's password is kept in the credential store.
    pub fn password_kept(&self, source: &str) -> bool {
        self.source(source).is_some_and(|found| self.kept_password(&found).is_ok_and(|kept| kept.is_some()))
    }

    /// What the credential store is called: `Credential Manager`.
    pub fn credential_store(&self) -> &'static str {
        self.credentials.name()
    }

    /// Keeps a source's password in the credential store (never anywhere else), for every source with its server
    /// and login; where it went.
    pub fn keep_password(&mut self, source: &str, password: &Password) -> Result<&'static str, String> {
        let source = self.known_source(source)?;
        self.keep_password_for(&source, password)
    }

    /// Keeps the password of a source that may not be saved yet (the add form's).
    pub fn keep_password_for(&mut self, source: &DatabaseSource, password: &Password) -> Result<&'static str, String> {
        let (Some(shared), legacy) = self.password_places(source) else {
            return Err(format!("{} logs in without a password lazyrunner keeps", source.name));
        };
        let user = source.login().and_then(|login| login.split('@').next().map(String::from)).unwrap_or_default();
        self.credentials.set(&shared, &user, password)?;
        let _ = self.credentials.forget(&legacy);
        let store = self.credentials.name();
        self.add_log(format!("{}'s password is kept in {store}", source.name), Hue::Gray);
        Ok(store)
    }

    /// Forgets a source's password (its server and login's, and one kept under its name); what it says.
    pub fn forget_password(&mut self, source: &str) -> Result<String, String> {
        match self.source(source) {
            Some(found) => self.forget_password_of(&found),
            None => self.forget_places(source, (None, credentials::target(&self.project.id, source))),
        }
    }

    /// Forgets the password of a source that may be gone from the list already.
    pub fn forget_password_of(&mut self, found: &DatabaseSource) -> Result<String, String> {
        let places = self.password_places(found);
        self.forget_places(&found.name, places)
    }

    /// Whether another source than `name` signs in with `source`'s server and login.
    pub fn password_shared(&self, source: &DatabaseSource) -> bool {
        let login = source.login();
        login.is_some()
            && self
                .sources()
                .iter()
                .any(|other| !other.name.eq_ignore_ascii_case(&source.name) && other.login() == login)
    }

    fn forget_places(&mut self, source: &str, (shared, legacy): (Option<String>, String)) -> Result<String, String> {
        let mut forgot = self.credentials.forget(&legacy)?;
        if let Some(shared) = shared {
            forgot |= self.credentials.forget(&shared)?;
        }
        let said = match forgot {
            true => format!("Forgot {source}'s password - the next pull asks for it"),
            false => format!("No password of {source}'s was kept"),
        };
        self.add_log(said.clone(), Hue::Gray);
        Ok(said)
    }

    /// Why a pull or a listing of `source` can't start without the user: its password isn't kept, and an agent
    /// can't type it.
    pub fn password_missing(&self, source: &str) -> Option<String> {
        let found = self.source(source).filter(DatabaseSource::needs_password)?;
        (!self.password_kept(&found.name)).then(|| {
            format!(
                "{}'s password isn't kept yet - ask the user to pull once from lazyrunner (p on the database screen): \
                 it asks for the password and keeps it in {}",
                found.name,
                self.credentials.name()
            )
        })
    }

    /// The password a source signs in with: the one just typed, else the one kept; none for a login without one.
    pub(super) fn password_for(
        &self,
        source: &DatabaseSource,
        typed: Option<Password>,
    ) -> Result<Option<Password>, String> {
        match typed {
            Some(password) => Ok(Some(password)),
            None if source.needs_password() => {
                let kept = self.kept_password(source)?;
                Ok(Some(kept.ok_or_else(|| self.password_missing(&source.name).unwrap_or_default())?))
            }
            None => Ok(None),
        }
    }

    // ---------------------------------------------------------------- listing

    /// A source's databases as last listed.
    pub fn catalog(&self, source: &str) -> Option<&Catalog> {
        self.catalogs.iter().find(|(name, _)| name.eq_ignore_ascii_case(source)).map(|(_, catalog)| catalog)
    }

    /// Whether a source's databases are being listed now.
    pub fn is_listing(&self, source: &str) -> bool {
        self.listing.iter().any(|name| name.eq_ignore_ascii_case(source))
    }

    /// `g`: lists a source's server's databases again in the background (read only, from `master`).
    pub fn list_databases(&mut self, name: &str, typed: Option<Password>) -> Result<String, String> {
        let source = self.known_source(name)?;
        self.sql.ready()?;
        if self.is_listing(&source.name) {
            return Ok(format!("listing {}'s databases", source.name));
        }
        let password = self.password_for(&source, typed)?;
        let connection = pull::connection_to(&source, Some(MASTER), password.as_ref(), env)?;
        let (sql, events, named) = (Arc::clone(&self.sql), self.events.clone(), source.name.clone());
        std::thread::spawn(move || {
            let result = catalog::list(sql.as_ref(), &connection);
            let _ = events.send(Event::Sources(News::Listed { source: named, result }));
        });
        self.listing.push(source.name.clone());
        Ok(format!("list {}'s databases", source.name))
    }

    /// Every remote database there is to link to: the database sources', then the whole servers' as last listed.
    pub fn remotes(&self) -> Vec<Remote> {
        let mut remotes = Vec::new();
        for source in self.sources() {
            match source.database() {
                Some(database) => remotes.push(Remote { source: source.name.clone(), database: database.into() }),
                None => {
                    let listed =
                        self.catalog(&source.name).map(|catalog| catalog.databases.clone()).unwrap_or_default();
                    remotes
                        .extend(listed.into_iter().map(|db| Remote { source: source.name.clone(), database: db.name }));
                }
            }
        }
        remotes
    }

    // ---------------------------------------------------------------- trying a new source

    /// `g` in the add form: signs in to a source not saved yet (the check step of a pull) and lists its databases,
    /// in the background; the try's job, its answer in `tried` once it's there.
    pub fn try_source(&mut self, draft: &DatabaseSource, typed: Option<Password>) -> Result<u64, String> {
        self.sql.ready()?;
        let password = match typed {
            Some(password) => Some(password),
            None if draft.needs_password() => Some(self.kept_password(draft)?.ok_or_else(|| {
                format!("type {}'s password first - it's kept in {}", draft.name, self.credentials.name())
            })?),
            None => None,
        };
        let connection = pull::connection_to(draft, Some(MASTER), password.as_ref(), env)?;
        let job = self.new_db_job();
        let (sql, events) = (Arc::clone(&self.sql), self.events.clone());
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = super::database_jobs::first(sql.as_ref(), &connection, Some(MASTER), pull::CHECK_SQL)
                .and_then(|login| {
                    let ms = started.elapsed().as_millis();
                    Ok(Tried { login, ms, databases: catalog::list(sql.as_ref(), &connection)? })
                })
                .map_err(|why| crate::run::secrets::mask_text(&why));
            let _ = events.send(Event::Sources(News::Tried { job, result }));
        });
        self.tried = None;
        Ok(job)
    }

    /// A listing or a try is done.
    pub(super) fn sources_news(&mut self, news: News) {
        match news {
            News::Listed { source, result } => {
                self.listing.retain(|name| !name.eq_ignore_ascii_case(&source));
                match result {
                    Ok(databases) => {
                        let count = databases.len();
                        self.catalogs.insert(source.clone(), Catalog { at: crate::time::now_rfc3339(), databases });
                        if let Err(error) = catalog::save(&self.state_dir, &self.catalogs) {
                            self.add_log(format!("! Couldn't keep {source}'s list ({error})"), Hue::Yellow);
                        }
                        self.add_log(format!("Listed {source}'s databases: {count}"), Hue::Gray);
                    }
                    Err(why) => self.add_log(
                        format!("✗ Couldn't list {source}'s databases: {}", why.lines().next().unwrap_or_default()),
                        Hue::Red,
                    ),
                }
            }
            News::Tried { job, result } => self.tried = Some((job, result)),
        }
    }

    // ---------------------------------------------------------------- saving

    /// The profile file a team source is written to: the repo's or your own - never one bundled with lazyrunner.
    pub fn team_file(&self) -> Result<PathBuf, String> {
        match self.project.source {
            Source::Bundled => Err(format!(
                "{} comes with lazyrunner - save the source for only you, or add a {} to the repo",
                self.project.name,
                profile::REPO_PROFILE_FILE
            )),
            _ => Ok(self.project.path.clone()),
        }
    }

    /// Saves `drafts` (one, or one per database picked) for the team or only you, in place of `replacing` (`E`) when
    /// it's given; a password typed is kept for their server and login. What it says it did.
    pub fn save_sources(
        &mut self,
        drafts: &[DatabaseSource],
        place: Place,
        replacing: Option<&str>,
        password: Option<&Password>,
    ) -> Result<String, String> {
        let Some(first) = drafts.first() else { return Err("nothing to save".into()) };
        let listed: Vec<Value> = drafts.iter().map(DatabaseSource::to_json).collect();
        if let Some(problem) = sources::check_list(&Value::Array(listed)).into_iter().next() {
            return Err(problem.replace("databaseSources[0] ", "").replace("databaseSources", "the source"));
        }
        let others: Vec<DatabaseSource> = self
            .sources()
            .into_iter()
            .filter(|known| replacing.is_none_or(|old| !known.name.eq_ignore_ascii_case(old)))
            .collect();
        if let Some(taken) =
            drafts.iter().find(|draft| others.iter().any(|known| known.name.eq_ignore_ascii_case(&draft.name)))
        {
            return Err(format!("there's a source named {} already", taken.name));
        }
        let old = replacing.and_then(|name| self.source(name));
        if let Some(old) = old.as_ref().filter(|old| old.mine != (place == Place::Mine)) {
            self.remove_from(old.mine, &old.name)?;
        }
        let in_place = old.as_ref().filter(|old| old.mine == (place == Place::Mine)).map(|old| old.name.clone());
        let mut first_done = false;
        for draft in drafts {
            let replacing = if first_done { None } else { in_place.as_deref() };
            match place {
                Place::Team => self.write_team(Some(draft), replacing)?,
                Place::Mine => self.write_mine(Some(draft), replacing)?,
            }
            first_done = true;
        }
        if let Some(password) = password.filter(|_| first.needs_password()) {
            self.keep_password_for(first, password)?;
        }
        let names: Vec<&str> = drafts.iter().map(|draft| draft.name.as_str()).collect();
        let whose = match place {
            Place::Team => self.project.path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into()),
            Place::Mine => "only mine".into(),
        };
        let said = format!("Saved {} ({whose})", names.join(", "));
        self.add_log(said.clone(), Hue::Cyan);
        Ok(said)
    }

    /// `Del` on a source: taken out of the list it's in - nothing on its server is touched. What it says.
    pub fn remove_source(&mut self, name: &str) -> Result<String, String> {
        let source = self.known_source(name)?;
        self.remove_from(source.mine, &source.name)?;
        let from = if source.mine { "this machine's sources" } else { "the profile" };
        let said = format!("Removed {} from {from} - nothing on its server was touched", source.name);
        self.add_log(said.clone(), Hue::Gray);
        Ok(said)
    }

    fn remove_from(&mut self, mine: bool, name: &str) -> Result<(), String> {
        if mine {
            return self.write_mine(None, Some(name));
        }
        self.write_team(None, Some(name))
    }

    /// `+` on a database under a server: it becomes a database source of its own (`<server source>-<database>`),
    /// saved where the server source is. What it says.
    pub fn keep_as_source(&mut self, server: &str, database: &str) -> Result<String, String> {
        let source = self.known_source(server)?;
        let taken: Vec<String> = self.sources().into_iter().map(|known| known.name).collect();
        let wanted: String = format!("{}-{}", source.name, database.to_lowercase())
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect();
        let draft = DatabaseSource { name: sources::free(&wanted, &taken), ..source.on(database) };
        let place = if source.mine { Place::Mine } else { Place::Team };
        self.save_sources(std::slice::from_ref(&draft), place, None, None)
    }

    /// This machine's sources with `draft` in place of `replacing` (or added), or `replacing` taken out.
    fn write_mine(&mut self, draft: Option<&DatabaseSource>, replacing: Option<&str>) -> Result<(), String> {
        let mut listed: Vec<Value> =
            self.config_file.own(&self.project.id, MINE).and_then(|mine| mine.as_array().cloned()).unwrap_or_default();
        let at = replacing
            .and_then(|old| listed.iter().position(|item| crate::json::text(&item["name"]).eq_ignore_ascii_case(old)));
        match (at, draft) {
            (Some(at), Some(draft)) => listed[at] = draft.to_json(),
            (Some(at), None) => {
                listed.remove(at);
            }
            (None, Some(draft)) => listed.push(draft.to_json()),
            (None, None) => return Err(format!("no source '{}' among this machine's", replacing.unwrap_or_default())),
        }
        let value = (!listed.is_empty()).then_some(Value::Array(listed));
        self.config = self.config_file.set_own(&self.project.id, MINE, value).map_err(|error| error.to_string())?;
        Ok(())
    }

    /// The profile's sources with `draft` in place of `replacing` (or added), or `replacing` taken out: only that
    /// item of `databaseSources` written, the rest of the file as it was; the profile read again and trusted as
    /// written (when it was trusted before).
    fn write_team(&mut self, draft: Option<&DatabaseSource>, replacing: Option<&str>) -> Result<(), String> {
        let path = self.team_file()?;
        let text = crate::json::read_text(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let raw = std::fs::read(&path).map_err(|error| error.to_string())?;
        let text = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) { format!("\u{feff}{text}") } else { text };
        let data: Value = serde_json::from_str(crate::json::strip_bom(&text)).map_err(|error| error.to_string())?;
        let key = ["databaseSources"];
        let written = match data.get(key[0]).and_then(Value::as_array) {
            Some(items) => {
                let at = replacing.and_then(|old| {
                    items.iter().position(|item| crate::json::text(&item["name"]).eq_ignore_ascii_case(old))
                });
                match (at, draft) {
                    (Some(at), Some(draft)) => crate::json_edit::replace_item(&text, &key, at, &draft.to_json()),
                    (Some(at), None) => crate::json_edit::remove_item(&text, &key, at),
                    (None, Some(draft)) => crate::json_edit::push_item(&text, &key, &draft.to_json()),
                    (None, None) => Err(format!("no source '{}' in the profile", replacing.unwrap_or_default())),
                }
            }
            None => {
                let mut all: Vec<Value> = self
                    .project
                    .pulling
                    .sources
                    .iter()
                    .filter(|known| replacing.is_none_or(|old| !known.name.eq_ignore_ascii_case(old)))
                    .map(DatabaseSource::to_json)
                    .collect();
                all.extend(draft.map(DatabaseSource::to_json));
                crate::json_edit::set_member(&text, &[], key[0], &Value::Array(all))
            }
        }?;
        let mut problems = Vec::new();
        let reread: Value =
            serde_json::from_str(crate::json::strip_bom(&written)).map_err(|error| error.to_string())?;
        sources::check(&reread, &mut problems);
        if let Some(problem) = problems.into_iter().next() {
            return Err(problem);
        }
        let store = self.paths.trust_store();
        let trusted = profile::trust::is_trusted(&self.project, &store);
        crate::config::write_text(&path, &written).map_err(|error| error.to_string())?;
        self.project.pulling.sources = sources::to_sources(&reread["databaseSources"]);
        self.project.file_hash = profile::load::text_hash(crate::json::strip_bom(&written));
        let found = profile::Found { path: path.clone(), source: self.project.source, root: self.project.root.clone() };
        if let Ok(now) = profile::resolve(&found, &self.paths.profiles) {
            self.project.hash = now.hash;
            self.project.file_hash = now.file_hash;
        }
        if trusted && self.project.source == Source::Repo {
            profile::trust::approve(&self.project, &store).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- links

    /// The link of `database` on `server`, when it has one.
    pub fn link_of(&self, server: &str, database: &str) -> Option<&Link> {
        self.db_links.iter().find(|link| link.of(server, database))
    }

    /// The remote databases in the order `database`'s twin is suggested.
    pub fn suggest_twins(&self, database: &str) -> Vec<Remote> {
        db_links::suggest(database, &self.remotes())
    }

    /// Links `database` on `server` to `remote` at `source` (one it had is replaced): kept on this machine. What it
    /// says.
    pub fn link_database(
        &mut self,
        (server, database): (&str, &str),
        (source, remote): (&str, &str),
        pulled: Option<String>,
    ) -> Result<String, String> {
        let source = self.known_source(source)?;
        self.db_links.retain(|link| !link.of(server, database));
        let link = Link {
            server: server.into(),
            database: database.into(),
            source: source.name.clone(),
            remote: remote.into(),
            pulled,
        };
        let twin = link.twin();
        self.db_links.push(link);
        self.keep_links();
        let said = format!("Linked {database} @ {server} ↔ {twin} - U updates it from there");
        self.add_log(said.clone(), Hue::Cyan);
        Ok(said)
    }

    /// Unlinks `database` on `server`. What it says.
    pub fn unlink_database(&mut self, server: &str, database: &str) -> Result<String, String> {
        let link = self.link_of(server, database).cloned().ok_or_else(|| format!("{database} isn't linked"))?;
        self.db_links.retain(|known| !known.of(server, database));
        self.keep_links();
        let said = format!("Unlinked {database} from {}", link.twin());
        self.add_log(said.clone(), Hue::Gray);
        Ok(said)
    }

    pub(super) fn keep_links(&mut self) {
        if let Err(error) = db_links::save(&self.state_dir, &self.db_links) {
            self.add_log(format!("! Couldn't keep the links ({error})"), Hue::Yellow);
        }
    }

    /// Whether a linked database's twin changed after the copy was taken, where its server's last list says so.
    pub fn twin_changed(&self, link: &Link) -> bool {
        let changed = self.catalog(&link.source).and_then(|catalog| catalog.database(&link.remote)?.changed.clone());
        let parse = |at: &str| at.parse::<jiff::Timestamp>().ok();
        matches!((changed.as_deref().and_then(parse), link.pulled.as_deref().and_then(parse)), (Some(changed), Some(pulled)) if changed > pulled)
    }

    /// `stalePulled`: how many days old a linked copy may be before it's dimmed, when it's set.
    pub fn stale_days(&self) -> Option<u64> {
        self.config[STALE_PULLED].as_u64().filter(|days| *days > 0)
    }

    /// Whether a linked copy is older than `stalePulled` says.
    pub fn is_stale(&self, link: &Link) -> bool {
        let Some(days) = self.stale_days() else { return false };
        let Some(pulled) = link.pulled.as_deref().and_then(|at| at.parse::<jiff::Timestamp>().ok()) else {
            return false;
        };
        jiff::Timestamp::now().as_second() - pulled.as_second() > days as i64 * DAY_SECONDS
    }

    /// For the tests: a source of this machine's added as it is.
    #[cfg(test)]
    pub(super) fn write_mine_for_tests(&mut self, source: &DatabaseSource) {
        self.write_mine(Some(source), None).expect("written");
    }

    /// For the tests: a try's answer as if it came from the server.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pretend_tried(&mut self, job: u64, result: Result<Tried, String>) {
        self.sources_news(News::Tried { job, result });
    }

    /// The connection string to `server` alone, from any of its connection strings: for a database that has none of
    /// its own (an update's old copy).
    pub(super) fn server_connection(&self, server: &str) -> Result<String, String> {
        let entry =
            self.entry_on(server, None).ok_or_else(|| format!("no connection string to {server} in secrets.json"))?;
        let connection = self.connection(&entry.server, &entry.database)?;
        Ok(crate::run::sqlpackage::with_database(&connection, MASTER))
    }
}
