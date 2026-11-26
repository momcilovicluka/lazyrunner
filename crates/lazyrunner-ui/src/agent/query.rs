//! query_database (#112): a read-only query against the database in use or one named by its label or name - checked
//! first, run in the background (`query_database` starts it, `query_result` asks for its answer, as `lazyrunner mcp`
//! does until it's there). The log says what it reads; the connection string stays in lazyrunner.

use lazyrunner_core::output::Hue;
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::controller::Queried;
use lazyrunner_core::run::query::{self, CELL_CHARS, MOST_ROWS};
use lazyrunner_core::run::secrets::mask_text;
use serde_json::{Value, json};

use super::{Answer, databases};

/// What of the query the log shows at most, in characters.
const SHOWN_CHARS: usize = 80;
const MORE: &str = "more rows than max_rows - raise it (1000 at most) or narrow the query (TOP, WHERE)";

/// The query in one line for the log: its blanks as single spaces, at most [`SHOWN_CHARS`] long, secrets masked.
fn shown(sql: &str) -> String {
    let line = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = line.chars().take(SHOWN_CHARS).collect();
    let more = if line.chars().count() > SHOWN_CHARS { "…" } else { "" };
    mask_text(&format!("{short}{more}"))
}

/// Starts the query `"sql"` against `"database"` (the one in use without it), at most `"max_rows"` rows: its id.
pub fn start(controller: &mut Controller, args: &Value, client: &str) -> Answer {
    let Some(sql) = args["sql"].as_str() else {
        return Err(("bad_args", "\"sql\" is the SELECT to run".into()));
    };
    let rows = match &args["max_rows"] {
        Value::Null => query::ROWS,
        value => value
            .as_u64()
            .filter(|rows| (1..=MOST_ROWS as u64).contains(rows))
            .map(|rows| rows as usize)
            .ok_or_else(|| ("bad_args", format!("\"max_rows\" is 1 to {MOST_ROWS}")))?,
    };
    query::check(sql).map_err(|why| ("refused", why))?;
    let (server, name) = databases::database(controller, args)?;
    let id = controller.query_database(&server, &name, sql, rows).map_err(|why| ("refused", why))?;
    let doing = format!("queries {name} (read-only): {}", shown(sql));
    controller.add_log(format!("▸ {client}: {doing}"), Hue::Magenta);
    Ok(json!({ "id": id, "database": format!("{name} @ {server}"), "doing": doing }))
}

/// Query `"id"`'s answer: `running` until it's there, then its columns, rows (each an array of values in the columns'
/// order), how many, whether there were more and how long it took.
pub fn answer(controller: &mut Controller, args: &Value) -> Answer {
    let Some(id) = args["id"].as_u64() else { return Err(("bad_args", "which query? (\"id\")".into())) };
    let read = match controller.query_answer(id) {
        Queried::Running => return Ok(json!({ "running": true })),
        Queried::Done(read) => read,
        Queried::Failed(why) => return Err(("sql_error", why)),
        Queried::Unknown => return Err(("not_found", format!("no query {id} - lazyrunner keeps the newest answers"))),
    };
    let mut notes = Vec::new();
    if read.more {
        notes.push(MORE.to_string());
    }
    if read.cut > 0 {
        notes.push(format!("{} values cut at {CELL_CHARS} characters - select SUBSTRING(...) for the rest", read.cut));
    }
    Ok(json!({
        "columns": read.columns,
        "rows": read.rows,
        "count": read.rows.len(),
        "more": read.more,
        "ms": read.ms,
        "note": (!notes.is_empty()).then(|| notes.join("; ")),
    }))
}
