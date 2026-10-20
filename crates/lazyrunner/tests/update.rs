//! Self-update end to end: an installed copy of lazyrunner (outside the cargo target folder, as on a real machine)
//! runs `lazyrunner update` against a local stand-in for GitHub Releases, which redirects `releases/latest` to the
//! tag and the download to another path, as GitHub does. The binary that is running gets replaced; a download that
//! doesn't match its checksum is refused and leaves it as it was.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread;

use lazyrunner_core::testing::TempDir;
use lazyrunner_core::update::{TARGET, asset_name, sha256_of};

const BIN: &str = env!("CARGO_BIN_EXE_lazyrunner");
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Appended to the release's binary, so the installed one can be told from the one it replaced.
const MARKER: &[u8] = b"\nlazyrunner-update-test-release\n";

/// What the stand-in serves: the latest tag, and the release's files by name.
struct Release {
    tag: String,
    files: Vec<(String, Vec<u8>)>,
}

/// Serves `release` at http://127.0.0.1:<port>/o/r (the repo URL to update from).
fn serve(release: Release) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let release = Arc::new(release);
    let host = base.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (release, host) = (Arc::clone(&release), host.clone());
            thread::spawn(move || answer(stream, &release, &host));
        }
    });
    format!("{base}/o/r")
}

fn answer(mut stream: TcpStream, release: &Release, host: &str) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request = String::new();
    let _ = reader.read_line(&mut request);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
    let download = format!("/o/r/releases/download/{}/", release.tag);
    let file = |name: &str| release.files.iter().find(|(file, _)| file == name).map(|(_, bytes)| bytes.clone());
    let (status, location, body) = if path == "/o/r/releases/latest" {
        ("302 Found", format!("{host}/o/r/releases/tag/{}", release.tag), Vec::new())
    } else if let Some(name) = path.strip_prefix(&download) {
        ("302 Found", format!("{host}/objects/{name}"), Vec::new()) // GitHub serves files from another host
    } else if let Some(bytes) = path.strip_prefix("/objects/").and_then(file) {
        ("200 OK", String::new(), bytes)
    } else {
        ("404 Not Found", String::new(), b"Not Found".to_vec())
    };
    let location = if location.is_empty() { String::new() } else { format!("Location: {location}\r\n") };
    let head = format!("HTTP/1.1 {status}\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(&body));
}

/// This platform's release archive holding `binary`, and its .sha256 (made up when `checksum` is given).
fn release_files(temp: &TempDir, binary: &[u8], checksum: Option<&str>) -> Vec<(String, Vec<u8>)> {
    let asset = asset_name(TARGET);
    let path = temp.join(&asset);
    let name = if cfg!(windows) { "lazyrunner.exe" } else { "lazyrunner" };
    if asset.ends_with(".zip") {
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        // Stored, not compressed: the debug binary is large, and the test is about the update, not zip.
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file(format!("lazyrunner-{TARGET}/{name}"), stored).unwrap();
        zip.write_all(binary).unwrap();
        zip.finish().unwrap();
    } else {
        let gz = flate2::write::GzEncoder::new(File::create(&path).unwrap(), flate2::Compression::none());
        let mut tar = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(binary.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, format!("lazyrunner-{TARGET}/{name}"), binary).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    let sum = checksum.map_or_else(|| sha256_of(&path).unwrap(), str::to_string);
    vec![(format!("{asset}.sha256"), format!("{sum}  {asset}\n").into_bytes()), (asset, fs::read(&path).unwrap())]
}

/// lazyrunner copied to a folder of its own, as an install is.
fn install(temp: &TempDir) -> PathBuf {
    let dir = temp.join("bin");
    fs::create_dir_all(&dir).unwrap();
    let installed = dir.join(Path::new(BIN).file_name().unwrap());
    fs::copy(BIN, &installed).unwrap();
    installed
}

/// `lazyrunner update` from `repo`, with its own settings and state; what it printed.
fn update(temp: &TempDir, installed: &Path, repo: &str) -> String {
    let output = Command::new(installed)
        .arg("update")
        .env("LAZYRUNNER_UPDATE_REPO", repo)
        .env("LAZYRUNNER_CONFIG_PATH", temp.join("config.json"))
        .env("LAZYRUNNER_STATE_DIR", temp.join("state"))
        .env("LAZYRUNNER_PROFILES_DIR", temp.join("profiles"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn new_binary() -> Vec<u8> {
    let mut binary = fs::read(BIN).unwrap();
    binary.extend_from_slice(MARKER);
    binary
}

#[test]
fn an_install_updates_itself_to_a_newer_release() {
    let temp = TempDir::new("update-newer");
    let installed = install(&temp);
    let binary = new_binary();
    let repo = serve(Release { tag: "v99.1.0".into(), files: release_files(&temp, &binary, None) });
    let printed = update(&temp, &installed, &repo);
    assert!(printed.contains(&format!("lazyrunner v{VERSION}")), "{printed}");
    assert!(printed.contains("checking 127.0.0.1…") && printed.contains("installing v99.1.0…"), "{printed}");
    assert!(printed.contains(&format!("updated v{VERSION} → v99.1.0")), "{printed}");
    assert_eq!(fs::read(&installed).unwrap(), binary, "the running binary was replaced by the release's");
    let log = fs::read_to_string(temp.join("state").join("lazyrunner.log")).unwrap();
    assert!(log.contains(&format!("updated lazyrunner v{VERSION} -> v99.1.0")), "{log}");
    let leftovers = fs::read_dir(temp.join("state").join("update")).map_or(0, |dir| dir.count());
    assert_eq!(leftovers, 0, "the download is cleaned up");
    // The new binary runs (the Unix risk: it must still be executable). Not on macOS, where the bytes this test
    // appends would break the binary's code signature.
    if !cfg!(target_os = "macos") {
        let version = Command::new(&installed).arg("version").env("NO_COLOR", "1").output().unwrap();
        assert!(String::from_utf8_lossy(&version.stdout).starts_with(&format!("lazyrunner v{VERSION}")));
    }
}

#[test]
fn a_tag_without_a_v_is_found_where_it_was_published() {
    let temp = TempDir::new("update-bare-tag");
    let installed = install(&temp);
    let binary = new_binary();
    let repo = serve(Release { tag: "99.2.0".into(), files: release_files(&temp, &binary, None) });
    let printed = update(&temp, &installed, &repo);
    assert!(printed.contains(&format!("updated v{VERSION} → v99.2.0")), "{printed}");
    assert_eq!(fs::read(&installed).unwrap(), binary);
}

#[test]
fn a_download_that_doesnt_match_its_checksum_is_not_installed() {
    let temp = TempDir::new("update-checksum");
    let installed = install(&temp);
    let before = fs::read(&installed).unwrap();
    let files = release_files(&temp, &new_binary(), Some(&"0".repeat(64)));
    let repo = serve(Release { tag: "v99.1.0".into(), files });
    let printed = update(&temp, &installed, &repo);
    assert!(
        printed.contains("v99.1.0 is out, but not installed: the download doesn't match its checksum"),
        "{printed}"
    );
    assert_eq!(fs::read(&installed).unwrap(), before, "lazyrunner is left as it was");
}

#[test]
fn the_same_or_an_older_release_changes_nothing() {
    let temp = TempDir::new("update-same");
    let installed = install(&temp);
    let repo = serve(Release { tag: format!("v{VERSION}"), files: Vec::new() });
    let printed = update(&temp, &installed, &repo);
    assert!(printed.contains(&format!("v{VERSION} · up to date")), "{printed}");
    let repo = serve(Release { tag: "v0.0.1".into(), files: Vec::new() });
    let printed = update(&temp, &installed, &repo);
    assert!(printed.contains(&format!("v{VERSION} · ahead of the released v0.0.1")), "{printed}");
}

#[test]
fn a_release_repo_that_cant_be_reached_or_has_no_releases_says_so() {
    let temp = TempDir::new("update-unreachable");
    let installed = install(&temp);
    // Port 9 (discard): nothing listens there, and no test's stand-in can be given it.
    let printed = update(&temp, &installed, "http://127.0.0.1:9/o/r");
    assert!(printed.contains("couldn't check for updates: can't reach 127.0.0.1 (offline?)"), "{printed}");
    let repo = serve(Release { tag: String::new(), files: Vec::new() });
    let printed = update(&temp, &installed, &repo.replace("/o/r", "/someone/else"));
    assert!(printed.contains("no releases at"), "{printed}");
}

#[test]
fn none_turns_updates_off() {
    let temp = TempDir::new("update-none");
    let installed = install(&temp);
    assert!(update(&temp, &installed, "none").contains("no release repo configured"));
}
