//! Database sources over MCP (#114): the `database` tool's `sources` (the sources, and a source's databases as last
//! listed - never a secret), `update` (a linked database brought up to date from its twin, after `"confirm": true`)
//! and `link` (a local database linked to a remote one); `export` and `import` of a source's database go through
//! `databases`. An agent never adds a source or types a password: adding one, and its password, stay the user's.

use lazyrunner_core::profile::sources::Reach;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::catalog::Listed;
use lazyrunner_core::run::pull::Mode;
use serde_json::{Value, json};

use super::databases::database;
use super::drive::{confirm_first, confirmed};
use super::{CLIENT_CHARS, Refusal, clean};

/// A listed database as an agent reads it.
fn listed(controller: &Controller, source: &str, listed: &Listed) -> Value {
    let link = controller
        .db_links
        .iter()
        .find(|link| link.source.eq_ignore_ascii_case(source) && link.remote.eq_ignore_ascii_case(&listed.name));
    json!({
        "name": listed.name,
        "bytes": listed.bytes,
        "created": listed.created,
        "changed": listed.changed,
        "linked_to": link.map(|link| format!("{} @ {}", link.database, link.server)),
    })
}

/// The sources (where each is, how it logs in, whose it is, whether its password is kept - never the password), the
/// local databases linked to their twins, and with `"source"` that source's databases as last listed.
pub fn sources(controller: &Controller, args: &Value) -> Result<Value, Refusal> {
    let all: Vec<Value> = controller
        .sources()
        .iter()
        .map(|source| {
            let catalog = controller.catalog(&source.name);
            json!({
                "name": source.name,
                "kind": if source.is_server() { "server" } else { "database" },
                "where": source.label(),
                "login": match &source.reach {
                    Reach::Server { auth, user, .. } => user.clone().unwrap_or_else(|| auth.name().into()),
                    Reach::Env(_) => "its variable's".into(),
                },
                "whose": if source.mine { "only mine" } else { "team" },
                "password_kept": source.needs_password().then(|| controller.password_kept(&source.name)),
                "databases_listed": catalog.map(|catalog| catalog.databases.len()),
                "listed_at": catalog.map(|catalog| catalog.at.clone()),
                "listing": controller.is_listing(&source.name),
            })
        })
        .collect();
    let links: Vec<Value> = controller
        .db_links
        .iter()
        .map(|link| {
            json!({
                "database": format!("{} @ {}", link.database, link.server),
                "twin": link.twin(),
                "pulled": link.pulled,
                "twin_changed_since": controller.twin_changed(link),
                "stale": controller.is_stale(link),
            })
        })
        .collect();
    let mut answer = json!({ "sources": all, "links": links });
    if let Some(wanted) = args["source"].as_str() {
        let source = controller
            .source(wanted)
            .ok_or_else(|| ("not_found", format!("no source '{}'", clean(wanted, CLIENT_CHARS))))?;
        let databases: Vec<Value> = match (source.database(), controller.catalog(&source.name)) {
            (Some(database), _) => vec![json!({ "name": database })],
            (None, Some(catalog)) => catalog.databases.iter().map(|db| listed(controller, &source.name, db)).collect(),
            (None, None) => Vec::new(),
        };
        answer["databases"] = json!(databases);
        if source.is_server() && controller.catalog(&source.name).is_none() {
            answer["note"] =
                json!("not listed yet - the user lists a server's databases (Enter on it in the Sources section)");
        }
    }
    Ok(answer)
}

/// The source `"source"` names, refused when its password isn't kept (only the user types it).
fn source(controller: &Controller, args: &Value) -> Result<String, Refusal> {
    let wanted = args["source"]
        .as_str()
        .ok_or_else(|| ("bad_args", "\"source\" names a source (op sources lists them)".to_string()))?;
    let found = controller.source(&clean(wanted, CLIENT_CHARS)).ok_or_else(|| {
        let known: Vec<String> = controller.sources().into_iter().map(|source| source.name).collect();
        ("not_found", format!("no source '{}' - the ones there: {}", clean(wanted, CLIENT_CHARS), known.join(", ")))
    })?;
    if let Some(why) = controller.password_missing(&found.name) {
        return Err(("refused", why));
    }
    Ok(found.name)
}

/// `export` or `import` of a source's database (`"source"`, and `"database"` for one of a whole server's): only its
/// `.bacpac`, or a new local database not switched to (linked to its twin).
pub fn from_source(controller: &mut Controller, args: &Value, mode: Mode) -> Result<String, Refusal> {
    let name = source(controller, args)?;
    let database = args["database"].as_str().map(|database| clean(database, CLIENT_CHARS));
    let doing = controller.pull_from(mode, &name, database.as_deref(), None).map_err(|why| ("refused", why))?;
    let then = match mode {
        Mode::Export => "database op files lists the file once it's written",
        _ => "its connection string is added once it's in, not in use; wait for pull_done",
    };
    Ok(format!("{doing} - {then}"))
}

/// Brings a linked database (`"database"`, else the one in use) up to date from its twin, after `"confirm": true`:
/// its apps stop for the swap and start again.
pub fn update(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let (server, name) = database(controller, args)?;
    let link = controller.link_of(&server, &name).cloned().ok_or_else(|| {
        ("refused", format!("{name} @ {server} isn't linked to a remote database - op link links it"))
    })?;
    if let Some(why) = controller.password_missing(&link.source) {
        return Err(("refused", why));
    }
    if !confirmed(args) {
        let apps = controller.names(&controller.apps_on(&server, &name));
        let stops = if apps.is_empty() { String::new() } else { format!(", stopping {apps} for the swap") };
        return Err(confirm_first(&format!(
            "updating {name} @ {server} from {} imports a new copy beside it and swaps it in under its name{stops} (the \
             old one is kept as {name}_old_<date>) -",
            link.twin()
        )));
    }
    let doing = controller.update_linked(&server, &name, None).map_err(|why| ("refused", why))?;
    Ok(format!("{doing} - database op files says the step it's at; wait for pull_done"))
}

/// Links a local database (`"database"`, else the one in use) to `"source"`'s database (`"remote"`, else the source's
/// own).
pub fn link(controller: &mut Controller, args: &Value) -> Result<String, Refusal> {
    let (server, name) = database(controller, args)?;
    let wanted =
        args["source"].as_str().ok_or_else(|| ("bad_args", "\"source\" names the twin's source".to_string()))?;
    let source = controller
        .source(&clean(wanted, CLIENT_CHARS))
        .ok_or_else(|| ("not_found", format!("no source '{}'", clean(wanted, CLIENT_CHARS))))?;
    let remote = match (args["remote"].as_str(), source.database()) {
        (Some(remote), _) => clean(remote, CLIENT_CHARS),
        (None, Some(own)) => own.to_string(),
        (None, None) => {
            return Err(("bad_args", format!("{} is a whole server - \"remote\" names its database", source.name)));
        }
    };
    let pulled = controller.link_of(&server, &name).and_then(|link| link.pulled.clone());
    controller.link_database((&server, &name), (&source.name, &remote), pulled).map_err(|why| ("refused", why))
}
