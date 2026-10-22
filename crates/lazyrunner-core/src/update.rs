//! Self-update from GitHub Releases: lazyrunner asks the release repo for its latest release and, when that is a
//! higher version, downloads this platform's archive, checks it against its .sha256, and puts the new binary in place
//! of the running one. In the startup checklist (or in the background when the checklist is off), when the checklist
//! runs again (, then r) and with `lazyrunner update`. Only a higher version counts. After an update lazyrunner
//! reopens on the new binary and the checklist shows what's new (the changelog is built into it).
//!
//! Where updates come from: `DEFAULT_REPO`; "updateRepoUrl" in lazyrunner.config.json overrides it on a machine, and
//! LAZYRUNNER_UPDATE_REPO overrides both ("none": no updates). A release is what cargo-dist publishes: a tag
//! (`v1.2.3`), an archive per platform (`lazyrunner-<target>.zip` on Windows, `.tar.gz` elsewhere) and its `.sha256`.
//!
//! What it trusts: HTTPS to the release host (redirects must stay on HTTPS). The .sha256 comes from the same release
//! as the archive, so it catches a broken or cut-off download, not someone who can publish releases.

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::changelog;
use crate::json;

/// Where releases come from unless the config or LAZYRUNNER_UPDATE_REPO say otherwise.
pub const DEFAULT_REPO: &str = "https://github.com/momcilovicluka/lazyrunner";
/// The platform this binary was built for (from build.rs).
pub const TARGET: &str = env!("LAZYRUNNER_TARGET");

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Checking takes one small request; a release repo that doesn't answer by then is unreachable.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// The release feed is a few KB; more is not GitHub's feed.
const FEED_BYTES: u64 = 1024 * 1024;
/// The archive itself may take a while on a slow line.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// GitHub sends release files from another host: a redirect or two.
const MAX_REDIRECTS: usize = 5;
/// Neither the archive nor the binary in it is anywhere near this; more is not a lazyrunner release.
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const PROGRESS_BAR_CELLS: usize = 10;
/// A step that takes longer than this shows its seconds.
const SHOW_SECONDS_AFTER: Duration = Duration::from_secs(3);
const CHUNK: usize = 64 * 1024;
/// The new binary has this long to answer `--version` (a first run is slow while antivirus scans it).
const SMOKE_TIMEOUT: Duration = Duration::from_secs(20);

/// The version this process installed, if it did (the UI reopens on it).
static INSTALLED: Mutex<Option<String>> = Mutex::new(None);
/// One update at a time in this process (, then r while the startup one still runs).
static UPDATING: AtomicBool = AtomicBool::new(false);
/// The binary as it was when lazyrunner started (on Linux the replaced one reads as "… (deleted)" afterwards).
static EXE: OnceLock<Option<PathBuf>> = OnceLock::new();

type Response = ureq::http::Response<ureq::Body>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Skip,
}

/// What an update attempt found: ok / warn / skip, what to say, and the version installed if one was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub status: Status,
    pub detail: String,
    pub updated_to: Option<String>,
}

impl Outcome {
    fn new(status: Status, detail: String) -> Self {
        Self { status, detail, updated_to: None }
    }
}

/// The release repo: `https://github.com/<owner>/<repo>`, or a local stand-in for the tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub url: String,
}

impl Source {
    pub fn host(&self) -> String {
        let rest = self.url.split("://").nth(1).unwrap_or(&self.url);
        rest.split(['/', ':']).next().filter(|host| !host.is_empty()).unwrap_or("the release repo").to_string()
    }

    /// Plain HTTP only when the repo itself is (the tests' local stand-in); otherwise every hop is HTTPS.
    fn allows_http(&self) -> bool {
        self.url.starts_with("http://")
    }
}

/// A published release: its tag as it is (`v1.2.3` or `1.2.3`) and the version in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: String,
}

/// Where updates come from (`None`: nowhere, so no updates).
pub fn source(config: &Value) -> Option<Source> {
    let from_env = std::env::var("LAZYRUNNER_UPDATE_REPO").ok();
    let configured = json::text(json::field(config, "updateRepoUrl"));
    let url = from_env.unwrap_or_else(|| if configured.is_empty() { DEFAULT_REPO.to_string() } else { configured });
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git").to_string();
    (!url.is_empty() && url != "none").then_some(Source { url })
}

/// The version this process installed (it runs the old one until it reopens).
pub fn installed() -> Option<String> {
    INSTALLED.lock().map(|installed| installed.clone()).unwrap_or_default()
}

/// This binary's path, as it was the first time anyone asked.
pub fn exe() -> Option<PathBuf> {
    EXE.get_or_init(|| std::env::current_exe().ok()).clone()
}

/// The archive of this platform's release. Windows builds of any flavour take the MSVC one (the binary stands alone).
pub fn asset_name(target: &str) -> String {
    let target = target.replace("-windows-gnullvm", "-windows-msvc").replace("-windows-gnu", "-windows-msvc");
    let extension = if target.contains("-windows-") { "zip" } else { "tar.gz" };
    format!("lazyrunner-{target}.{extension}")
}

/// The binary's name inside the archive.
fn binary_name() -> &'static str {
    if cfg!(windows) { "lazyrunner.exe" } else { "lazyrunner" }
}

/// How `remote` compares with `local`: by their numbers, then a release above any pre-release of it
/// (`0.2.0` > `0.2.0-dev`). `None` when either isn't a version.
pub fn compare(remote: &str, local: &str) -> Option<Ordering> {
    let numbers = |text: &str| changelog::version(text);
    let pre = |text: &str| {
        let text = text.trim().trim_start_matches('v');
        let core_len = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
        let rest = &text[core_len..];
        rest.strip_prefix('-').map(str::to_string).filter(|_| !rest.is_empty())
    };
    let by_numbers = numbers(remote)?.cmp(&numbers(local)?);
    Some(by_numbers.then_with(|| match (pre(remote), pre(local)) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => a.cmp(&b),
    }))
}

// ---------------------------------------------------------------- progress

/// What the update is doing, for the checklist's lazyrunner line or `lazyrunner update`:
/// checking github.com… → downloading ▕████░░░░░░▏ 40% · 3.1 MB → installing v0.2.1… → updated v0.2.0 → v0.2.1
pub struct Progress<'a> {
    report: &'a mut dyn FnMut(&str),
    text: String,
    percent: Option<u64>,
    size: String,
    since: Instant,
}

impl<'a> Progress<'a> {
    pub fn new(report: &'a mut dyn FnMut(&str)) -> Self {
        Self { report, text: String::new(), percent: None, size: String::new(), since: Instant::now() }
    }

    /// A new step is shown at once; within a step the bar only moves forward.
    fn step(&mut self, text: &str, percent: Option<u64>, size: Option<String>) {
        if self.text != text {
            self.text = text.to_string();
            self.percent = None;
            self.size.clear();
            self.since = Instant::now();
        }
        if let Some(percent) = percent {
            self.percent = Some(self.percent.unwrap_or(0).max(percent.min(100)));
        }
        if let Some(size) = size {
            self.size = size;
        }
        let line = self.line();
        (self.report)(&line);
    }

    fn line(&self) -> String {
        let mut line = self.text.clone();
        if let Some(percent) = self.percent {
            let filled = (percent as usize * PROGRESS_BAR_CELLS) / 100;
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(PROGRESS_BAR_CELLS - filled));
            line.push_str(&format!(" ▕{bar}▏ {percent}%"));
        }
        if !self.size.is_empty() {
            line.push_str(&format!(" · {}", self.size));
        }
        let elapsed = self.since.elapsed();
        if elapsed >= SHOW_SECONDS_AFTER {
            line.push_str(&format!(" · {}s", elapsed.as_secs()));
        }
        line
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

// ---------------------------------------------------------------- the update

/// Lets the next update run once this one is over, however it ends.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        UPDATING.store(false, AtomicOrdering::SeqCst);
    }
}

/// Checks and, when a higher version is out, installs it. `force`: even with "autoUpdate" off (`lazyrunner update`).
pub fn update(config: &Value, state_dir: &Path, force: bool, progress: &mut Progress) -> Outcome {
    let version = changelog::VERSION;
    if !force && !crate::config::Toggle::AutoUpdate.read(config) {
        return Outcome::new(Status::Skip, format!("v{version} · auto-update off (, then v turns it on)"));
    }
    let Some(source) = source(config) else {
        return Outcome::new(Status::Skip, format!("v{version} · no release repo configured"));
    };
    if let Some(why) = not_updatable() {
        return Outcome::new(Status::Skip, format!("v{version} · {why}"));
    }
    if UPDATING.swap(true, AtomicOrdering::SeqCst) {
        return Outcome::new(Status::Skip, format!("v{version} · an update is already running"));
    }
    let _running = Running;
    progress.step(&format!("checking {}…", source.host()), None, None);
    let release = match latest(&source) {
        Ok(release) => release,
        Err(error) => return Outcome::new(Status::Warn, format!("v{version} · couldn't check for updates: {error}")),
    };
    let latest = release.version.clone();
    if let Some(installed) = installed()
        && compare(&latest, &installed).is_some_and(|order| order != Ordering::Greater)
    {
        return Outcome::new(Status::Ok, format!("v{installed} is installed - it runs once lazyrunner reopens"));
    }
    match compare(&latest, version) {
        None => {
            let detail = format!("v{version} · couldn't check for updates: no version in the tag {}", release.tag);
            return Outcome::new(Status::Warn, detail);
        }
        Some(Ordering::Less) => {
            return Outcome::new(Status::Ok, format!("v{version} · ahead of the released v{latest}"));
        }
        Some(Ordering::Equal) => return Outcome::new(Status::Ok, format!("v{version} · up to date")),
        Some(Ordering::Greater) => {}
    }
    if let Err(problem) = install(&source, &release, state_dir, progress) {
        return Outcome::new(Status::Warn, format!("v{version} · v{latest} is out, but not installed: {problem}"));
    }
    log(state_dir, &format!("updated lazyrunner v{version} -> v{latest}"));
    if let Ok(mut installed) = INSTALLED.lock() {
        *installed = Some(latest.clone());
    }
    Outcome { status: Status::Ok, detail: format!("updated v{version} → v{latest}"), updated_to: Some(latest) }
}

/// Why this binary isn't one to replace: a package manager's (it updates it) or a developer's cargo build.
fn not_updatable() -> Option<String> {
    if std::env::var_os("LAZYRUNNER_UPDATE_REPO").is_some() {
        return None; // the tests point this at a local release repo on purpose
    }
    managed_by(&exe()?)
}

/// Who looks after the binary at `exe`, when it isn't lazyrunner itself.
pub fn managed_by(exe: &Path) -> Option<String> {
    let parts: Vec<String> = exe.components().map(|part| part.as_os_str().to_string_lossy().to_lowercase()).collect();
    let has = |a: &str, b: &str| parts.windows(2).any(|pair| pair[0] == a && pair[1] == b);
    let any = |name: &str| parts.iter().any(|part| part == name);
    let why = if any("cellar") || any("homebrew") || any("linuxbrew") {
        "installed by Homebrew - brew upgrade lazyrunner updates it"
    } else if has("scoop", "apps") {
        "installed by Scoop - scoop update lazyrunner updates it"
    } else if has("winget", "packages") {
        "installed by winget - winget upgrade lazyrunner updates it"
    } else if has("target", "debug") || has("target", "release") || has("target", "dist") {
        "a development build (cargo) - not updated"
    } else {
        return None;
    };
    Some(why.to_string())
}

/// Whether lazyrunner can put a new binary in `dir` (not, say, in /usr/local/bin without root).
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".lazyrunner-update-{}", std::process::id()));
    let ok = fs::OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

fn agent(timeout: Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .http_status_as_error(false)
        .user_agent(format!("lazyrunner/{}", changelog::VERSION))
        .build()
        .new_agent()
}

/// What a failed request means for the person reading the checklist.
fn request_error(error: &ureq::Error, source: &Source) -> String {
    match error {
        ureq::Error::Timeout(_) | ureq::Error::Io(_) | ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => {
            format!("can't reach {} (offline?)", source.host())
        }
        other => other.to_string(),
    }
}

/// What an answer that isn't the expected one means.
fn status_error(status: u16, what: &str, source: &Source) -> String {
    match status {
        403 | 429 => {
            format!("{} refused ({status}: too many checks from this network?) - try again later", source.host())
        }
        _ => format!("{what}: {} answered {status}", source.host()),
    }
}

fn location(response: &Response) -> String {
    response.headers().get("location").and_then(|value| value.to_str().ok()).unwrap_or_default().to_string()
}

/// Where a redirect from `current` to `location` goes: relative ones resolved, nothing but HTTPS (and HTTP when
/// `allow_http`).
pub fn resolve(current: &str, location: &str, allow_http: bool) -> Result<String, String> {
    let location = location.trim();
    let target = if location.contains("://") {
        location.to_string()
    } else if let Some(path) = location.strip_prefix('/') {
        let (scheme, rest) = current.split_once("://").ok_or_else(|| format!("not a URL: {current}"))?;
        let host = rest.split('/').next().unwrap_or(rest);
        format!("{scheme}://{host}/{path}")
    } else {
        return Err(format!("a redirect to nowhere ({location:?})"));
    };
    let secure = target.starts_with("https://") || (allow_http && target.starts_with("http://"));
    if secure { Ok(target) } else { Err(format!("a redirect off HTTPS ({target})")) }
}

/// The newest release: the highest version the release feed lists. GitHub's "Latest" is only the release published
/// last - not the highest one when several tags go out at once and their builds finish in another order - so it is
/// the fallback, for when there is no feed to read.
pub fn latest(source: &Source) -> Result<Release, String> {
    match newest_in_feed(source) {
        Some(release) => Ok(release),
        None => marked_latest(source),
    }
}

/// The highest stable release in the repo's `releases.atom` (it isn't rate-limited like GitHub's API).
fn newest_in_feed(source: &Source) -> Option<Release> {
    let response = get(&format!("{}/releases.atom", source.url), source, CHECK_TIMEOUT).ok()?;
    let feed = response.into_body().with_config().limit(FEED_BYTES).read_to_string().ok()?;
    newest(&feed)
}

/// The highest version among a feed's `…/releases/tag/<tag>` links, pre-releases left out.
pub fn newest(feed: &str) -> Option<Release> {
    feed.split("/releases/tag/")
        .skip(1)
        .filter_map(|rest| tag_of(&format!("/releases/tag/{rest}")))
        .filter(|release| !release.version.contains('-'))
        .max_by(|a, b| compare(&a.version, &b.version).unwrap_or(Ordering::Equal))
}

/// The release GitHub marks "Latest", from where it redirects `releases/latest` (`…/releases/tag/v1.2.3`).
fn marked_latest(source: &Source) -> Result<Release, String> {
    let mut url = format!("{}/releases/latest", source.url);
    for _ in 0..MAX_REDIRECTS {
        let response = agent(CHECK_TIMEOUT).get(&url).call().map_err(|error| request_error(&error, source))?;
        let status = response.status().as_u16();
        let next = location(&response);
        if let Some(release) = tag_of(&next) {
            return Ok(release);
        }
        match status {
            404 => return Err(format!("no releases at {} (or it is private)", source.url)),
            300..=399 => url = resolve(&url, &next, source.allows_http())?, // the repo moved
            _ => return Err(status_error(status, "the latest release", source)),
        }
    }
    Err(format!("{}: too many redirects", source.url))
}

/// "https://github.com/o/r/releases/tag/v1.2.3" → tag "v1.2.3", version "1.2.3".
pub fn tag_of(location: &str) -> Option<Release> {
    let tag = location.split("/releases/tag/").nth(1)?.split(['?', '#', '/', '"', '<']).next()?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    changelog::version(version).map(|_| Release { tag: tag.to_string(), version: version.to_string() })
}

/// GETs a release file, following the redirects by hand (the agent follows none, so `latest` can read its Location).
fn get(url: &str, source: &Source, timeout: Duration) -> Result<Response, String> {
    let name = url.rsplit('/').next().unwrap_or(url).to_string();
    let mut url = url.to_string();
    for _ in 0..MAX_REDIRECTS {
        let response = agent(timeout).get(&url).call().map_err(|error| request_error(&error, source))?;
        let status = response.status().as_u16();
        match status {
            300..=399 => url = resolve(&url, &location(&response), source.allows_http())?,
            404 => return Err(format!("the release has no {name}")),
            400.. => return Err(status_error(status, &name, source)),
            _ => return Ok(response),
        }
    }
    Err(format!("{name}: too many redirects"))
}

/// A folder of this run's own for the download (two lazyrunners updating at once don't share files); gone after.
struct Scratch(PathBuf);

impl Scratch {
    fn new(state_dir: &Path) -> Result<Self, String> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        let dir = state_dir.join("update").join(format!("{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|error| format!("couldn't prepare {} - {error}", dir.display()))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Downloads, checks and puts the release's binary in place of this one.
fn install(source: &Source, release: &Release, state_dir: &Path, progress: &mut Progress) -> Result<(), String> {
    let exe = exe().ok_or("can't find lazyrunner's own path")?;
    if let Some(dir) = exe.parent().filter(|dir| !writable(dir)) {
        return Err(format!("can't write to {} - update it the way you installed it", dir.display()));
    }
    let asset = asset_name(TARGET);
    let base = format!("{}/releases/download/{}", source.url, release.tag);
    let scratch = Scratch::new(state_dir)?;
    let checksums = get(&format!("{base}/{asset}.sha256"), source, CHECK_TIMEOUT)?
        .into_body()
        .with_config()
        .limit(4096)
        .read_to_string()
        .map_err(|error| request_error(&error, source))?;
    let expected = checksum_of(&checksums).ok_or_else(|| format!("{asset}.sha256 has no checksum in it"))?;
    let archive = scratch.0.join(&asset);
    download(&format!("{base}/{asset}"), &archive, source, progress)?;
    let actual = sha256_of(&archive).map_err(|error| format!("couldn't read the download - {error}"))?;
    if actual != expected {
        return Err(format!("the download doesn't match its checksum ({asset})"));
    }
    progress.step(&format!("installing v{}…", release.version), None, None);
    let binary = scratch.0.join(binary_name());
    extract(&archive, &binary).map_err(|error| format!("couldn't unpack {asset} - {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("couldn't make the new lazyrunner runnable - {error}"))?;
    }
    runs(&binary)?;
    self_replace::self_replace(&binary).map_err(|error| format!("couldn't replace lazyrunner - {error}"))
}

/// The new binary must run before it replaces this one: a damaged, blocked or wrong-platform one would end the
/// session that switches to it (and the apps handed over to it), and then stay in place.
fn runs(binary: &Path) -> Result<(), String> {
    let mut child = std::process::Command::new(binary)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("the new lazyrunner doesn't start - {error}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < SMOKE_TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the new lazyrunner didn't answer --version".into());
            }
        }
    };
    let mut printed = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut printed);
    }
    if status.success() && printed.starts_with("lazyrunner ") {
        Ok(())
    } else {
        Err(format!("the new lazyrunner doesn't run properly ({status})"))
    }
}

fn download(url: &str, to: &Path, source: &Source, progress: &mut Progress) -> Result<(), String> {
    progress.step("downloading", Some(0), None);
    let response = get(url, source, DOWNLOAD_TIMEOUT)?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if total.is_some_and(|total| total > MAX_BYTES) {
        return Err("the download is far too big for lazyrunner".into());
    }
    let mut reader = response.into_body().into_reader();
    let mut file = File::create(to).map_err(|error| format!("couldn't write {} - {error}", to.display()))?;
    let mut buffer = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let read = reader.read(&mut buffer).map_err(|_| format!("the download from {} broke off", source.host()))?;
        if read == 0 {
            break;
        }
        done += read as u64;
        if done > MAX_BYTES {
            return Err("the download is far too big for lazyrunner".into());
        }
        file.write_all(&buffer[..read]).map_err(|error| format!("couldn't write {} - {error}", to.display()))?;
        let percent = total.filter(|&total| total > 0).map(|total| done * 100 / total);
        progress.step("downloading", percent, Some(megabytes(done)));
    }
    progress.step("downloading", Some(100), Some(megabytes(done)));
    Ok(())
}

/// The checksum in a .sha256 file ("<hex>", "<hex>  name" or "<hex> *name"), lowercase.
pub fn checksum_of(text: &str) -> Option<String> {
    let hex = text.split_whitespace().next()?;
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_lowercase())
}

/// A file's SHA-256, lowercase hex.
pub fn sha256_of(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Writes the archive's lazyrunner binary (wherever in it) to `to`, a new file.
pub fn extract(archive: &Path, to: &Path) -> io::Result<()> {
    let wanted = binary_name();
    let is_wanted = |name: &Path| name.file_name().is_some_and(|file| file == wanted);
    let mut out = fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    let copied = if archive.extension().is_some_and(|extension| extension == "zip") {
        let mut zip = zip::ZipArchive::new(File::open(archive)?).map_err(io::Error::other)?;
        let mut copied = None;
        for index in 0..zip.len() {
            let entry = zip.by_index(index).map_err(io::Error::other)?;
            if entry.is_file() && entry.enclosed_name().is_some_and(|name| is_wanted(&name)) {
                copied = Some(io::copy(&mut entry.take(MAX_BYTES + 1), &mut out)?);
                break;
            }
        }
        copied
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(File::open(archive)?));
        let mut copied = None;
        for entry in tar.entries()? {
            let entry = entry?;
            if entry.header().entry_type().is_file() && is_wanted(&entry.path()?) {
                copied = Some(io::copy(&mut entry.take(MAX_BYTES + 1), &mut out)?);
                break;
            }
        }
        copied
    };
    drop(out);
    let problem = match copied {
        Some(bytes) if bytes <= MAX_BYTES => return Ok(()),
        Some(_) => format!("its {wanted} is far too big"),
        None => format!("no {wanted} in it"),
    };
    let _ = fs::remove_file(to);
    Err(io::Error::new(io::ErrorKind::InvalidData, problem))
}

fn log(state_dir: &Path, text: &str) {
    let line = format!("{} {text}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(state_dir.join("lazyrunner.log")) {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn the_tag_github_redirects_to_is_kept_with_its_version() {
        let release = tag_of("https://github.com/o/r/releases/tag/v0.2.1").unwrap();
        assert_eq!((release.tag.as_str(), release.version.as_str()), ("v0.2.1", "0.2.1"));
        let bare = tag_of("/o/r/releases/tag/0.3.0").unwrap();
        assert_eq!((bare.tag.as_str(), bare.version.as_str()), ("0.3.0", "0.3.0"), "a tag without v stays without");
        assert_eq!(tag_of("https://github.com/o/r/releases"), None);
        assert_eq!(tag_of("https://github.com/o/r/releases/tag/nightly"), None);
    }

    #[test]
    fn the_newest_release_is_the_highest_version_not_the_last_published() {
        let feed = r#"<feed>
  <entry><id>tag:github.com,2008:Repository/1/v0.15.0</id><link rel="alternate" type="text/html" href="https://github.com/o/r/releases/tag/v0.15.0"/></entry>
  <entry><link rel="alternate" type="text/html" href="https://github.com/o/r/releases/tag/v0.18.0"/></entry>
  <entry><link rel="alternate" type="text/html" href="https://github.com/o/r/releases/tag/v0.19.0-beta.1"/></entry>
  <entry><link rel="alternate" type="text/html" href="https://github.com/o/r/releases/tag/v0.9.3"/></entry>
</feed>"#;
        let newest = newest(feed).unwrap();
        assert_eq!((newest.tag.as_str(), newest.version.as_str()), ("v0.18.0", "0.18.0"), "not a pre-release");
        assert_eq!(super::newest("<feed></feed>"), None);
    }

    #[test]
    fn a_release_beats_its_own_pre_releases() {
        assert_eq!(compare("0.2.0", "0.2.0-dev"), Some(Ordering::Greater));
        assert_eq!(compare("0.2.0-rc.1", "0.2.0"), Some(Ordering::Less));
        assert_eq!(compare("0.2.1", "0.2.0"), Some(Ordering::Greater));
        assert_eq!(compare("v0.2.0", "0.2.0"), Some(Ordering::Equal));
        assert_eq!(compare("0.2.0-rc.2", "0.2.0-rc.1"), Some(Ordering::Greater));
        assert_eq!(compare("nightly", "0.2.0"), None);
    }

    #[test]
    fn redirects_resolve_and_stay_on_https() {
        let here = "https://github.com/o/r/releases/download/v1/a.zip";
        assert_eq!(resolve(here, "/o/r/x", false).unwrap(), "https://github.com/o/r/x");
        let cdn = "https://objects.githubusercontent.com/a?b=c";
        assert_eq!(resolve(here, cdn, false).unwrap(), cdn);
        assert!(resolve(here, "http://evil.example/a.zip", false).is_err(), "never down to plain HTTP");
        assert!(resolve(here, "ftp://example/a.zip", true).is_err());
        assert!(resolve(here, "", false).is_err());
        assert!(resolve("http://127.0.0.1:9/o/r", "/objects/a", true).is_ok(), "the tests' stand-in is plain HTTP");
    }

    #[test]
    fn every_windows_build_takes_the_msvc_archive() {
        assert_eq!(asset_name("x86_64-pc-windows-gnu"), "lazyrunner-x86_64-pc-windows-msvc.zip");
        assert_eq!(asset_name("aarch64-apple-darwin"), "lazyrunner-aarch64-apple-darwin.tar.gz");
        assert_eq!(asset_name("x86_64-unknown-linux-gnu"), "lazyrunner-x86_64-unknown-linux-gnu.tar.gz");
    }

    #[test]
    fn checksum_files_in_any_of_the_usual_shapes() {
        let hex = "a".repeat(64);
        assert_eq!(checksum_of(&hex), Some(hex.clone()));
        assert_eq!(checksum_of(&format!("{}  lazyrunner.zip\n", hex.to_uppercase())), Some(hex.clone()));
        assert_eq!(checksum_of(&format!("{hex} *lazyrunner.zip")), Some(hex));
        assert_eq!(checksum_of("not a checksum"), None);
    }

    #[test]
    fn the_source_comes_from_the_config_and_none_turns_it_off() {
        if std::env::var_os("LAZYRUNNER_UPDATE_REPO").is_some() {
            return; // the environment decides then
        }
        assert_eq!(source(&serde_json::json!({})).unwrap().url, DEFAULT_REPO);
        let mine = serde_json::json!({ "updateRepoUrl": "https://github.com/me/fork.git/" });
        assert_eq!(source(&mine).unwrap().url, "https://github.com/me/fork");
        assert_eq!(source(&mine).unwrap().host(), "github.com");
        assert_eq!(source(&serde_json::json!({ "updateRepoUrl": "none" })), None);
    }

    #[test]
    fn package_managers_and_cargo_builds_keep_their_binaries() {
        let managed = |path: &str| managed_by(Path::new(path)).unwrap_or_default();
        assert!(managed("/opt/homebrew/Cellar/lazyrunner/0.2.0/bin/lazyrunner").starts_with("installed by Homebrew"));
        assert!(managed("C:/Users/me/scoop/apps/lazyrunner/current/lazyrunner.exe").starts_with("installed by Scoop"));
        let winget = "C:/Users/me/AppData/Local/Microsoft/WinGet/Packages/lazyrunner_x/lazyrunner.exe";
        assert!(managed(winget).starts_with("installed by winget"));
        assert!(managed("/home/me/src/lazyrunner/target/release/lazyrunner").starts_with("a development build"));
        assert_eq!(managed_by(Path::new("/home/me/.local/bin/lazyrunner")), None);
    }

    #[test]
    fn auto_update_off_skips_unless_forced() {
        let temp = TempDir::new("update-off");
        let mut report = |_: &str| {};
        let config = serde_json::json!({ "autoUpdate": false });
        let outcome = update(&config, temp.path(), false, &mut Progress::new(&mut report));
        assert_eq!(outcome.status, Status::Skip);
        assert!(outcome.detail.ends_with("auto-update off (, then v turns it on)"), "{}", outcome.detail);
    }

    #[test]
    fn progress_shows_the_step_its_bar_and_size() {
        let mut lines = Vec::new();
        let mut report = |line: &str| lines.push(line.to_string());
        let mut progress = Progress::new(&mut report);
        progress.step("downloading", Some(40), Some(megabytes(3_100_000)));
        progress.step("downloading", Some(20), None);
        assert_eq!(lines, ["downloading ▕████░░░░░░▏ 40% · 3.1 MB", "downloading ▕████░░░░░░▏ 40% · 3.1 MB"]);
    }

    #[test]
    fn a_folder_lazyrunner_cant_write_to_is_noticed_before_downloading() {
        let temp = TempDir::new("update-writable");
        assert!(writable(temp.path()));
        assert!(!writable(&temp.join("missing")), "a folder that isn't there can't take the binary");
    }

    #[test]
    fn the_binary_comes_out_of_either_archive() {
        let temp = TempDir::new("update-extract");
        let name = binary_name();
        let zip_path = temp.join("lazyrunner-x.zip");
        let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("README.md", options).unwrap();
        zip.write_all(b"readme").unwrap();
        zip.start_file(format!("lazyrunner-x/{name}"), options).unwrap();
        zip.write_all(b"zip binary").unwrap();
        zip.finish().unwrap();
        extract(&zip_path, &temp.join("from-zip")).unwrap();
        assert_eq!(fs::read(temp.join("from-zip")).unwrap(), b"zip binary");

        let tar_path = temp.join("lazyrunner-x.tar.gz");
        let gz = flate2::write::GzEncoder::new(File::create(&tar_path).unwrap(), flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(10);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, format!("lazyrunner-x/{name}"), &b"tar binary"[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        extract(&tar_path, &temp.join("from-tar")).unwrap();
        assert_eq!(fs::read(temp.join("from-tar")).unwrap(), b"tar binary");

        let empty = temp.join("empty.zip");
        zip::ZipWriter::new(File::create(&empty).unwrap()).finish().unwrap();
        assert!(extract(&empty, &temp.join("none")).is_err());
        assert!(!temp.join("none").exists(), "nothing half-written is left");
        assert!(extract(&zip_path, &temp.join("from-zip")).is_err(), "never over a file that is already there");
    }
}
