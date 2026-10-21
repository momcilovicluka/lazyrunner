//! The network side of running apps: which TCP ports are listening and who holds them (readiness, port
//! conflicts), and response probes (how long an app takes to answer).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A process holding a port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub pid: u32,
    pub name: String,
}

/// The TCP ports that are listening, and who listens on each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listening {
    pub ports: HashMap<u16, Owner>,
    /// Why the ports couldn't be listed (then `ports` is empty, which isn't "nothing listens").
    pub problem: Option<String>,
}

impl Listening {
    /// A snapshot of this machine's listening TCP ports (blocking: a few milliseconds).
    pub fn now() -> Self {
        let all = match listeners::get_all() {
            Ok(all) => all,
            Err(error) => return Self { ports: HashMap::new(), problem: Some(error.to_string()) },
        };
        let mut ports = HashMap::new();
        for listener in all {
            if listener.protocol == listeners::Protocol::TCP && listener.state == listeners::SocketState::Listen {
                ports.insert(listener.socket.port(), Owner { pid: listener.process.pid, name: listener.process.name });
            }
        }
        Self { ports, problem: None }
    }

    pub fn contains(&self, port: u16) -> bool {
        self.ports.contains_key(&port)
    }
}

/// One probe: whether the app answered, its HTTP status, and how long it took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeResult {
    pub ok: bool,
    pub code: u16,
    pub ms: u64,
}

/// GETs `path` on localhost:`port` and times it inside the task, so a busy lazyrunner can't inflate the result.
pub async fn probe(port: u16, path: &str, timeout: Duration) -> ProbeResult {
    let started = Instant::now();
    let result = tokio::time::timeout(timeout, get(port, path)).await;
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(Some(code)) => ProbeResult { ok: true, code, ms },
        _ => ProbeResult { ok: false, code: 0, ms },
    }
}

/// The status code of `GET path` (only the headers are waited for).
async fn get(port: u16, path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(("localhost", port)).await.ok()?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nUser-Agent: lazyrunner\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut head = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];
    while !head.windows(2).any(|pair| pair == b"\r\n") && head.len() < 4096 {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    let status_line = String::from_utf8_lossy(&head);
    status_line.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn a_probe_reads_the_status_and_a_closed_port_fails() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 512];
            let _ = socket.read(&mut buffer).await;
            let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
        });
        let result = probe(port, "/swagger/index.html", Duration::from_secs(5)).await;
        assert!(result.ok && result.code == 404, "{result:?}");

        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        assert!(!probe(port, "/", Duration::from_secs(2)).await.ok);
    }

    #[test]
    fn the_snapshot_sees_a_listening_port_and_its_owner() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let listening = Listening::now();
        let owner = listening.ports.get(&port).expect("our own listener");
        assert_eq!(owner.pid, std::process::id());
    }
}
