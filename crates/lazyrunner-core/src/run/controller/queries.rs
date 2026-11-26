//! query_database (#112): an agent's read-only query, checked first (`query::check`) and run in the background - the
//! server may take a while - its answer kept until it's asked for. The connection string stays here: neither the
//! query's answer nor lazyrunner's log names it.

use std::sync::Arc;
use std::time::Instant;

use super::{Controller, Doing, Pending};
use crate::run::query::{self, Read};

/// The answers kept at most: an agent that stopped asking leaves its own behind.
const KEPT: usize = 8;

/// Where a query is.
#[derive(Debug, PartialEq)]
pub enum Queried {
    Running,
    Done(Read),
    Failed(String),
    /// No query of that id: long answered, or one of the oldest.
    Unknown,
}

/// The queries running or answered: each its id and its answer once it's there.
pub(super) type Queries = Vec<(u64, Pending<Result<Read, String>>)>;

impl Controller {
    /// Runs `text` against `database` on `server` in the background, read-only, at most `rows` rows - refused before
    /// it runs when it isn't one SELECT, when secrets.json has no connection string for the database, or while it's
    /// rewound. Its id, to ask for its answer with.
    pub fn query_database(&mut self, server: &str, database: &str, text: &str, rows: usize) -> Result<u64, String> {
        query::check(text)?;
        self.sql.ready()?;
        let rewinding = self.db_work.as_ref().is_some_and(|work| {
            matches!(work.doing, Doing::Rewinding(_))
                && work.server.eq_ignore_ascii_case(server)
                && work.database.eq_ignore_ascii_case(database)
        });
        if rewinding {
            return Err(format!("{database} @ {server} is being rewound - query it once that's done"));
        }
        let connection = self.connection(server, database)?;
        let rows = rows.clamp(1, query::MOST_ROWS);
        let batch = query::batch(text, rows);
        let id = self.new_db_job();
        let pending: Pending<Result<Read, String>> = Arc::default();
        let answer = Arc::clone(&pending);
        let (sql, database) = (Arc::clone(&self.sql), database.to_string());
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut lines = Vec::new();
            let result = sql
                .read(&connection, Some(&database), &batch, query::SECONDS, &mut |line| lines.push(line.to_string()))
                .and_then(|()| query::read(&lines, rows))
                .map(|read| Read { ms: started.elapsed().as_millis() as u64, ..read });
            if let Ok(mut slot) = answer.lock() {
                *slot = Some(result);
            }
        });
        self.queries.push((id, pending));
        let over = self.queries.len().saturating_sub(KEPT);
        self.queries.drain(..over);
        Ok(id)
    }

    /// Where query `id` is; an answer is given once.
    pub fn query_answer(&mut self, id: u64) -> Queried {
        let Some(at) = self.queries.iter().position(|(query, _)| *query == id) else { return Queried::Unknown };
        let done = self.queries[at].1.lock().ok().and_then(|mut slot| slot.take());
        let Some(result) = done else { return Queried::Running };
        self.queries.remove(at);
        match result {
            Ok(read) => Queried::Done(read),
            Err(why) => Queried::Failed(why),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use serde_json::json;

    use super::super::tests::controller;
    use super::*;
    use crate::run::query::tests::line;
    use crate::run::sql::Sql;
    use crate::testing::TempDir;

    /// What an agent's query was run with: the connection string, the database, the batch and its time limit.
    type Asked = (String, Option<String>, String, u32);

    /// SQL that answers an agent's query with two rows of one column, and keeps what it was asked and where.
    #[derive(Default)]
    struct Scripted {
        asked: Mutex<Vec<Asked>>,
    }

    impl Sql for Scripted {
        fn run(&self, _: &str, _: Option<&str>, _: &str, _: &mut dyn FnMut(&str)) -> Result<(), String> {
            Err("only read runs an agent's query".into())
        }

        fn read(
            &self,
            connection: &str,
            database: Option<&str>,
            sql: &str,
            seconds: u32,
            line_out: &mut dyn FnMut(&str),
        ) -> Result<(), String> {
            let asked = (connection.to_string(), database.map(str::to_string), sql.to_string(), seconds);
            self.asked.lock().unwrap().push(asked);
            line_out(&line('N', r#"[{"name":"Customer"}]"#));
            line_out(&line('R', r#"{"c1":"Ana"}"#));
            line_out(&line('R', r#"{"c1":"Bob"}"#));
            line_out("E");
            Ok(())
        }

        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn answered(controller: &mut Controller, id: u64) -> Queried {
        for _ in 0..200 {
            match controller.query_answer(id) {
                Queried::Running => std::thread::sleep(Duration::from_millis(10)),
                other => return other,
            }
        }
        panic!("query {id} never answered");
    }

    #[test]
    fn a_query_runs_against_the_database_named_and_answers_once_capped() {
        let temp = TempDir::new("query-database");
        let (mut controller, _rx) = controller(&temp, json!({}));
        let secrets = temp.join("secrets.json");
        let shop = "Server=localhost;Database=Shop;Trusted_Connection=True";
        std::fs::write(&secrets, format!("{{\n  \"DefaultConnection\": \"{shop}\"\n}}\n")).unwrap();
        controller.secrets_at = Some(secrets);
        let sql = Arc::new(Scripted::default());
        controller.sql = sql.clone();

        let id = controller.query_database("localhost", "Shop", "SELECT Customer FROM Orders", 1).unwrap();
        let Queried::Done(read) = answered(&mut controller, id) else { panic!("it failed") };
        assert_eq!(read.columns, [Some("Customer".to_string())]);
        assert_eq!((read.rows, read.more), (vec![vec![json!("Ana")]], true), "one row asked for, two came");
        assert_eq!(controller.query_answer(id), Queried::Unknown, "given once");
        let asked = sql.asked.lock().unwrap().clone();
        let (connection, database, batch, seconds) = &asked[0];
        assert_eq!((connection.as_str(), database.as_deref(), *seconds), (shop, Some("Shop"), query::SECONDS));
        assert!(batch.contains("BEGIN TRANSACTION") && batch.contains("SET ROWCOUNT 2;"), "{batch}");

        let refused = controller.query_database("localhost", "Shop", "UPDATE Orders SET Total = 0", 10).unwrap_err();
        assert!(refused.starts_with("refused before it ran"), "{refused}");
        let unknown = controller.query_database("localhost", "Billing", "SELECT 1", 10).unwrap_err();
        assert!(unknown.contains("no connection string for Billing @ localhost"), "{unknown}");
        assert_eq!(sql.asked.lock().unwrap().len(), 1, "neither ran");
    }

    #[test]
    fn only_the_newest_answers_are_kept() {
        let temp = TempDir::new("query-kept");
        let (mut controller, _rx) = controller(&temp, json!({}));
        let secrets = temp.join("secrets.json");
        let shop = "Server=localhost;Database=Shop;Trusted_Connection=True";
        std::fs::write(&secrets, format!("{{\n  \"DefaultConnection\": \"{shop}\"\n}}\n")).unwrap();
        controller.secrets_at = Some(secrets);
        controller.sql = Arc::new(Scripted::default());
        let ids: Vec<u64> =
            (0..=KEPT).map(|_| controller.query_database("localhost", "Shop", "SELECT 1", 5).unwrap()).collect();
        assert_eq!(controller.query_answer(ids[0]), Queried::Unknown);
        assert!(matches!(answered(&mut controller, ids[KEPT]), Queried::Done(_)));
    }
}
