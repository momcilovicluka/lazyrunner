//! query_database (#112): lazyrunner checks the query and runs it in the background - the server may take a while,
//! and lazyrunner answers each ask at once - so it's asked for the answer until it's there.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Channel, NOT_ANSWERING, Server};

/// How often lazyrunner is asked for the answer, and for how long: the query's own limit, the login's, and a margin.
const ANSWER_POLL: Duration = Duration::from_millis(200);
const ANSWER_WAIT: Duration = Duration::from_secs(45);

impl<C: Channel> Server<C> {
    /// query_database: the query started, then its answer - the database it read first.
    pub(super) async fn query(&self, args: Value) -> Result<Value, String> {
        let started = self.agent("query_database", args).await?;
        let asked = json!({ "id": started["id"] });
        let deadline = Instant::now() + ANSWER_WAIT;
        loop {
            tokio::time::sleep(ANSWER_POLL).await;
            match self.agent("query_result", asked.clone()).await {
                Ok(answer) if answer["running"] == true => {}
                Ok(answer) => return Ok(with_database(&started["database"], answer)),
                Err(why) if why == NOT_ANSWERING => {}
                Err(why) => return Err(why),
            }
            if Instant::now() >= deadline {
                return Err(format!("timed_out: the query didn't answer in {} s", ANSWER_WAIT.as_secs()));
            }
        }
    }
}

/// The answer with the database it read, first.
fn with_database(database: &Value, answer: Value) -> Value {
    let mut said = serde_json::Map::new();
    said.insert("database".into(), database.clone());
    if let Value::Object(fields) = answer {
        said.extend(fields);
    }
    Value::Object(said)
}
