//! Save points over MCP (#110): `save_point` makes one of a database, `rewind` takes it back to one (after
//! `"confirm": true` - what changed since is lost), `database_changes` says what changed since one, table by table;
//! get_status lists them. A database is named by its label (`Shop @ localhost`) or its name, the one in use when left
//! out; a save point by its label or id, the database's newest when left out. Passwords never leave lazyrunner.

use std::time::Duration;

use lazyrunner_core::run::Controller;
use lazyrunner_core::run::save_points::{self as points, Kind, SavePoint};
use lazyrunner_core::run::secrets::{self, Entry};
use serde_json::{Value, json};

use super::drive::{confirm_first, confirmed};
use super::{CLIENT_CHARS, Refusal, clean};

/// Counts older than this are counted again when an agent asks what changed.
const STALE: Duration = Duration::from_secs(5);

/// Every save point: its id, label, database, kind, when it was made, whether it's pinned or gone.
pub fn list(controller: &Controller) -> Vec<Value> {
    controller
        .save_points
        .iter()
        .map(|point| {
            json!({
                "id": point.id,
                "label": point.label,
                "database": format!("{} @ {}", point.database, point.server),
                "kind": point.kind.name(),
                "created": point.created,
                "pinned": point.pinned,
                "gone": point.gone,
            })
        })
        .collect()
}

/// The database `args["database"]` names - its label or name - else the one in use.
fn database(controller: &Controller, args: &Value) -> Result<(String, String), Refusal> {
    let path = secrets::secrets_path(&controller.root, &controller.apps)
        .ok_or_else(|| ("not_found", "no secrets.json: no API has a UserSecretsId".to_string()))?;
    let text = std::fs::read_to_string(path)
        .map_err(|error| ("not_found", format!("secrets.json couldn't be read: {error}")))?;
    let pick = |entry: &Entry| (entry.server.clone(), entry.database.clone());
    let Some(wanted) = args["database"].as_str() else {
        let groups = secrets::groups(&text);
        let active = groups.first().and_then(|(_, entries)| entries.iter().find(|entry| entry.active));
        return active.map(pick).ok_or_else(|| ("not_found", "no connection string is in use".to_string()));
    };
    let entries = secrets::connections(&text);
    let named =
        |entry: &&Entry| entry.label.eq_ignore_ascii_case(wanted) || entry.database.eq_ignore_ascii_case(wanted);
    let mut found: Vec<(String, String)> = entries.iter().filter(named).map(pick).collect();
    found.sort();
    found.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0) && a.1.eq_ignore_ascii_case(&b.1));
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => {
            let known: Vec<&str> = entries.iter().map(|entry| entry.label.as_str()).collect();
            Err((
                "bad_args",
                format!("no database '{}' - the ones there: {}", clean(wanted, CLIENT_CHARS), known.join(", ")),
            ))
        }
        _ => Err((
            "bad_args",
            "several servers have a database of that name - give its label (database @ server)".into(),
        )),
    }
}

/// The save point `args["save_point"]` names - its label or id - of the database, else its newest.
fn save_point(controller: &Controller, server: &str, name: &str, args: &Value) -> Result<SavePoint, Refusal> {
    let all = controller.save_points_of(server, name);
    let found = match args["save_point"].as_str() {
        Some(wanted) => {
            all.iter().find(|point| point.label.eq_ignore_ascii_case(wanted) || point.id.eq_ignore_ascii_case(wanted))
        }
        None => all.first(),
    };
    let known: Vec<&str> = all.iter().map(|point| point.label.as_str()).collect();
    found.map(|point| (*point).clone()).ok_or_else(|| {
        let there = if known.is_empty() { "none yet - save_point makes one".to_string() } else { known.join(", ") };
        ("not_found", format!("no such save point of {name} @ {server} (there: {there})"))
    })
}

/// Makes a save point of a database: a snapshot (`"kind"`, the default) or a backup.
pub fn make(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let (server, name) = database(controller, args)?;
    let kind = match args["kind"].as_str() {
        None | Some("snapshot") => Kind::Snapshot,
        Some("backup") => Kind::Backup,
        Some(_) => return Err(("bad_args", "\"kind\" is snapshot or backup".into())),
    };
    let label = args["label"].as_str().map(|label| clean(label, CLIENT_CHARS));
    let label = controller.make_save_point(&server, &name, kind, label.as_deref()).map_err(|why| ("refused", why))?;
    Ok(format!(
        "make a {} save point \"{label}\" of {name} @ {server} - get_status's save_points list it once it's made",
        kind.name()
    ))
}

/// Rewinds a database to a save point, after `"confirm": true`.
pub fn rewind(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let (server, name) = database(controller, args)?;
    let point = save_point(controller, &server, &name, args)?;
    if !confirmed(args) {
        let lost: Vec<String> =
            controller.lost_by_rewind(&point.id).iter().map(|other| format!("\"{}\"", other.label)).collect();
        let also =
            if lost.is_empty() { String::new() } else { format!("; the save points {} go too", lost.join(", ")) };
        let apps = controller.names(&controller.apps_on(&server, &name));
        let stops = if apps.is_empty() { String::new() } else { format!(", stops {apps} meanwhile") };
        return Err(confirm_first(&format!(
            "rewinding {name} @ {server} to \"{}\" ({}) loses what changed since{stops} and cuts off other \
             connections to it{also} -",
            point.label,
            point.clock()
        )));
    }
    controller.rewind_to(&point.id, None).map_err(|why| ("refused", why))
}

/// What changed in a database since a save point, as the screen shows it; counted again when the counts are stale.
pub fn changes(controller: &mut Controller, args: &Value) -> Result<Value, Refusal> {
    let (server, name) = database(controller, args)?;
    let point = save_point(controller, &server, &name, args)?;
    let stale = controller
        .counts(&server, &name)
        .is_none_or(|counts| counts.at.is_none_or(|at| at.elapsed() > STALE) && counts.reading.is_none());
    if stale {
        controller.count_tables(&server, &name);
    }
    let counts = controller.counts(&server, &name);
    let counted = counts.filter(|counts| counts.at.is_some());
    let changes = counted.map(|counts| points::changes(&point.tables, &counts.tables)).unwrap_or_default();
    let rows: Vec<Value> = changes
        .iter()
        .map(|change| {
            json!({
                "table": change.table,
                "then": change.then,
                "now": change.now,
                "delta": change.delta(),
                "changed_in_place": change.changed && change.delta() == 0,
            })
        })
        .collect();
    Ok(json!({
        "database": format!("{name} @ {server}"),
        "save_point": { "label": point.label, "created": point.created, "kind": point.kind.name(), "pinned": point.pinned },
        "counted_seconds_ago": counted.and_then(|counts| counts.at).map(|at| at.elapsed().as_secs()),
        "counting": counts.is_some_and(|counts| counts.reading.is_some()),
        "summary": counted.map(|_| points::summary(&changes)),
        "changes": rows,
        "error": counts.and_then(|counts| counts.error.clone()),
        "note": counted.is_none().then_some("the rows are being counted - call again in a moment"),
    }))
}
