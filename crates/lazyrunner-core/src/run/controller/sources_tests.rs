//! Database sources through the controller (#114), with SQL, SqlPackage and the credential store as stand-ins: the
//! team's and this machine's sources, a source written into the profile with only its key changed and trusted as
//! written, one password per server and login (a 1.118.0 one moved quietly), a server's databases listed and kept, a
//! new source tried, links, and an update swapping its new copy in - or putting everything back.

use std::time::Duration;

use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;

use super::Controller;
use super::pull_tests::{SECRET, pulled, pulling, said};
use super::sources::{News, Place};
use crate::profile::sources::{Auth, DatabaseSource, Reach};
use crate::profile::{Source, trust};
use crate::run::Event;
use crate::run::credentials::{self, Password, Store};
use crate::run::pull::{Mode, State, Step};
use crate::testing::TempDir;

const PROFILE: &str = "{\r\n    \"id\": \"sample\",\r\n    \"name\": \"Sample\",\r\n    \"apps\": [],\r\n    \
                       \"databaseSources\": [\r\n        { \"name\": \"qa\", \"server\": \"qa-sql.example.com,1433\", \
                       \"database\": \"Shop\", \"auth\": \"sql\", \"user\": \"reader\" }\r\n    ]\r\n}\r\n";

fn whole(name: &str) -> DatabaseSource {
    let reach = Reach::Server {
        server: "qa-sql.example.com,1433".into(),
        database: None,
        auth: Auth::Sql,
        user: Some("reader".into()),
    };
    DatabaseSource::new(name, reach)
}

/// The controller's profile made a repo's own file in the temp folder, trusted as it is.
fn own_profile(temp: &TempDir, controller: &mut Controller, text: &str) -> std::path::PathBuf {
    let path = temp.join(".lazyrunner.json");
    std::fs::write(&path, text).unwrap();
    controller.project.path = path.clone();
    controller.project.source = Source::Repo;
    controller.project.file_hash = crate::profile::load::text_hash(text);
    trust::approve(&controller.project, &controller.paths().trust_store()).unwrap();
    path
}

/// Events handled until `done` says so (or a few seconds pass).
async fn until(rx: &mut UnboundedReceiver<Event>, controller: &mut Controller, done: impl Fn(&Controller) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !done(controller) {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(event)) => controller.handle(event),
            _ => panic!("timed out: {}", said(controller)),
        }
    }
}

#[tokio::test]
async fn a_team_source_is_written_into_the_profile_alone_and_the_profile_stays_trusted() {
    let temp = TempDir::new("sources-team");
    let (mut controller, _rx, _, _) = pulling(&temp, None);
    let path = own_profile(&temp, &mut controller, PROFILE);
    let said = controller.save_sources(&[whole("qa-all")], Place::Team, None, None).unwrap();
    assert!(said.starts_with("Saved qa-all (.lazyrunner.json)"), "{said}");
    let written = std::fs::read_to_string(&path).unwrap();
    let added = "\r\n        { \"name\": \"qa-all\", \"server\": \"qa-sql.example.com,1433\", \"auth\": \"sql\", \
                 \"user\": \"reader\" }\r\n";
    assert_eq!(written, PROFILE.replace("\"reader\" }\r\n", &format!("\"reader\" }},{added}")), "only the item added");
    assert!(trust::is_trusted(&controller.project, &controller.paths().trust_store()), "its own edit needs no trust");
    assert!(controller.source("qa-all").is_some_and(|source| source.is_server() && !source.mine));
    let mut outside = controller.project.clone();
    outside.file_hash = crate::profile::load::text_hash(&written.replace("\"Sample\"", "\"Sample 2\""));
    assert!(!trust::is_trusted(&outside, &controller.paths().trust_store()), "a change from outside still asks");

    let edited = DatabaseSource { name: "qa-all".into(), ..whole("x") }.on("Shop_Audit");
    controller.save_sources(std::slice::from_ref(&edited), Place::Team, Some("qa-all"), None).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let expected = "{ \"name\": \"qa-all\", \"server\": \"qa-sql.example.com,1433\", \"database\": \"Shop_Audit\"";
    assert!(text.contains(expected), "{text}");
    controller.remove_source("qa-all").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), PROFILE, "and taken out again, the file as it was");
    assert!(controller.save_sources(&[whole("qa")], Place::Team, None, None).unwrap_err().contains("named qa already"));
    let bad = DatabaseSource::new("Bad", Reach::Env("X".into()));
    assert!(controller.save_sources(&[bad], Place::Team, None, None).unwrap_err().contains("name is required"));
}

#[tokio::test]
async fn only_my_sources_go_to_the_config_and_win_by_name() {
    let temp = TempDir::new("sources-mine");
    let (mut controller, _rx, _, _) = pulling(&temp, None);
    let path = own_profile(&temp, &mut controller, PROFILE);
    let mine = DatabaseSource::new("qa", Reach::Env("SHOP_QA_DB".into()));
    controller.save_sources(&[mine], Place::Mine, Some("qa"), None).unwrap();
    let item = "\r\n        { \"name\": \"qa\", \"server\": \"qa-sql.example.com,1433\", \"database\": \"Shop\", \
                \"auth\": \"sql\", \"user\": \"reader\" }\r\n    ";
    assert_eq!(std::fs::read_to_string(&path).unwrap(), PROFILE.replace(item, ""), "moved to only mine");
    let config = crate::json::read(&temp.join("config.json")).unwrap();
    assert_eq!(
        config["projects"][&controller.project.id]["databaseSources"],
        json!([{ "name": "qa", "connectionStringEnv": "SHOP_QA_DB" }])
    );
    let qa = controller.source("qa").unwrap();
    assert!(qa.mine && qa.reach == Reach::Env("SHOP_QA_DB".into()));
    assert!(controller.remove_source("qa").unwrap().contains("this machine's sources"));
    assert!(controller.sources().is_empty());
}

#[tokio::test]
async fn one_password_serves_a_server_and_login_and_one_kept_by_1_118_moves_quietly() {
    let temp = TempDir::new("sources-password");
    let (mut controller, _rx, _, store) = pulling(&temp, None);
    let legacy = credentials::target(&controller.project.id, "qa");
    store.set(&legacy, "reader", &Password::new(SECRET)).unwrap();
    assert!(controller.password_kept("qa"), "found where 1.118.0 kept it");
    let shared = credentials::shared_target(&controller.project.id, "reader@qa-sql.example.com,1433");
    assert_eq!(store.get(&shared).unwrap().map(|found| found.expose().to_string()).as_deref(), Some(SECRET));
    assert_eq!(store.get(&legacy).unwrap(), None, "moved, never lost");
    controller.write_mine_for_tests(&whole("qa-all"));
    assert!(controller.password_kept("qa-all"), "the same server and login: the same password");
    assert!(controller.forget_password("qa-all").unwrap().starts_with("Forgot"));
    assert!(!controller.password_kept("qa"));
}

#[tokio::test]
async fn a_servers_databases_are_listed_kept_and_tried_before_a_source_is_saved() {
    let temp = TempDir::new("sources-list");
    let (mut controller, mut rx, sql, _) = pulling(&temp, None);
    controller.write_mine_for_tests(&whole("qa-all"));
    assert!(controller.list_databases("qa-all", None).unwrap_err().contains("password isn't kept"));
    controller.keep_password("qa-all", &Password::new(SECRET)).unwrap();
    controller.list_databases("qa-all", None).unwrap();
    assert!(controller.is_listing("qa-all"));
    until(&mut rx, &mut controller, |controller| !controller.is_listing("qa-all")).await;
    let names: Vec<String> =
        controller.catalog("qa-all").unwrap().databases.iter().map(|listed| listed.name.clone()).collect();
    assert_eq!(names, ["Shop", "Shop_Audit"]);
    assert_eq!(crate::run::catalog::load(&controller.state_dir)["qa-all"].databases.len(), 2, "kept");
    let asked = sql.asked.lock().unwrap().clone();
    assert!(asked.iter().all(|asked| !asked.contains("qa-sql") || !asked.contains("ALTER")), "only reads there");

    let remotes: Vec<String> =
        controller.remotes().iter().map(|remote| format!("{}/{}", remote.source, remote.database)).collect();
    assert_eq!(remotes, ["qa/Shop", "qa-all/Shop", "qa-all/Shop_Audit"]);
    assert_eq!(controller.suggest_twins("Shop_Audit_qa_20261101")[0].database, "Shop_Audit");

    let job = controller.try_source(&whole("new"), Some(Password::new(SECRET))).unwrap();
    until(&mut rx, &mut controller, |controller| controller.tried.is_some()).await;
    let (tried, result) = controller.tried.clone().unwrap();
    let result = result.unwrap();
    assert_eq!((tried, result.login.as_deref(), result.databases.len()), (job, Some("reader"), 2));
    controller.sources_news(News::Listed { source: "qa-all".into(), result: Err("Login failed".into()) });
    assert!(said(&controller).contains("✗ Couldn't list qa-all's databases: Login failed"));

    let kept = controller.keep_as_source("qa-all", "Shop_Audit").unwrap();
    assert!(kept.contains("qa-all-shop_audit") && kept.contains("only mine"), "{kept}");
    assert_eq!(controller.source("qa-all-shop_audit").unwrap().database(), Some("Shop_Audit"));
}

/// Updates the linked Shop until the pull is over.
async fn update(rx: &mut UnboundedReceiver<Event>, controller: &mut Controller) {
    controller.update_linked("localhost", "Shop", None).unwrap();
    pulled(rx, controller).await;
    until(rx, controller, |controller| controller.pull.as_ref().is_some_and(|pull| pull.is_over())).await;
}

#[tokio::test]
async fn an_update_imports_its_twin_beside_the_linked_database_and_swaps_it_in() {
    let temp = TempDir::new("sources-update");
    let (mut controller, mut rx, sql, _) = pulling(&temp, None);
    controller.keep_password("qa", &Password::new(SECRET)).unwrap();
    assert!(controller.update_linked("localhost", "Shop", None).unwrap_err().contains("isn't linked"));
    controller.link_database(("localhost", "Shop"), ("qa", "Shop"), Some("2026-11-01T09:00:00Z".into())).unwrap();
    update(&mut rx, &mut controller).await;
    let pull = controller.pull.as_ref().unwrap();
    assert!(pull.went_well() && pull.mode == Mode::Update, "{}", said(&controller));
    let states: Vec<State> = pull.steps().iter().map(|step| pull.step(*step).state).collect();
    assert_eq!(states, [State::Done, State::Done, State::Done, State::Done, State::Skipped, State::Done]);
    let old = pull.old.clone().unwrap();
    assert!(old.starts_with("Shop_old_"), "{old}");
    let asked = sql.asked.lock().unwrap().clone();
    let swap = asked.iter().find(|asked| asked.contains("MODIFY NAME")).expect("the swap ran");
    assert!(swap.starts_with("Server=localhost"));
    assert!(swap.contains(&format!("ALTER DATABASE [Shop] MODIFY NAME = [{old}]")), "{swap}");
    let link = controller.link_of("localhost", "Shop").unwrap();
    assert!(link.pulled.as_deref().is_some_and(|at| at > "2026-11-02"), "pulled just now");
    let offered: Vec<String> = controller.pull_stale.iter().map(|copy| copy.database.clone()).collect();
    assert_eq!(offered, std::slice::from_ref(&old), "the old copy offered");
    let text = std::fs::read_to_string(temp.join("secrets.json")).unwrap();
    assert!(!text.contains("Shop_qa_"), "the connection strings stay as they were: {text}");
    assert!(said(&controller).contains("✓ Updated Shop @ localhost from qa/Shop in"));
}

#[tokio::test]
async fn a_failed_swap_puts_the_names_back_and_a_failure_before_it_drops_only_the_new_copy() {
    let temp = TempDir::new("sources-update-fail");
    let (mut controller, mut rx, sql, _) = pulling(&temp, None);
    controller.keep_password("qa", &Password::new(SECRET)).unwrap();
    controller.link_database(("localhost", "Shop"), ("qa", "Shop"), None).unwrap();
    *sql.fail.lock().unwrap() = Some("MODIFY NAME");
    update(&mut rx, &mut controller).await;
    let pull = controller.pull.as_ref().unwrap();
    assert_eq!(pull.error.as_ref().map(|(step, _)| *step), Some(Step::Swap));
    assert!(said(&controller).contains("the names are as they were"), "{}", said(&controller));
    assert!(controller.link_of("localhost", "Shop").unwrap().pulled.is_none(), "the link's age unchanged");

    *sql.fail.lock().unwrap() = Some("UPDATE Settings");
    update(&mut rx, &mut controller).await;
    let pull = controller.pull.as_ref().unwrap();
    assert_eq!(pull.error.as_ref().map(|(step, _)| *step), Some(Step::AfterPull));
    let new = pull.database.clone();
    assert!(said(&controller).contains(&format!("its new copy {new} is dropped")), "{}", said(&controller));
    assert!(controller.database_busy().is_some_and(|busy| busy.contains(&new)), "dropping it");
    assert!(
        !sql.asked.lock().unwrap().iter().any(|asked| asked.contains("DROP DATABASE [Shop]")),
        "never the linked one"
    );
}

#[tokio::test]
async fn an_export_only_writes_the_file_and_an_import_links_without_switching() {
    let temp = TempDir::new("sources-export");
    let (mut controller, mut rx, _, _) = pulling(&temp, None);
    controller.keep_password("qa", &Password::new(SECRET)).unwrap();
    controller.pull_from(Mode::Export, "qa", None, None).unwrap();
    pulled(&mut rx, &mut controller).await;
    assert!(controller.pull.as_ref().unwrap().went_well(), "{}", said(&controller));
    assert!(said(&controller).contains("✓ Exported qa/Shop in"));
    assert!(controller.db_files.iter().any(|file| file.name.starts_with("Shop_qa_")), "in the files folder");
    assert!(controller.db_links.is_empty(), "an export links nothing");

    controller.pull_from(Mode::Import, "qa", None, None).unwrap();
    pulled(&mut rx, &mut controller).await;
    let pull = controller.pull.as_ref().unwrap();
    assert!(pull.went_well(), "{}", said(&controller));
    let text = std::fs::read_to_string(temp.join("secrets.json")).unwrap();
    assert_eq!(crate::run::secrets::active_label(&text).unwrap(), "Shop @ localhost", "not switched to");
    assert!(controller.link_of("localhost", &pull.database).is_some(), "linked to its twin");
    controller.write_mine_for_tests(&whole("qa-all"));
    assert!(controller.pull_from(Mode::Pull, "qa-all", None, None).unwrap_err().contains("pick one of its databases"));
}

#[tokio::test]
async fn stale_pulled_dims_a_linked_copy_older_than_it_says_and_a_twin_that_changed_since_is_noticed() {
    let temp = TempDir::new("sources-stale");
    let (mut controller, _rx, _, _) = pulling(&temp, None);
    let days_ago = |days: i64| (jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24 * days)).to_string();
    controller.link_database(("localhost", "Shop"), ("qa", "Shop"), Some(days_ago(5))).unwrap();
    let link = controller.link_of("localhost", "Shop").unwrap().clone();
    assert!(controller.stale_days().is_none() && !controller.is_stale(&link), "off by default");
    controller.set_setting(super::sources::STALE_PULLED, json!(3));
    assert!(controller.is_stale(&link), "5 days old, stalePulled 3");
    controller.set_setting(super::sources::STALE_PULLED, json!(7));
    assert!(!controller.is_stale(&link));
    assert!(!controller.twin_changed(&link), "nothing listed: nothing said");
    let listed = crate::run::catalog::Listed { name: "SHOP".into(), changed: Some(days_ago(1)), ..Default::default() };
    let catalog = crate::run::catalog::Catalog { at: days_ago(0), databases: vec![listed] };
    controller.catalogs.insert("qa".into(), catalog);
    assert!(controller.twin_changed(&link), "changed a day ago, copied five days ago");
}
