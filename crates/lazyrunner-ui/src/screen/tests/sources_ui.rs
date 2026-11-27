//! The database screen's sources (#114): the Sources section with a whole server open, the add form filled by a
//! pasted connection string, tried and saved, the link picker and a linked row, an update's swap in the pull graph,
//! and the agent's `database` ops - never a password in any of it.

use std::cell::RefCell;
use std::time::Duration;

use lazyrunner_core::run::catalog::{Catalog, Listed};
use lazyrunner_core::run::controller::sources::{Place, Tried};
use lazyrunner_core::run::credentials::{self, Password, Store};
use lazyrunner_core::run::database_files::Progress;
use lazyrunner_core::run::pull::{Mode, State};
use serde_json::json;

use super::super::pull_view::Card;
use super::super::{Overlay, source_form};
use super::{Screen, agent, pulling_screen};
use lazyrunner_core::profile::sources::{Auth, DatabaseSource, Reach};

const SECRET: &str = "s3cret-Pa55";
const PASTED: &str = "Data Source=qa-sql.example.com,1433\\SQL2019;Initial Catalog=Shop;User ID=reader;\
                      Password=s3cret-Pa55;Encrypt=True;TrustServerCertificate=True;Application Name=\"SQL Server \
                      Management Studio\"";

thread_local! {
    /// What the tests' clipboard holds for Ctrl+V.
    static CLIPBOARD: RefCell<String> = const { RefCell::new(String::new()) };
}

fn clipboard() -> Result<String, String> {
    Ok(CLIPBOARD.with(|held| held.borrow().clone()))
}

/// An RFC 3339 time this long ago.
fn ago(seconds: i64) -> String {
    let at = jiff::Timestamp::now() - jiff::SignedDuration::from_secs(seconds);
    at.to_string()
}

fn whole(name: &str) -> DatabaseSource {
    let reach = Reach::Server {
        server: "qa-sql.example.com,1433".into(),
        database: None,
        auth: Auth::Sql,
        user: Some("reader".into()),
    };
    DatabaseSource::new(name, reach)
}

fn listed(name: &str, bytes: u64, changed: i64) -> Listed {
    Listed { name: name.into(), bytes: Some(bytes), created: None, changed: Some(ago(changed)) }
}

/// The pulling screen with a whole server of this machine's own, listed three minutes ago, Shop linked to its twin.
fn sources_screen(label: &str) -> Screen {
    let (mut screen, _) = pulling_screen(label);
    screen.controller.save_sources(&[whole("qa-all")], Place::Mine, None, None).unwrap();
    let databases = vec![
        listed("Shop", 4_509_715_660, 3_600),
        listed("Shop_Audit", 1_181_116_006, 2 * 86_400),
        listed("Tenant_Acme", 398_458_880, 8 * 86_400),
    ];
    screen.controller.catalogs.insert("qa-all".into(), Catalog { at: ago(180), databases });
    let pulled = Some(ago(12 * 86_400 + 60));
    screen.controller.link_database(("localhost", "Shop"), ("qa-all", "Shop"), pulled).unwrap();
    screen
}

#[tokio::test]
async fn database_sources() {
    let mut screen = sources_screen("snap-database-sources");
    screen.press("Tab");
    assert!(screen.ui.sources_view.focused, "nothing else to pick: Tab goes to the sources");
    screen.press("j Enter");
    let frame = screen.frame();
    for expected in [
        "↔ qa-all/Shop · pulled 12 d ago",
        "▾ ≡  qa-all",
        "3 databases · listed 3 min ago",
        "only mine",
        "Shop_Audit",
        "1.1 GB",
        "changed 2 d ago",
        "↔ Shop",
    ] {
        assert!(frame.contains(expected), "{expected}: {frame}");
    }
    insta::assert_snapshot!("database_sources", frame);

    screen.press("/ T e n Enter");
    let frame = screen.frame();
    assert!(frame.contains("Tenant_Acme") && !frame.contains("Shop_Audit"), "filtered: {frame}");
    screen.press("Esc Esc");
    assert!(!screen.ui.sources_view.focused);
}

#[tokio::test]
async fn a_server_opens_and_lists_its_databases_and_a_database_under_it_is_kept_as_a_source() {
    let mut screen = sources_screen("sources-open");
    screen.controller.catalogs.clear();
    screen.press("Tab j Enter");
    assert!(super::super::pull_view::asking_password(&screen.ui), "listing needs its password first");
    screen.press("Esc");
    screen.controller.keep_password("qa-all", &Password::new(SECRET)).unwrap();
    screen.press("Enter Enter");
    assert!(screen.controller.is_listing("qa-all"), "opened again: listed - {}", screen.last_log());
    assert!(screen.frame().contains("◌ listing…"));
    screen.controller.catalogs.insert("qa-all".into(), Catalog { at: ago(1), databases: vec![listed("Shop", 1, 1)] });
    screen.controller.listing.clear();
    screen.press("j +");
    let kept = screen.controller.source("qa-all-shop").expect("kept as a source of its own");
    assert_eq!((kept.database(), kept.mine), (Some("Shop"), true));
    assert!(screen.frame().contains("qa-all-shop"));
}

/// The add form, as it is now.
fn form(screen: &Screen) -> &source_form::Form {
    match screen.ui.overlay.as_ref() {
        Some(Overlay::Pull(card)) => match card.as_ref() {
            Card::Form(form) => form,
            other => panic!("not the form: {other:?}"),
        },
        _ => panic!("no card"),
    }
}

#[tokio::test]
async fn a_pasted_connection_string_fills_the_form_is_tried_and_saved_for_the_team() {
    let (mut screen, store) = pulling_screen("source-form");
    let profile = screen.temp.join(".lazyrunner.json");
    let text = "{\n  \"id\": \"sample\",\n  \"name\": \"Sample\",\n  \"apps\": []\n}\n";
    std::fs::write(&profile, text).unwrap();
    screen.controller.project.path = profile.clone();
    screen.controller.project.source = lazyrunner_core::profile::Source::Given;
    screen.ui.paste = clipboard;
    CLIPBOARD.with(|held| *held.borrow_mut() = PASTED.to_string());
    screen.press("Tab a");
    assert!(super::super::pull_view::asking_password(&screen.ui), "the form is the user's: an agent never types in it");
    screen.press("Ctrl+V");
    let frame = screen.frame();
    assert!(frame.contains("qa-sql.example.com,1433") && !frame.contains("SQL2019"), "the port kept: {frame}");
    assert!(frame.contains("◉ one: Shop") && frame.contains("•".repeat(SECRET.len()).as_str()), "{frame}");
    assert!(frame.contains("left out (a pull sets them itself): Encrypt,"), "listed once: {frame}");
    assert!(frame.contains("TrustServerCertificate, Application Name"), "{frame}");
    assert!(!frame.contains(SECRET), "the password never shows: {frame}");
    assert_eq!(form(&screen).text[source_form::NAME], "qa-2", "named from the server, free");
    insta::assert_snapshot!("database_source_form", frame);

    screen.press("Ctrl+G");
    let job = form(&screen).trying.expect("trying").0;
    screen.controller.pretend_tried(job, Err("Login failed for user 'reader'.".into()));
    let frame = screen.frame();
    assert!(frame.contains("✗ Login failed for user 'reader'."), "{frame}");
    screen.press("Ctrl+G");
    let job = form(&screen).trying.expect("trying again").0;
    let databases = vec![listed("Shop", 4_509_715_660, 60), listed("Shop_Audit", 1_181_116_006, 60)];
    screen.controller.pretend_tried(job, Ok(Tried { login: Some("reader".into()), ms: 38, databases }));
    let frame = screen.frame();
    assert!(frame.contains("✓ signed in as reader · 38 ms · 2 databases") && frame.contains("☑ Shop"), "{frame}");
    insta::assert_snapshot!("database_source_form_tried", frame);

    screen.press("Down Down Space Enter");
    let written = std::fs::read_to_string(&profile).unwrap();
    assert!(
        written.contains("{ \"name\": \"qa-2-shop\", \"server\": \"qa-sql.example.com,1433\", \"database\": \"Shop\""),
        "{written}"
    );
    assert!(written.contains("\"qa-2-shop_audit\""), "{written}");
    assert!(
        written.starts_with(
            "{\n  \"id\": \"sample\",\n  \"name\": \"Sample\",\n  \"apps\": [],\n  \"databaseSources\": ["
        )
    );
    let target = credentials::shared_target(&screen.controller.project.id, "reader@qa-sql.example.com,1433");
    assert_eq!(store.get(&target).unwrap().map(|kept| kept.expose().to_string()).as_deref(), Some(SECRET));
    screen.ui.now += Duration::from_secs(1);
    screen.frame();
    assert!(screen.ui.overlay.is_none(), "folded away");
    assert!(screen.frame().contains("qa-2-shop"), "the new rows");
    let said: Vec<String> = screen.controller.log.iter().map(|entry| entry.text.clone()).collect();
    assert!(!said.iter().any(|line| line.contains(SECRET)), "{said:?}");

    screen.press("E");
    assert_eq!(form(&screen).editing.as_deref(), Some("qa-2-shop"), "E opens the form filled in");
    assert_eq!(form(&screen).text[source_form::SERVER], "qa-sql.example.com,1433");
}

#[tokio::test]
async fn a_local_database_links_to_its_closest_twin_and_unlinks_again() {
    let (mut screen, _) = pulling_screen("link-card");
    screen.controller.save_sources(&[whole("qa-all")], Place::Mine, None, None).unwrap();
    let databases = vec![listed("Shop_Audit", 1, 1), listed("Shop", 1, 1)];
    screen.controller.catalogs.insert("qa-all".into(), Catalog { at: ago(60), databases });
    screen.press("L");
    let frame = screen.frame();
    assert!(frame.contains("Link Shop to") && frame.contains("qa/Shop   suggested"), "{frame}");
    insta::assert_snapshot!("database_link_card", frame);
    screen.press("j Enter");
    let link = screen.controller.link_of("localhost", "Shop").expect("linked");
    assert_eq!(link.twin(), "qa-all/Shop");
    let frame = screen.frame();
    assert!(
        frame.contains("Shop @ localhost  ↔ qa-all/Shop") && frame.contains("U update from qa-all/Shop"),
        "{frame}"
    );
    screen.press("L");
    assert!(screen.controller.link_of("localhost", "Shop").is_none(), "L again unlinks");
}

#[tokio::test]
async fn u_updates_a_linked_database_and_the_graph_shows_the_swap() {
    let (mut screen, _) = pulling_screen("update-key");
    screen.controller.project.pulling.after_pull.clear();
    screen.controller.link_database(("localhost", "Shop"), ("qa", "Shop"), Some(ago(86_400))).unwrap();
    screen.press("U");
    assert!(super::super::pull_view::asking_password(&screen.ui), "its password first");
    screen.press("Esc");
    screen.controller.keep_password("qa", &Password::new(SECRET)).unwrap();
    screen.press("U");
    let said = screen.last_log();
    let pull = screen.controller.pull.as_ref().unwrap_or_else(|| panic!("U updates it: {said}"));
    assert_eq!((pull.mode, pull.target.as_deref()), (Mode::Update, Some("Shop")));

    let started = screen.ui.now - Duration::from_secs(95);
    let states = [State::Done, State::Done, State::Done, State::Done, State::Skipped, State::Running];
    screen.controller.pretend_job(Mode::Update, ("qa", "Shop_qa_20261127"), started, &states, Progress::default());
    let frame = screen.frame();
    assert!(frame.contains("UPDATE Shop @ localhost ← qa/Shop") && frame.contains("● swap"), "{frame}");
    assert!(frame.contains("↑ Shop_qa_202"), "the new one about to take its place: {frame}");
    insta::assert_snapshot!("database_update_swap", frame);
}

#[tokio::test]
async fn an_agent_reads_the_sources_and_links_but_never_a_password_and_asks_before_an_update() {
    let mut screen = sources_screen("agent-sources");
    let sources = agent(&mut screen, "database_sources", json!({ "source": "qa-all" }), None).to_string();
    assert!(sources.contains("\"name\":\"qa-all\"") && sources.contains("\"kind\":\"server\""), "{sources}");
    assert!(sources.contains("\"whose\":\"only mine\"") && sources.contains("\"twin\":\"qa-all/Shop\""), "{sources}");
    assert!(sources.contains("\"linked_to\":\"Shop @ localhost\""), "{sources}");
    assert!(!sources.contains(SECRET) && !sources.contains("Password="), "{sources}");
    let refused = agent(&mut screen, "export_database", json!({ "source": "qa-all", "database": "Shop" }), None);
    assert!(refused.to_string().contains("ask the user to pull once from lazyrunner"), "{refused}");
    let asked = agent(&mut screen, "update_database", json!({ "database": "Shop" }), None).to_string();
    assert!(asked.contains("password isn't kept"), "{asked}");
    screen.controller.keep_password("qa", &Password::new(SECRET)).unwrap();
    let linked = agent(&mut screen, "link_database", json!({ "database": "Shop", "source": "qa" }), None);
    assert_eq!(linked["ok"], json!(true), "{linked}");
    assert_eq!(screen.controller.link_of("localhost", "Shop").unwrap().twin(), "qa/Shop");
    let asked = agent(&mut screen, "update_database", json!({ "database": "Shop" }), None).to_string();
    assert!(asked.contains("needs_confirm") && asked.contains("swaps it in under its name"), "{asked}");
}
