//! The network side of running apps: which TCP ports are listening and who holds them (readiness, port
//! conflicts), which ones Windows reserved, and response probes (how long an app takes to answer).

use std::collections::HashMap;
use std::sync::Mutex;
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

/// How long the reserved ranges are kept: reading them runs netsh, and they change at boot, not by the minute.
const RESERVED_FOR: Duration = Duration::from_secs(60);

/// Ports reserved together: the first and the last.
pub type PortRange = (u16, u16);
/// When the reserved ranges were read, and what they were.
type Reserved = (Instant, Vec<PortRange>);
static RESERVED: Mutex<Option<Reserved>> = Mutex::new(None);

/// The TCP port ranges Windows reserved, first to last port: Hyper-V, WSL and Docker take blocks of the dynamic range
/// (49152-65535) at boot. Nothing listens on them, yet no app can either ("access forbidden"). From netsh, kept a
/// minute; none elsewhere.
pub fn reserved_ranges() -> Vec<PortRange> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let mut cached = RESERVED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, ranges)) = cached.as_ref()
        && at.elapsed() < RESERVED_FOR
    {
        return ranges.clone();
    }
    let ranges = std::process::Command::new("netsh")
        .args(["interface", "ipv4", "show", "excludedportrange", "protocol=tcp"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| parse_reserved(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    *cached = Some((Instant::now(), ranges.clone()));
    ranges
}

/// The ranges in netsh's excludedportrange table: the lines that start with two port numbers (its headings are in
/// the system's language).
pub fn parse_reserved(text: &str) -> Vec<PortRange> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some((words.next()?.parse().ok()?, words.next()?.parse().ok()?))
        })
        .collect()
}

/// The reserved range `port` is in.
pub fn reserved(port: u16, ranges: &[PortRange]) -> Option<PortRange> {
    ranges.iter().copied().find(|(first, last)| (*first..=*last).contains(&port))
}

/// Why an app can't have `port`, and what frees it.
pub fn reserved_advice(port: u16, (first, last): PortRange) -> String {
    format!(
        "port {port} is reserved by Windows ({first}-{last}, which Hyper-V, WSL or Docker took) - in an elevated \
         terminal net stop winnat, then net start winnat, frees it; or give the app a port below 49152"
    )
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

    #[test]
    fn netshs_reserved_ranges_are_read_in_any_language() {
        let english = "\nProtocol tcp Port Exclusion Ranges\n\nStart Port    End Port\n----------    --------\n      5357        5357\n     49700       49799\n     50000       50059     *\n     58121       58220\n\n* - Administered port exclusions.\n";
        let ranges = parse_reserved(english);
        assert_eq!(ranges, [(5357, 5357), (49700, 49799), (50000, 50059), (58121, 58220)]);
        let german = "Protokoll tcp Portausschlussbereiche\nStartport    Endport\n  58121   58220\n";
        assert_eq!(parse_reserved(german), [(58121, 58220)]);
        assert_eq!(reserved(58201, &ranges), Some((58121, 58220)));
        assert_eq!(reserved(47201, &ranges), None);
        assert_eq!(reserved(5357, &ranges), Some((5357, 5357)), "a range of one port");
    }
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
