//! Faults on the calls between apps (#105): a call from one app to another (or to a host) delayed, failed with a
//! status, dropped, or answered with a canned response - for a share of the calls, on a route's prefix. They're kept
//! in `faults.json` in the project's state folder, which an npm app's call hook and a .NET app's startup hook read as
//! they change; a failed or mocked call goes to lazyrunner's fault server, which answers it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

/// The faults the hooks read, in the state folder; where a .NET app finds it, and which app it is.
pub const FILE: &str = "faults.json";
pub const ENV_FILE: &str = "LAZYRUNNER_FAULTS";
pub const ENV_APP: &str = "LAZYRUNNER_APP";
pub const ENV_HOOKS: &str = "DOTNET_STARTUP_HOOKS";
/// Where a failed or mocked call goes on the fault server, by the fault's id.
pub const PATH_PREFIX: &str = "/__lazyrunner_fault/";
/// The header a Node app's failed or mocked call carries its fault's id in.
pub const HEADER: &str = "x-lazyrunner-fault";
/// The .NET startup hook's source, and the folder it's built in (its version in the name: a new source builds anew).
const HOOK_SOURCE: &str = include_str!("fault-hook.cs");
const HOOK_FOLDER: &str = "fault-hook-1";
const HOOK_NAME: &str = "LazyrunnerFaults";
const DEFAULT_FRAMEWORK: &str = "net8.0";
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const MOST_HEAD: usize = 64 * 1024;
const JSON: &str = "application/json";

/// What happens to a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Delay(u64),
    Fail(u16),
    Drop,
    /// A canned response: its status, its body and the file it came from.
    Mock {
        status: u16,
        body: String,
        file: String,
    },
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Delay(_) => "delay",
            Self::Fail(_) => "fail",
            Self::Drop => "drop",
            Self::Mock { .. } => "mock",
        }
    }
}

/// A fault on `caller`'s calls to `target` (`host:port`, or a host) - `to_app` when it's another app - on routes
/// starting with `path`, for `share` percent of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub id: u64,
    pub caller: String,
    pub target: String,
    pub to_app: Option<String>,
    pub path: String,
    pub kind: Kind,
    pub share: u8,
}

impl Fault {
    /// As its edge on the map says it: `500 · 30%`, `+800 ms`, `drop`, `mock 200`.
    pub fn said(&self) -> String {
        let what = match &self.kind {
            Kind::Delay(ms) => format!("+{ms} ms"),
            Kind::Fail(status) => status.to_string(),
            Kind::Drop => "drop".into(),
            Kind::Mock { status, .. } => format!("mock {status}"),
        };
        let share = if self.share < 100 { format!(" · {}%", self.share) } else { String::new() };
        let path = if self.path.is_empty() { String::new() } else { format!(" {}", self.path) };
        format!("{what}{share}{path}")
    }

    fn to_json(&self) -> Value {
        let (ms, status) = match &self.kind {
            Kind::Delay(ms) => (*ms, 0),
            Kind::Fail(status) | Kind::Mock { status, .. } => (0, *status),
            Kind::Drop => (0, 0),
        };
        json!({
            "id": self.id, "caller": self.caller, "target": self.target, "path": self.path, "share": self.share,
            "kind": self.kind.name(), "ms": ms, "status": status,
        })
    }
}

/// Writes `faults` for the hooks, with the fault server's port.
pub fn write(file: &Path, port: u16, faults: &[Fault]) -> Result<()> {
    let faults: Vec<Value> = faults.iter().map(Fault::to_json).collect();
    crate::config::write(file, &json!({ "port": port, "faults": faults }))
}

/// The faults the server answers for, and how many calls each answered - shared with its threads.
type Shared = Arc<Mutex<Vec<Fault>>>;
type Hits = Arc<Mutex<HashMap<u64, usize>>>;

/// The server failed and mocked calls go to: it answers each with its fault's status and body, and counts them.
pub struct Server {
    pub port: u16,
    faults: Shared,
    hits: Hits,
}

impl Server {
    pub fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let (faults, hits): (Shared, Hits) = Default::default();
        let (shared, counted) = (Arc::clone(&faults), Arc::clone(&hits));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (faults, hits) = (Arc::clone(&shared), Arc::clone(&counted));
                std::thread::spawn(move || answer(stream, &faults, &hits));
            }
        });
        Ok(Self { port, faults, hits })
    }

    /// The faults it answers for now.
    pub fn set(&self, faults: &[Fault]) {
        if let Ok(mut kept) = self.faults.lock() {
            *kept = faults.to_vec();
        }
    }

    /// How many calls fault `id` answered.
    pub fn hits(&self, id: u64) -> usize {
        self.hits.lock().ok().and_then(|hits| hits.get(&id).copied()).unwrap_or(0)
    }
}

/// Reads a call's head (and its body, so the caller isn't cut off), and answers it as its fault says.
fn answer(stream: TcpStream, faults: &Mutex<Vec<Fault>>, hits: &Mutex<HashMap<u64, usize>>) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let mut reader = BufReader::new(&stream);
    let (mut head, mut line, mut size) = (Vec::new(), String::new(), 0);
    while reader.read_line(&mut line).is_ok_and(|read| read > 0) && !line.trim_end().is_empty() && size < MOST_HEAD {
        size += line.len();
        head.push(std::mem::take(&mut line));
    }
    let header = |name: &str| {
        head.iter().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim().to_string())
        })
    };
    let length: u64 = header("content-length").and_then(|value| value.parse().ok()).unwrap_or(0);
    let _ = reader.by_ref().take(length).read_to_end(&mut Vec::new());
    let path = head.first().and_then(|line| line.split_whitespace().nth(1)).unwrap_or_default();
    let id = path.strip_prefix(PATH_PREFIX).map(str::to_string).or_else(|| header(HEADER));
    let fault = id
        .and_then(|id| id.trim_matches('/').parse::<u64>().ok())
        .and_then(|id| faults.lock().ok()?.iter().find(|fault| fault.id == id).cloned());
    let (status, body, kind) = match fault.as_ref().map(|fault| &fault.kind) {
        Some(Kind::Mock { status, body, file }) => (*status, body.clone(), content_type(file)),
        Some(Kind::Fail(status)) => (*status, r#"{"error":"a fault lazyrunner set (#105)"}"#.to_string(), JSON),
        _ => (404, "no such fault".to_string(), "text/plain"),
    };
    if let (Some(fault), Ok(mut hits)) = (&fault, hits.lock()) {
        *hits.entry(fault.id).or_default() += 1;
    }
    let reason = if status < 400 { "OK" } else { "Fault" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = (&stream).write_all(response.as_bytes());
}

fn content_type(file: &str) -> &'static str {
    match Path::new(file).extension().and_then(|extension| extension.to_str()) {
        Some("xml") => "application/xml",
        Some("txt") => "text/plain",
        Some("html") => "text/html",
        _ => JSON,
    }
}

/// A .NET project's target framework (the first of several); net8.0 when it doesn't say.
pub fn target_framework(project: &Path) -> String {
    let text = std::fs::read_to_string(project).unwrap_or_default();
    let found = ["<TargetFramework>", "<TargetFrameworks>"].into_iter().find_map(|tag| {
        let from = text.find(tag)? + tag.len();
        let value = &text[from..from + text[from..].find('<')?];
        value.split(';').next().map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
    });
    found.unwrap_or_else(|| DEFAULT_FRAMEWORK.to_string())
}

/// Where the hook for `framework` is once built.
pub fn hook_path(state_dir: &Path, framework: &str) -> PathBuf {
    state_dir.join(HOOK_FOLDER).join(framework).join("out").join(format!("{HOOK_NAME}.dll"))
}

/// Builds the startup hook for `framework` with dotnet (once: a built one is kept); where it is, or why it isn't.
pub fn build_hook(state_dir: &Path, framework: &str) -> Result<PathBuf, String> {
    let built = hook_path(state_dir, framework);
    if built.is_file() {
        return Ok(built);
    }
    let dir = state_dir.join(HOOK_FOLDER).join(framework);
    let project = format!(
        "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <TargetFramework>{framework}</TargetFramework>\n    \
         <Nullable>enable</Nullable>\n    <AssemblyName>{HOOK_NAME}</AssemblyName>\n    <ImplicitUsings>disable</ImplicitUsings>\n  \
         </PropertyGroup>\n</Project>\n"
    );
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join(format!("{HOOK_NAME}.csproj")), project))
        .and_then(|()| std::fs::write(dir.join("StartupHook.cs"), HOOK_SOURCE));
    written.map_err(|error| format!("couldn't write the hook's project: {error}"))?;
    let output = Command::new("dotnet")
        .args(["build", "-c", "Release", "-o", "out", "--nologo", "-v", "q"])
        .current_dir(&dir)
        .output()
        .map_err(|error| format!("dotnet didn't run: {error}"))?;
    if built.is_file() {
        return Ok(built);
    }
    let said = String::from_utf8_lossy(&output.stdout);
    let error = said.lines().find(|line| line.contains("error")).unwrap_or("the build failed");
    Err(error.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn fault(id: u64, kind: Kind) -> Fault {
        Fault {
            id,
            caller: "web".into(),
            target: "localhost:47201".into(),
            to_app: None,
            path: String::new(),
            kind,
            share: 100,
        }
    }

    #[test]
    fn the_server_answers_a_failed_or_mocked_call_as_its_fault_says() {
        let server = Server::start().unwrap();
        let mock = Kind::Mock { status: 200, body: r#"{"id":7}"#.into(), file: "order.json".into() };
        server.set(&[fault(1, Kind::Fail(503)), fault(2, mock)]);
        let ask = |path: &str| {
            let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
            let request = format!("POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\n\r\n{{}}");
            stream.write_all(request.as_bytes()).unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            answer
        };
        assert!(ask("/__lazyrunner_fault/1").starts_with("HTTP/1.1 503"));
        let mocked = ask("/__lazyrunner_fault/2");
        assert!(mocked.starts_with("HTTP/1.1 200") && mocked.ends_with(r#"{"id":7}"#), "{mocked}");
        assert!(ask("/__lazyrunner_fault/9").starts_with("HTTP/1.1 404"));
        assert_eq!((server.hits(1), server.hits(2)), (1, 1));
    }

    #[test]
    fn faults_are_written_for_the_hooks_and_say_themselves_short() {
        let temp = TempDir::new("faults");
        let file = temp.join(FILE);
        let delayed = Fault { share: 30, path: "/api".into(), ..fault(3, Kind::Delay(800)) };
        write(&file, 5000, std::slice::from_ref(&delayed)).unwrap();
        let written = crate::json::read(&file).unwrap();
        assert_eq!(written["port"], 5000);
        assert_eq!(written["faults"][0]["kind"], "delay");
        assert_eq!(written["faults"][0]["ms"], 800);
        assert_eq!(delayed.said(), "+800 ms · 30% /api");
        let project = temp.join("Orders.csproj");
        std::fs::write(&project, "<Project><PropertyGroup><TargetFrameworks>net9.0;net8.0</TargetFrameworks>").unwrap();
        assert_eq!(target_framework(&project), "net9.0");
        assert_eq!(target_framework(&temp.join("missing.csproj")), DEFAULT_FRAMEWORK);
    }
}
