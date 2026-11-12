//! CPU profiles (#83): `dotnet-trace` samples a running .NET app's threads for a few seconds, its trace converted to
//! Speedscope's JSON; the stretches each thread spent on the CPU (its stack's `CPU_TIME` leaf) become one call tree for
//! the whole app - each frame's time in itself and in all it called - with the hottest frames by self time, the split
//! by namespace, and how a capture compares with the one before. The last [`KEPT`] captures of each app are kept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long a capture samples, unless asked for another length.
pub const SECONDS: u64 = 10;
/// The captures kept of each app.
pub const KEPT: usize = 10;
/// A frame whose self time is this share of the whole is hot.
pub const HOT: f64 = 0.05;
/// The command that installs the tool.
pub const INSTALL: &str = "dotnet tool install -g dotnet-trace";
const TOOL: &str = "dotnet-trace";
/// The leaf of a stack while its thread ran on the CPU.
const CPU: &str = "CPU_TIME";
/// The frame that starts a thread's own stack; what's under it names the process and its activities.
const THREAD: &str = "Thread (";
/// dotnet-trace's profile that samples the threads: 10's name first, then the earlier versions'.
const PROFILES: [&str; 2] = ["dotnet-sampled-thread-time", "cpu-sampling"];
/// What dotnet-trace says of a profile its version doesn't have.
const NO_SUCH_PROFILE: [&str; 3] = ["does not apply", "Unknown profile", "is not a valid"];
const ROOT: &str = "all";

/// A frame in the call tree: which one, its time in all it ran (ms), its time in itself, its callees, its caller.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub frame: usize,
    pub total: f64,
    pub own: f64,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
}

/// A capture: the app's key, when (RFC 3339) and for how long, the frames' names and the call tree (node 0 is all).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub app: String,
    pub at: String,
    pub seconds: u64,
    pub frames: Vec<String>,
    pub nodes: Vec<Node>,
    /// Each frame's file and line, when the profile says it (Node's do).
    #[serde(default)]
    pub sources: Vec<Option<(String, u32)>>,
}

/// A frame by its time: its name, its time in itself and in all.
#[derive(Clone, Debug, PartialEq)]
pub struct Hot {
    pub name: String,
    pub own: f64,
    pub total: f64,
}

/// A frame's name without its module and arguments: `Burn!Shop.Pricing.Quote.Total(int32)` is
/// `Shop.Pricing.Quote.Total`.
fn bare(name: &str) -> &str {
    let name = name.split_once('!').map_or(name, |(_, name)| name);
    name.split_once('(').map_or(name, |(name, _)| name)
}

/// A frame as the chart says it: its type and method (`Quote.Total`).
pub fn short(name: &str) -> String {
    let parts: Vec<&str> = bare(name).split('.').collect();
    parts[parts.len().saturating_sub(2)..].join(".")
}

/// A frame's namespace, two deep (`Shop.Pricing`, `Microsoft.EntityFrameworkCore`); its module when it has none.
pub fn namespace(name: &str) -> String {
    let parts: Vec<&str> = bare(name).split('.').collect();
    let space = &parts[..parts.len().saturating_sub(2)];
    match space {
        [] => name.split_once('!').map_or(name, |(module, _)| module).to_string(),
        _ => space.iter().take(2).copied().collect::<Vec<_>>().join("."),
    }
}

impl Profile {
    fn new(app: &str, at: &str, seconds: u64) -> Self {
        let root = Node::default();
        Self {
            app: app.into(),
            at: at.into(),
            seconds,
            frames: vec![ROOT.into()],
            nodes: vec![root],
            sources: Vec::new(),
        }
    }

    /// Its time on the CPU, in ms.
    pub fn total(&self) -> f64 {
        self.nodes.first().map_or(0.0, |root| root.total)
    }

    pub fn name(&self, node: usize) -> &str {
        &self.frames[self.nodes[node].frame]
    }

    /// Where a node's frame is in the code, when the profile says it.
    pub fn source(&self, node: usize) -> Option<&(String, u32)> {
        self.sources.get(self.nodes[node].frame)?.as_ref()
    }

    /// `ms` spent in the frames of `path`, the caller first.
    fn add(&mut self, path: &[&str], ms: f64, interned: &mut HashMap<String, usize>) {
        let mut node = 0;
        self.nodes[0].total += ms;
        for name in path {
            let frame = *interned.entry((*name).to_string()).or_insert_with(|| {
                self.frames.push((*name).to_string());
                self.frames.len() - 1
            });
            let child = self.nodes[node].children.iter().copied().find(|&child| self.nodes[child].frame == frame);
            node = child.unwrap_or_else(|| {
                let parent = Some(node);
                self.nodes.push(Node { frame, parent, ..Node::default() });
                let added = self.nodes.len() - 1;
                self.nodes[node].children.push(added);
                added
            });
            self.nodes[node].total += ms;
        }
        self.nodes[node].own += ms;
    }

    /// The frames by self time, the most first: each frame's time in itself summed, and in all - counted once
    /// where it calls itself.
    pub fn top(&self, count: usize) -> Vec<Hot> {
        let mut by_name: HashMap<&str, (f64, f64)> = HashMap::new();
        for (at, node) in self.nodes.iter().enumerate().skip(1) {
            let name = self.frames[node.frame].as_str();
            let entry = by_name.entry(name).or_default();
            entry.0 += node.own;
            if !self.called_by_itself(at) {
                entry.1 += node.total;
            }
        }
        let mut hot: Vec<Hot> =
            by_name.into_iter().map(|(name, (own, total))| Hot { name: name.to_string(), own, total }).collect();
        hot.sort_by(|a, b| b.own.total_cmp(&a.own).then_with(|| a.name.cmp(&b.name)));
        hot.truncate(count);
        hot
    }

    fn called_by_itself(&self, at: usize) -> bool {
        let frame = self.nodes[at].frame;
        let mut caller = self.nodes[at].parent;
        while let Some(node) = caller {
            if self.nodes[node].frame == frame {
                return true;
            }
            caller = self.nodes[node].parent;
        }
        false
    }

    /// The time in itself by namespace, the most first.
    pub fn namespaces(&self) -> Vec<(String, f64)> {
        let mut by: HashMap<String, f64> = HashMap::new();
        for node in self.nodes.iter().skip(1).filter(|node| node.own > 0.0) {
            *by.entry(namespace(&self.frames[node.frame])).or_default() += node.own;
        }
        let mut split: Vec<(String, f64)> = by.into_iter().collect();
        split.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        split
    }

    /// How much more (or less) time each frame spent in itself than in `before`, by name.
    pub fn compare(&self, before: &Profile) -> HashMap<String, f64> {
        let own = |profile: &Profile| {
            let mut by: HashMap<String, f64> = HashMap::new();
            for node in profile.nodes.iter().skip(1) {
                *by.entry(profile.frames[node.frame].clone()).or_default() += node.own;
            }
            by
        };
        let (now, then) = (own(self), own(before));
        let mut change: HashMap<String, f64> =
            now.iter().map(|(name, ms)| (name.clone(), ms - then.get(name).copied().unwrap_or(0.0))).collect();
        for (name, ms) in then {
            change.entry(name).or_insert(-ms);
        }
        change
    }
}

/// A stack's own frames: those above its thread's frame, without the `CPU_TIME` leaf.
fn own_frames<'a>(stack: &[usize], names: &'a [String]) -> Vec<&'a str> {
    let from = stack.iter().rposition(|&frame| names[frame].starts_with(THREAD)).map_or(0, |at| at + 1);
    stack[from..].iter().map(|&frame| names[frame].as_str()).filter(|name| *name != CPU).collect()
}

/// A sampled profile's stacks on the CPU into `profile`.
fn add_samples(profile: &mut Profile, thread: &Value, names: &[String], interned: &mut HashMap<String, usize>) {
    let weights = thread["weights"].as_array().cloned().unwrap_or_default();
    for (sample, weight) in thread["samples"].as_array().into_iter().flatten().zip(weights) {
        let frames = sample.as_array().into_iter().flatten().filter_map(Value::as_u64).map(|frame| frame as usize);
        let stack: Vec<usize> = frames.filter(|&frame| frame < names.len()).collect();
        if stack.last().is_some_and(|&frame| names[frame] == CPU) {
            profile.add(&own_frames(&stack, names), weight.as_f64().unwrap_or(0.0), interned);
        }
    }
}

/// An evented profile's stretches on the CPU into `profile`.
fn add_events(profile: &mut Profile, thread: &Value, names: &[String], interned: &mut HashMap<String, usize>) {
    let mut stack: Vec<usize> = Vec::new();
    let mut last = 0.0;
    for event in thread["events"].as_array().into_iter().flatten() {
        let at = event["at"].as_f64().unwrap_or(last);
        if stack.last().is_some_and(|&frame| names[frame] == CPU) && at > last {
            profile.add(&own_frames(&stack, names), at - last, interned);
        }
        last = at;
        let Some(frame) = event["frame"].as_u64().map(|frame| frame as usize).filter(|&frame| frame < names.len())
        else {
            continue;
        };
        match event["type"].as_str() {
            Some("O") => stack.push(frame),
            Some("C") => {
                if let Some(at) = stack.iter().rposition(|&open| open == frame) {
                    stack.truncate(at);
                }
            }
            _ => {}
        }
    }
}

/// A capture from dotnet-trace's Speedscope JSON: its threads' time on the CPU as one call tree.
pub fn from_speedscope(text: &str, app: &str, at: &str, seconds: u64) -> Result<Profile, String> {
    let json: Value =
        serde_json::from_str(text).map_err(|error| format!("its Speedscope file didn't read: {error}"))?;
    let names: Vec<String> = json["shared"]["frames"]
        .as_array()
        .ok_or("its Speedscope file has no frames")?
        .iter()
        .map(|frame| frame["name"].as_str().unwrap_or("?").to_string())
        .collect();
    let mut profile = Profile::new(app, at, seconds);
    let mut interned = HashMap::new();
    for thread in json["profiles"].as_array().into_iter().flatten() {
        if thread["type"] == "sampled" {
            add_samples(&mut profile, thread, &names, &mut interned);
        } else {
            add_events(&mut profile, thread, &names, &mut interned);
        }
    }
    if profile.total() <= 0.0 {
        return Err("no time on the CPU in it: the app was idle while it was profiled".into());
    }
    Ok(profile)
}

/// A JavaScript frame's module: its package (`node_modules/express/...` is `express`), `node` for Node's own, else
/// its file's name.
fn module_of(url: &str) -> String {
    if let Some((_, rest)) = url.rsplit_once("node_modules/") {
        let mut parts = rest.split('/');
        let first = parts.next().unwrap_or_default();
        return if first.starts_with('@') {
            format!("{first}/{}", parts.next().unwrap_or_default())
        } else {
            first.into()
        };
    }
    if url.starts_with("node:") || url.is_empty() {
        return "node".into();
    }
    url.rsplit(['/', '\\']).next().unwrap_or(url).to_string()
}

/// A `file://` URL's path, as the editor opens it.
fn file_of(url: &str) -> Option<String> {
    let path =
        url.strip_prefix("file:///").map(str::to_string).or_else(|| url.strip_prefix("file://").map(str::to_string))?;
    Some(path.replace("%20", " "))
}

/// A capture from Node's own `.cpuprofile` (the inspector's): each sample's stack, its time the gap before it; idle
/// time left out.
pub fn from_cpuprofile(text: &str, app: &str, at: &str, seconds: u64) -> Result<Profile, String> {
    let json: Value = serde_json::from_str(text).map_err(|error| format!("its profile didn't read: {error}"))?;
    if let Some(error) = json["error"].as_str() {
        return Err(format!("Node couldn't profile it: {error}"));
    }
    let nodes = json["nodes"].as_array().ok_or("its profile has no nodes")?;
    let mut named: HashMap<u64, (String, Option<(String, u32)>)> = HashMap::new();
    let mut parents: HashMap<u64, u64> = HashMap::new();
    for node in nodes {
        let id = node["id"].as_u64().unwrap_or_default();
        let frame = &node["callFrame"];
        let url = frame["url"].as_str().unwrap_or_default();
        let function = frame["functionName"].as_str().filter(|name| !name.is_empty()).unwrap_or("(anonymous)");
        let line = frame["lineNumber"].as_u64().map_or(0, |line| line as u32 + 1);
        let source = file_of(url).map(|file| (file, line));
        named.insert(id, (format!("{}!{function}", module_of(url)), source));
        for child in node["children"].as_array().into_iter().flatten().filter_map(Value::as_u64) {
            parents.insert(child, id);
        }
    }
    let mut profile = Profile::new(app, at, seconds);
    let mut interned = HashMap::new();
    let mut sources: HashMap<String, (String, u32)> = HashMap::new();
    let deltas = json["timeDeltas"].as_array().cloned().unwrap_or_default();
    for (sample, delta) in json["samples"].as_array().into_iter().flatten().zip(deltas) {
        let mut stack = Vec::new();
        let mut at = sample.as_u64();
        while let Some(id) = at {
            stack.push(id);
            at = parents.get(&id).copied();
        }
        stack.reverse();
        let path: Vec<&str> = stack
            .iter()
            .filter_map(|id| named.get(id))
            .map(|(name, _)| name.as_str())
            .filter(|name| !name.ends_with("!(root)"))
            .collect();
        if path.last().is_none_or(|leaf| leaf.ends_with("!(idle)")) {
            continue;
        }
        for (name, source) in stack.iter().filter_map(|id| named.get(id)) {
            if let Some(source) = source {
                sources.entry(name.clone()).or_insert_with(|| source.clone());
            }
        }
        profile.add(&path, delta.as_f64().unwrap_or(0.0).max(0.0) / 1000.0, &mut interned);
    }
    if profile.total() <= 0.0 {
        return Err("no time on the CPU in it: the app was idle while it was profiled".into());
    }
    profile.sources = profile.frames.iter().map(|name| sources.get(name).cloned()).collect();
    Ok(profile)
}

/// How long a Node process has to pick a request up before it's taken as one without lazyrunner's hook.
const PICKED_UP: std::time::Duration = std::time::Duration::from_secs(4);
/// How long past the capture its file may take to be written.
const WRITTEN: std::time::Duration = std::time::Duration::from_secs(10);
const POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// Asks Node process `pid` - its lazyrunner hook, through a file in `dir` - to sample itself for `seconds`: its
/// `.cpuprofile`.
pub fn ask_node(dir: &Path, pid: u32, seconds: u64, stopped: &AtomicBool) -> Result<String, String> {
    let asking = dir.join(format!("profile-request.{pid}.json"));
    let out = dir.join(format!("profile-out.{pid}.cpuprofile"));
    let _ = std::fs::remove_file(&out);
    let request = serde_json::json!({ "seconds": seconds, "out": out.to_string_lossy() });
    std::fs::write(&asking, request.to_string()).map_err(|error| error.to_string())?;
    let started = std::time::Instant::now();
    let deadline = std::time::Duration::from_secs(seconds) + WRITTEN;
    loop {
        if stopped.load(Ordering::SeqCst) {
            let _ = std::fs::remove_file(&asking);
            return Err("stopped".into());
        }
        if let Some(text) = std::fs::read_to_string(&out).ok().filter(|text| !text.is_empty()) {
            let _ = std::fs::remove_file(&out);
            return Ok(text);
        }
        if asking.exists() && started.elapsed() > PICKED_UP {
            let _ = std::fs::remove_file(&asking);
            return Err("its process didn't answer - restart the app: lazyrunner's Node hook loads as it starts".into());
        }
        if started.elapsed() > deadline {
            return Err("its profile wasn't written in time".into());
        }
        std::thread::sleep(POLL);
    }
}

/// The folder captures are kept in, in the project's state folder.
fn folder(state: &Path) -> PathBuf {
    state.join("profiles")
}

/// Keeps `profile` with its app's others, the oldest beyond [`KEPT`] let go.
pub fn save(state: &Path, profile: &Profile) -> Result<(), String> {
    let folder = folder(state);
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let stamp = profile.at.replace([':', '+'], "-");
    let text = serde_json::to_string(profile).map_err(|error| error.to_string())?;
    std::fs::write(folder.join(format!("{}.{stamp}.json", profile.app)), text).map_err(|error| error.to_string())?;
    let mine = files(&folder, &profile.app);
    for old in mine.iter().take(mine.len().saturating_sub(KEPT)) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// An app's capture files, the oldest first.
fn files(folder: &Path, app: &str) -> Vec<PathBuf> {
    let prefix = format!("{app}.");
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(&prefix)))
        .collect();
    found.sort();
    found
}

/// An app's kept captures, the oldest first.
pub fn load(state: &Path, app: &str) -> Vec<Profile> {
    let read = |path: &PathBuf| serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok();
    files(&folder(state), app).iter().filter_map(read).collect()
}

/// dotnet-trace: on PATH, else where `dotnet tool install -g` puts it.
pub fn tool() -> Option<PathBuf> {
    super::tools::find_on_path(TOOL).or_else(|| {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        let name = if cfg!(windows) { format!("{TOOL}.exe") } else { TOOL.to_string() };
        Some(PathBuf::from(home).join(".dotnet").join("tools").join(name)).filter(|path| path.is_file())
    })
}

/// Runs `tool` with `args`, its pid in `running` while it runs (a cancel kills it): what it said, and whether it went
/// fine.
fn run(tool: &Path, args: &[&str], running: &AtomicU32) -> Result<(String, bool), String> {
    let child = Command::new(tool)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("dotnet-trace didn't start: {error}"))?;
    running.store(child.id(), Ordering::SeqCst);
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    running.store(0, Ordering::SeqCst);
    let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    Ok((said, output.status.success()))
}

/// The last line `said` worth showing.
fn last_line(said: &str) -> String {
    said.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("it said nothing").to_string()
}

/// Samples process `pid` for `seconds` with `tool` into `dir`, then converts the trace: its Speedscope JSON.
pub fn capture(tool: &Path, pid: u32, seconds: u64, dir: &Path, running: &Arc<AtomicU32>) -> Result<String, String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let base = dir.join(format!("capture-{pid}"));
    let trace = base.with_extension("nettrace");
    let length = format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60);
    let (pid_text, trace_text) = (pid.to_string(), trace.to_string_lossy().into_owned());
    let mut said = String::new();
    for profile in PROFILES {
        let args =
            ["collect", "--process-id", &pid_text, "--profile", profile, "--duration", &length, "-o", &trace_text];
        let (output, fine) = run(tool, &args, running)?;
        said = output;
        if NO_SUCH_PROFILE.iter().any(|sign| said.contains(sign)) {
            continue;
        }
        if !fine {
            return Err(format!("dotnet-trace couldn't sample it: {}", last_line(&said)));
        }
        break;
    }
    if !trace.is_file() {
        return Err(format!("dotnet-trace couldn't sample it: {}", last_line(&said)));
    }
    let base_text = base.to_string_lossy().into_owned();
    let (said, fine) = run(tool, &["convert", "--format", "Speedscope", &trace_text, "-o", &base_text], running)?;
    let json = base.with_extension("speedscope.json");
    let text = std::fs::read_to_string(&json);
    let _ = std::fs::remove_file(&trace);
    let _ = std::fs::remove_file(&json);
    match text {
        Ok(text) if fine => Ok(text),
        _ => Err(format!("dotnet-trace couldn't convert its trace: {}", last_line(&said))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Main → Quote.Total on the CPU 30 ms, then DateTime.get_Now 10 ms, and a waiting thread that's left out.
    const SAMPLE: &str = r#"{
        "shared": { "frames": [
            { "name": "Process64 Burn (1) Args:  " }, { "name": "(Non-Activities)" }, { "name": "Threads" },
            { "name": "Thread (7)" }, { "name": "Burn!Shop.Pricing.Program.Main()" },
            { "name": "Burn!Shop.Pricing.Quote.Total(int32)" }, { "name": "CPU_TIME" },
            { "name": "System.Private.CoreLib.il!System.DateTime.get_Now()" }, { "name": "BLOCKED_TIME" },
            { "name": "Thread (8)" }
        ] },
        "profiles": [
            { "type": "evented", "events": [
                { "type": "O", "frame": 0, "at": 0 }, { "type": "O", "frame": 1, "at": 0 },
                { "type": "O", "frame": 2, "at": 0 }, { "type": "O", "frame": 3, "at": 0 },
                { "type": "O", "frame": 4, "at": 0 }, { "type": "O", "frame": 5, "at": 0 },
                { "type": "O", "frame": 6, "at": 0 }, { "type": "C", "frame": 6, "at": 30 },
                { "type": "C", "frame": 5, "at": 30 }, { "type": "O", "frame": 7, "at": 30 },
                { "type": "O", "frame": 6, "at": 30 }, { "type": "C", "frame": 6, "at": 40 },
                { "type": "C", "frame": 7, "at": 40 }, { "type": "C", "frame": 4, "at": 40 }
            ] },
            { "type": "evented", "events": [
                { "type": "O", "frame": 9, "at": 0 }, { "type": "O", "frame": 8, "at": 0 },
                { "type": "C", "frame": 8, "at": 500 }, { "type": "C", "frame": 9, "at": 500 }
            ] }
        ]
    }"#;

    #[test]
    fn a_traces_time_on_the_cpu_is_one_call_tree_with_its_hottest_frames_and_namespaces() {
        let profile = from_speedscope(SAMPLE, "orders", "2026-11-12T12:00:00+01:00", 10).unwrap();
        assert_eq!(profile.total(), 40.0, "the waiting thread isn't CPU time");
        let main = profile.nodes[0].children[0];
        assert_eq!((profile.name(main), profile.nodes[main].total), ("Burn!Shop.Pricing.Program.Main()", 40.0));
        let top = profile.top(3);
        assert_eq!(short(&top[0].name), "Quote.Total");
        assert_eq!((top[0].own, top[1].own), (30.0, 10.0));
        assert_eq!(profile.namespaces(), [("Shop.Pricing".to_string(), 30.0), ("System".to_string(), 10.0)]);
        let ef = "Microsoft.EntityFrameworkCore.dll!Microsoft.EntityFrameworkCore.Query.Run()";
        assert_eq!(namespace(ef), "Microsoft.EntityFrameworkCore");
    }

    #[test]
    fn a_node_process_is_asked_through_a_file_and_its_answer_read_back() {
        let temp = crate::testing::TempDir::new("ask-node");
        let dir = temp.path().to_path_buf();
        let hook = dir.clone();
        let answering = std::thread::spawn(move || {
            let asking = hook.join("profile-request.4242.json");
            while !asking.exists() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let asked: Value = serde_json::from_str(&std::fs::read_to_string(&asking).unwrap()).unwrap();
            std::fs::remove_file(&asking).unwrap();
            assert_eq!(asked["seconds"], 3);
            std::fs::write(asked["out"].as_str().unwrap(), r#"{ "nodes": [] }"#).unwrap();
        });
        let answer = ask_node(&dir, 4242, 3, &AtomicBool::new(false)).unwrap();
        answering.join().unwrap();
        assert_eq!(answer, r#"{ "nodes": [] }"#);
        assert!(!dir.join("profile-out.4242.cpuprofile").exists(), "read and cleaned up");
    }

    #[test]
    fn a_node_profile_is_its_samples_stacks_idle_left_out_each_frame_with_its_file() {
        let text = r#"{ "nodes": [
            { "id": 1, "callFrame": { "functionName": "(root)", "url": "", "lineNumber": -1 }, "children": [2, 4] },
            { "id": 2, "callFrame": { "functionName": "handle", "url": "file:///C:/shop/node_modules/express/lib/router.js", "lineNumber": 10 }, "children": [3] },
            { "id": 3, "callFrame": { "functionName": "quote", "url": "file:///C:/shop/src/pricing.js", "lineNumber": 41 }, "children": [] },
            { "id": 4, "callFrame": { "functionName": "(idle)", "url": "", "lineNumber": -1 }, "children": [] }
        ], "samples": [3, 3, 4, 2], "timeDeltas": [1000, 2000, 5000, 1000] }"#;
        let profile = from_cpuprofile(text, "shop", "2026-11-12T12:00:00+01:00", 10).unwrap();
        assert_eq!(profile.total(), 4.0, "idle isn't CPU time");
        let top = profile.top(2);
        assert_eq!((top[0].name.as_str(), top[0].own), ("pricing.js!quote", 3.0));
        assert_eq!(profile.namespaces()[0].0, "pricing.js");
        let quote = profile.nodes.iter().position(|node| profile.frames[node.frame] == "pricing.js!quote").unwrap();
        assert_eq!(profile.source(quote), Some(&("C:/shop/src/pricing.js".to_string(), 42)));
        assert_eq!(namespace("express!handle"), "express");
    }

    #[test]
    fn a_capture_compares_with_the_one_before_and_the_oldest_beyond_ten_go() {
        let temp = crate::testing::TempDir::new("profiles");
        let profile = from_speedscope(SAMPLE, "orders", "2026-11-12T12:00:00+01:00", 10).unwrap();
        let mut slower = profile.clone();
        let total = slower.nodes.iter().position(|node| slower.frames[node.frame].contains("Quote.Total")).unwrap();
        slower.nodes[total].own += 180.0;
        assert_eq!(slower.compare(&profile)["Burn!Shop.Pricing.Quote.Total(int32)"], 180.0);
        for minute in 0..12 {
            let at = format!("2026-11-12T12:{minute:02}:00+01:00");
            save(temp.path(), &Profile { at, ..profile.clone() }).unwrap();
        }
        let kept = load(temp.path(), "orders");
        assert_eq!(kept.len(), KEPT);
        assert!(kept[0].at.contains("12:02") && kept[9].at.contains("12:11"), "the oldest went: {}", kept[0].at);
        assert!(from_speedscope(r#"{ "shared": { "frames": [] }, "profiles": [] }"#, "orders", "", 10).is_err());
    }
}
