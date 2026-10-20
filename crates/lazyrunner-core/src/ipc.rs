//! A local control channel (a named pipe on Windows, a Unix socket elsewhere) that speaks one JSON object per line.
//! The tests drive lazyrunner through it: inject keys, read the frame, quit. The agent channel (P5) builds on it.

use std::io;

use interprocess::local_socket::tokio::{Listener, Stream, prelude::*};
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions, Name};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// A request from a client, with where to send the answer.
pub struct Request {
    pub body: Value,
    pub reply: oneshot::Sender<Value>,
}

fn socket_name(name: &str) -> io::Result<Name<'static>> {
    if GenericNamespaced::is_supported() {
        format!("lazyrunner-{name}").to_ns_name::<GenericNamespaced>()
    } else {
        std::env::temp_dir().join(format!("lazyrunner-{name}.sock")).to_fs_name::<GenericFilePath>()
    }
}

/// Takes the name, so a clash or a bad name is an error the caller sees before anything runs.
pub fn listen(name: &str) -> io::Result<Listener> {
    ListenerOptions::new().name(socket_name(name)?).try_overwrite(true).create_tokio()
}

/// Hands every request on `listener` to `requests` until that channel closes.
pub async fn serve(listener: Listener, requests: mpsc::UnboundedSender<Request>) {
    loop {
        let connection = match listener.accept().await {
            Ok(connection) => connection,
            Err(_) => {
                // One client failing to connect doesn't close the channel for the next.
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                continue;
            }
        };
        if requests.is_closed() {
            return;
        }
        let requests = requests.clone();
        tokio::spawn(async move {
            let _ = handle(connection, requests).await;
        });
    }
}

async fn handle(connection: Stream, requests: mpsc::UnboundedSender<Request>) -> io::Result<()> {
    let (reader, mut writer) = connection.split();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(body) => {
                let (reply, answer) = oneshot::channel();
                if requests.send(Request { body, reply }).is_err() {
                    return Ok(());
                }
                answer.await.unwrap_or_else(|_| json!({ "ok": false, "error": "lazyrunner is closing" }))
            }
            Err(error) => json!({ "ok": false, "error": format!("not JSON: {error}") }),
        };
        writer.write_all(format!("{answer}\n").as_bytes()).await?;
    }
    Ok(())
}

/// Sends one request to the lazyrunner listening on `name` and returns its answer.
pub async fn request(name: &str, body: &Value) -> io::Result<Value> {
    let connection = Stream::connect(socket_name(name)?).await?;
    let (reader, mut writer) = connection.split();
    writer.write_all(format!("{body}\n").as_bytes()).await?;
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await?.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no answer"))?;
    serde_json::from_str(&line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_request_reaches_the_server_and_its_answer_comes_back() {
        let name = format!("test-{}", std::process::id());
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(serve(listen(&name).unwrap(), tx));
        tokio::spawn(async move {
            while let Some(request) = rx.recv().await {
                let keys = request.body["keys"].as_str().unwrap_or_default().to_string();
                let _ = request.reply.send(json!({ "ok": true, "echo": keys }));
            }
        });
        let answer = request(&name, &json!({ "keys": "l z" })).await.unwrap();
        assert_eq!(answer, json!({ "ok": true, "echo": "l z" }));
        let next = request(&name, &json!({ "keys": "q" })).await.unwrap();
        assert_eq!(next["echo"], "q", "the next client is served too");
    }
}
