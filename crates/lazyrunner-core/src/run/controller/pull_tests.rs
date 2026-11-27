//! A pull through the controller (#110), with SQL, SqlPackage and the credential store as stand-ins: the password
//! from the store or refused without it, the steps in order, the switch, `keepPulled`'s offer, and no password in
//! anything it says.

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;

use super::Controller;
use super::tests::controller;
use crate::profile::sources::{Auth, DatabaseSource, Pulling, Reach};
use crate::run::credentials::{self, Memory, Password, Store};
use crate::run::pull::{Pulled, State};
use crate::run::sql::Sql;
use crate::run::sqlpackage::{Action, Package};
use crate::run::{Event, secrets};
use crate::testing::TempDir;

pub(super) const SECRET: &str = "s3cret-Pa55";

/// SQL that answers a pull's queries and keeps what it was asked, and the connection strings it got; it fails what
/// names `fail`.
#[derive(Default)]
pub(super) struct Scripted {
    pub(super) asked: Mutex<Vec<String>>,
    pub(super) fail: Mutex<Option<&'static str>>,
}

impl Sql for Scripted {
    fn run(&self, connection: &str, _: Option<&str>, sql: &str, line: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.asked.lock().unwrap().push(format!("{connection} | {sql}"));
        if self.fail.lock().unwrap().is_some_and(|fail| sql.contains(fail)) {
            return Err(format!("Msg 5030: {sql} failed"));
        }
        if sql == crate::run::pull::CHECK_SQL {
            line("reader");
        } else if sql.contains("ISNULL(DB_ID(") {
            line("0");
        } else if sql.contains("COUNT(*) FROM sys.tables") {
            line("1");
        } else if sql == crate::run::catalog::LIST_SQL {
            line("120|2026-01-12T09:00:00+01:00|Shop");
            line("-|2026-03-02T10:00:00+01:00|Shop_Audit");
        }
        Ok(())
    }

    fn ready(&self) -> Result<(), String> {
        Ok(())
    }
}

struct Packaged;

impl Package for Packaged {
    fn run(&self, action: Action, _: &str, file: &str, _: &Path, line: &mut dyn FnMut(&str)) -> Result<(), String> {
        line("Processing Table '[dbo].[Orders]'.");
        if action == Action::Export {
            std::fs::write(file, "bacpac").map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn ready(&self) -> Result<(), String> {
        Ok(())
    }
}

/// A controller with a source `qa` (a SQL login), a script after the pull, its secrets.json and stand-ins.
pub(super) fn pulling(
    temp: &TempDir,
    keep: Option<usize>,
) -> (Controller, UnboundedReceiver<Event>, Arc<Scripted>, Arc<Memory>) {
    let (mut controller, rx) = controller(temp, json!({}));
    let reach = Reach::Server {
        server: "qa-sql.example.com,1433".into(),
        database: Some("Shop".into()),
        auth: Auth::Sql,
        user: Some("reader".into()),
    };
    controller.project.pulling = Pulling {
        sources: vec![DatabaseSource::new("qa", reach)],
        after_pull: vec!["scripts/local.sql".into()],
        keep_pulled: keep,
        migrate: false,
    };
    std::fs::create_dir_all(controller.root.join("scripts")).unwrap();
    std::fs::write(controller.root.join("scripts").join("local.sql"), "UPDATE Settings SET Url = 'http://localhost';")
        .unwrap();
    let secrets = temp.join("secrets.json");
    std::fs::write(
        &secrets,
        "{\n  \"DefaultConnection\": \"Server=localhost;Database=Shop;Trusted_Connection=True\"\n}\n",
    )
    .unwrap();
    let sql = Arc::new(Scripted::default());
    controller.use_sql(sql.clone(), Arc::new(Packaged), secrets);
    let store = Arc::new(Memory::default());
    controller.use_credentials(store.clone());
    (controller, rx, sql, store)
}

/// The pull's news handled until its thread is done, then the controller's own steps.
pub(super) async fn pulled(rx: &mut UnboundedReceiver<Event>, controller: &mut Controller) {
    loop {
        match rx.recv().await.unwrap() {
            Event::Pull { job, news: news @ crate::run::pull::News::Ended(_) } => {
                controller.pull_news(job, news);
                break;
            }
            Event::Pull { job, news } => controller.pull_news(job, news),
            _ => {}
        }
    }
    controller.tick_pull();
}

pub(super) fn said(controller: &Controller) -> String {
    controller.log.iter().map(|entry| entry.text.clone()).collect::<Vec<_>>().join("\n")
}

#[tokio::test]
async fn without_a_kept_password_a_pull_is_refused_and_says_to_pull_once_from_lazyrunner() {
    let temp = TempDir::new("pull-refused");
    let (mut controller, _rx, _, _) = pulling(&temp, None);
    let why = controller.pull_database("qa", None).unwrap_err();
    assert!(why.contains("ask the user to pull once from lazyrunner") && why.contains("the tests' store"), "{why}");
    assert!(controller.pull_database("nowhere", None).unwrap_err().contains("the ones there: qa"));
    assert!(controller.pull.is_none());
}

#[tokio::test]
async fn a_pull_runs_its_steps_switches_to_the_copy_and_never_says_the_password() {
    let temp = TempDir::new("pull-all");
    let (mut controller, mut rx, sql, store) = pulling(&temp, None);
    controller.keep_password("qa", &Password::new(SECRET)).unwrap();
    assert!(controller.password_kept("qa"));
    let target = credentials::shared_target(&controller.project.id, "reader@qa-sql.example.com,1433");
    assert_eq!(
        store.get(&target).unwrap().unwrap().expose(),
        SECRET,
        "kept under lazyrunner/<project>/<login>@<server>"
    );
    let doing = controller.pull_database("qa", None).unwrap();
    let today = jiff::ZonedDateTime::now().strftime("%Y%m%d").to_string();
    let database = format!("Shop_qa_{today}");
    assert!(doing.contains(&database) && !doing.contains(SECRET), "{doing}");
    assert!(controller.database_busy().is_some_and(|busy| busy.starts_with("pulling a copy of qa")));
    assert!(controller.backup_database("localhost", "Shop").is_err(), "one database job at a time");
    pulled(&mut rx, &mut controller).await;
    let pull = controller.pull.as_ref().unwrap();
    let states: Vec<State> = pull.steps().iter().map(|step| pull.step(*step).state).collect();
    assert_eq!(states, [State::Done, State::Done, State::Done, State::Done, State::Skipped, State::Done]);
    assert!(pull.went_well(), "{}", said(&controller));
    let asked = sql.asked.lock().unwrap().clone();
    let at_source: Vec<&String> = asked.iter().filter(|asked| asked.contains("qa-sql.example.com")).collect();
    assert!(at_source.iter().all(|asked| asked.contains("SELECT")), "only reads at the source: {at_source:?}");
    assert!(asked.iter().any(|asked| asked.starts_with("Server=localhost") && asked.contains("UPDATE Settings")));
    let text = std::fs::read_to_string(temp.join("secrets.json")).unwrap();
    let active = secrets::active_label(&text).unwrap();
    assert_eq!(active, format!("{database} @ localhost"), "{text}");
    assert!(!text.contains(SECRET));
    let log = said(&controller);
    assert!(log.contains("✓ Pulled qa in") && log.contains("o opens the folder"), "{log}");
    let link = controller.link_of("localhost", &database).expect("a pull links what it made");
    assert_eq!((link.twin(), link.pulled.is_some()), ("qa/Shop".to_string(), true));
    assert!(
        !log.contains(SECRET) && !format!("{:?}", controller.pull.as_ref().map(|pull| &pull.steps)).contains(SECRET)
    );
    let kept = crate::run::pull::load(&controller.state_dir);
    assert_eq!(kept.iter().map(|pull| pull.database.as_str()).collect::<Vec<_>>(), [database.as_str()]);
    assert_eq!(controller.last_pull_source().as_deref(), Some("qa"));
    assert!(controller.forget_password("qa").unwrap().starts_with("Forgot qa's password"));
    assert!(!controller.password_kept("qa"));
}

#[tokio::test]
async fn keep_pulled_offers_the_older_copies_and_a_pull_never_writes_to_the_server_in_use_as_a_source() {
    let temp = TempDir::new("pull-keep");
    let (mut controller, mut rx, _, _) = pulling(&temp, Some(1));
    let older = Pulled {
        source: "qa".into(),
        server: "localhost".into(),
        database: "Shop_qa_20261101".into(),
        at: "2026-11-01T09:00:00Z".into(),
        remote: None,
    };
    controller.pulled = vec![older.clone()];
    controller.pull_database("qa", Some(Password::new(SECRET))).unwrap();
    pulled(&mut rx, &mut controller).await;
    assert_eq!(controller.pull_stale, [older], "{}", said(&controller));
    assert!(said(&controller).contains("keepPulled: 1 older copy (Shop_qa_20261101)"));

    controller.project.pulling.sources[0].reach =
        Reach::Server { server: "LOCALHOST".into(), database: Some("Shop".into()), auth: Auth::Windows, user: None };
    let why = controller.pull_database("qa", None).unwrap_err();
    assert!(why.contains("is the server in use"), "{why}");
}

#[tokio::test]
async fn a_cancelled_pull_says_where_it_stopped() {
    let temp = TempDir::new("pull-cancel");
    let (mut controller, mut rx, _, _) = pulling(&temp, None);
    controller.pull_database("qa", Some(Password::new(SECRET))).unwrap();
    assert!(controller.cancel_pull().is_ok());
    pulled(&mut rx, &mut controller).await;
    let pull = controller.pull.as_ref().unwrap();
    assert!(pull.cancelled && pull.is_over() && !pull.went_well());
    let log = said(&controller);
    assert!(log.contains("■ The pull from qa was cancelled at ") && !log.contains("✓ Pulled"), "{log}");
    assert!(controller.cancel_pull().is_err(), "nothing runs now");
}
