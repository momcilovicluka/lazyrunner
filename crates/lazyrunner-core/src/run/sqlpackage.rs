//! SqlPackage (#110): exports a database to a `.bacpac` and imports one into a new database. It runs on this machine,
//! so its files are lazyrunner's own (the project's folder of them). The connection string, password and all, goes
//! to it in a response file (`@file`) in lazyrunner's state folder - deleted as soon as SqlPackage has read it (its
//! first line out) and in any case once it's done - never on its command line. It says each table as it gets to it
//! (`Processing Table '[dbo].[Orders]'.`): that's the progress.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use regex::Regex;

use super::cancel::{self, CANCELLED};
use super::secrets::mask_text;
use super::tools;

/// The program, as PATH names it.
pub const PROGRAM: &str = "sqlpackage";
/// What installs it, and what lazyrunner says when it isn't there.
pub const INSTALL: [&str; 4] = ["tool", "install", "-g", "microsoft.sqlpackage"];
pub const INSTALL_TEXT: &str = "dotnet tool install -g microsoft.sqlpackage";
pub const MISSING: &str = "SqlPackage isn't installed - exports and .bacpac imports run through it: dotnet tool \
                           install -g microsoft.sqlpackage";
/// Where the DacFx installer puts it, under Program Files: the newest first.
const DAC_FOLDERS: [&str; 5] = ["170", "160", "150", "140", "130"];
/// The line it names a table with, and the lines it reports an error with.
const TABLE_LINE: &str = "processing table";
const ERROR_LINE: &str = "***";
/// In a `.bacpac`: the file that counts what it holds, and its count of tables.
const ORIGIN: &str = "Origin.xml";
const TABLE_COUNT: &str = "Table";
const OBJECT_COUNTS: &str = "ObjectCounts";
/// The response file's name: `sqlpackage-<process>-<nanoseconds>.rsp`.
const RESPONSE_PREFIX: &str = "sqlpackage-";
const RESPONSE_EXTENSION: &str = "rsp";

static DATABASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)((?:^|;)\s*(?:database|initial catalog)\s*=\s*)[^;]*").expect("a valid pattern"));

/// SqlPackage: on PATH, else where `dotnet tool install -g` puts it, else where the DacFx installer does.
pub fn find() -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{PROGRAM}.exe") } else { PROGRAM.to_string() };
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let dotnet_tool = home.map(|home| home.join(".dotnet").join("tools").join(&exe));
    let program_files = ["ProgramFiles", "ProgramFiles(x86)"].into_iter().filter_map(std::env::var_os);
    let dac: Vec<PathBuf> = program_files
        .flat_map(|root| {
            DAC_FOLDERS.map(|version| {
                PathBuf::from(&root).join("Microsoft SQL Server").join(version).join("DAC").join("bin").join(&exe)
            })
        })
        .collect();
    tools::find_on_path(PROGRAM).or_else(|| dotnet_tool.into_iter().chain(dac).find(|path| path.is_file()))
}

/// What SqlPackage does: a database to a `.bacpac`, or a `.bacpac` into a new database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Export,
    Import,
}

/// SqlPackage's command line, and the response file's text: the connection string goes in that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub args: Vec<String>,
    pub response: String,
}

/// What runs `action` with `connection` and `file` (`file` the `.bacpac` it writes or reads).
pub fn invocation(action: Action, connection: &str, file: &str) -> Result<Invocation, String> {
    if connection.contains(['"', '\r', '\n']) {
        return Err("SqlPackage takes the connection string in a response file, which can't hold a \" - write its \
                    values with ' instead"
            .into());
    }
    let (name, side, target) = match action {
        Action::Export => ("Export", "SourceConnectionString", "TargetFile"),
        Action::Import => ("Import", "TargetConnectionString", "SourceFile"),
    };
    Ok(Invocation {
        args: vec![format!("/Action:{name}"), format!("/{target}:{file}")],
        response: format!("/{side}:\"{connection}\""),
    })
}

/// `connection` naming `database`: its own database's name swapped, or `Database=` added.
pub fn with_database(connection: &str, database: &str) -> String {
    if DATABASE.is_match(connection) {
        return DATABASE.replace_all(connection, |found: &regex::Captures| format!("{}{database}", &found[1])).into();
    }
    let joint = if connection.trim_end().ends_with(';') || connection.trim().is_empty() { "" } else { ";" };
    format!("{}{joint}Database={database}", connection.trim_end())
}

/// The table a line of SqlPackage's names as it gets to it: `Processing Table '[dbo].[Orders]'.` is `dbo.Orders`.
pub fn table_of(line: &str) -> Option<String> {
    let at = line.to_lowercase().find(TABLE_LINE)?;
    let rest = &line[at + TABLE_LINE.len()..];
    let (start, end) = (rest.find('\'')?, rest.rfind('\'')?);
    let name: String = rest.get(start + 1..end)?.chars().filter(|c| !matches!(c, '[' | ']')).collect();
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

/// The error SqlPackage reported among its lines, masked: its `*** ` lines.
pub fn error_of(lines: &[String]) -> Option<String> {
    let said: Vec<String> = lines
        .iter()
        .filter_map(|line| line.trim_start().strip_prefix(ERROR_LINE))
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    (!said.is_empty()).then(|| mask_text(&said.join(" ")))
}

/// How many tables a `.bacpac` holds, as its `Origin.xml` counts them: what an import's progress counts towards.
pub fn tables_in(path: &Path) -> Option<usize> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
    let mut text = String::new();
    archive.by_name(ORIGIN).ok()?.read_to_string(&mut text).ok()?;
    let document = roxmltree::Document::parse(text.trim_start_matches('\u{feff}')).ok()?;
    let counts = document.descendants().find(|node| node.tag_name().name() == OBJECT_COUNTS)?;
    let tables = counts.children().find(|node| node.tag_name().name() == TABLE_COUNT)?;
    tables.text()?.trim().parse().ok()
}

/// What runs SqlPackage: the real one, or the tests' stand-in.
pub trait Package: Send + Sync {
    /// Runs `action` with `connection` and `file`, its response file in `private` (a folder only you can read), each
    /// line it prints to `line` as it comes; the error it reported when it failed.
    fn run(
        &self,
        action: Action,
        connection: &str,
        file: &str,
        private: &Path,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), String>;

    /// Whether it's there; what to do when it isn't.
    fn ready(&self) -> Result<(), String>;
}

/// SqlPackage, wherever [`find`] finds it.
pub struct SqlPackage;

/// The response file: deleted when it's dropped, if it's still there.
struct Response(PathBuf);

impl Response {
    fn write(private: &Path, text: &str) -> Result<Self, String> {
        std::fs::create_dir_all(private).map_err(|error| format!("{}: {error}", private.display()))?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let name = format!("{RESPONSE_PREFIX}{}-{stamp}.{RESPONSE_EXTENSION}", std::process::id());
        let path = private.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let written = file.write_all(text.as_bytes());
        drop(file);
        let response = Self(path);
        written.map_err(|error| format!("{}: {error}", response.0.display()))?;
        Ok(response)
    }

    fn remove(&self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl Drop for Response {
    fn drop(&mut self) {
        self.remove();
    }
}

impl Package for SqlPackage {
    fn run(
        &self,
        action: Action,
        connection: &str,
        file: &str,
        private: &Path,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), String> {
        let program = find().ok_or_else(|| MISSING.to_string())?;
        if cancel::asked() {
            return Err(CANCELLED.into());
        }
        let call = invocation(action, connection, file)?;
        let response = Response::write(private, &call.response)?;
        let mut command = Command::new(program);
        command.args(&call.args).arg(format!("@{}", response.0.display()));
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = tools::own_console(&mut command).spawn().map_err(|error| format!("{PROGRAM}: {error}"))?;
        let watched = cancel::running(child.id());
        let errors = child.stderr.take().map(|stderr| {
            std::thread::spawn(move || BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<String>>())
        });
        let mut said = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            for text in BufReader::new(stdout).split(b'\n').map_while(Result::ok) {
                response.remove();
                let text = String::from_utf8_lossy(&text);
                let text = text.trim_end_matches('\r');
                line(text);
                said.push(text.to_string());
            }
        }
        said.extend(errors.and_then(|errors| errors.join().ok()).unwrap_or_default());
        let status = child.wait().map_err(|error| format!("{PROGRAM}: {error}"))?;
        drop((response, watched));
        if status.success() {
            return Ok(());
        }
        if cancel::asked() {
            return Err(CANCELLED.into());
        }
        Err(error_of(&said).unwrap_or_else(|| format!("{PROGRAM} failed ({status})")))
    }

    fn ready(&self) -> Result<(), String> {
        find().map(|_| ()).ok_or_else(|| MISSING.to_string())
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn the_connection_string_goes_in_the_response_file_never_on_the_command_line() {
        let login = "Server=qa-sql.example.com;Database=Shop;User Id=reader;Password=s3cret";
        let export = invocation(Action::Export, login, r"C:\lr\Shop_20261123_1042.bacpac").unwrap();
        assert_eq!(export.args, ["/Action:Export", r"/TargetFile:C:\lr\Shop_20261123_1042.bacpac"]);
        assert!(!export.args.iter().any(|arg| arg.contains("s3cret") || arg.contains("Server=")), "{:?}", export.args);
        assert_eq!(export.response, format!("/SourceConnectionString:\"{login}\""));
        let import = invocation(Action::Import, &with_database(login, "Shop_qa"), r"C:\lr\x.bacpac").unwrap();
        assert_eq!(import.args, ["/Action:Import", r"/SourceFile:C:\lr\x.bacpac"]);
        assert!(
            import.response.starts_with("/TargetConnectionString:\"") && import.response.contains("Database=Shop_qa;")
        );
        assert!(invocation(Action::Export, "Server=.;Password=\"a;b\"", "x").is_err(), "a \" can't be carried");
    }

    #[test]
    fn a_connection_string_names_another_database_its_own_swapped() {
        assert_eq!(
            with_database("Server=.;Database=Shop;Trusted_Connection=True", "Shop_copy"),
            "Server=.;Database=Shop_copy;Trusted_Connection=True"
        );
        assert_eq!(
            with_database("Data Source=.;Initial Catalog=Shop", "Orders"),
            "Data Source=.;Initial Catalog=Orders"
        );
        assert_eq!(
            with_database("Server=.;Trusted_Connection=True;", "Orders"),
            "Server=.;Trusted_Connection=True;Database=Orders"
        );
        assert_eq!(with_database("Server=.", "Orders"), "Server=.;Database=Orders");
    }

    #[test]
    fn its_lines_say_the_table_it_is_on_and_the_error_it_hit() {
        assert_eq!(table_of("Processing Table '[dbo].[OrderLines]'.").as_deref(), Some("dbo.OrderLines"));
        assert_eq!(table_of("  processing table '[sales].[Odd Name]'").as_deref(), Some("sales.Odd Name"));
        assert_eq!(table_of("Processing Export."), None);
        assert_eq!(table_of("Exporting data from database"), None);
        let lines: Vec<String> = [
            "Connecting to database 'Shop' on server '.'.",
            "*** Error exporting database:Could not connect to database server.",
            "*** Login failed; Password=s3cret",
        ]
        .iter()
        .map(|line| line.to_string())
        .collect();
        let said = error_of(&lines).unwrap();
        assert!(said.starts_with("Error exporting database:Could not connect") && !said.contains("s3cret"), "{said}");
        assert_eq!(error_of(&lines[..1]), None);
    }

    #[test]
    fn a_bacpac_counts_its_tables_in_its_origin() {
        let temp = TempDir::new("bacpac");
        let path = temp.join("Shop.bacpac");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        zip.start_file(ORIGIN, zip::write::SimpleFileOptions::default()).unwrap();
        let origin = "\u{feff}<?xml version=\"1.0\"?><DacOrigin xmlns=\"http://schemas.microsoft.com/sqlserver/dac/Serialization/2012/02\"><Server><ObjectCounts><SimpleColumn>7</SimpleColumn><Table>4</Table></ObjectCounts></Server></DacOrigin>";
        zip.write_all(origin.as_bytes()).unwrap();
        zip.finish().unwrap();
        assert_eq!(tables_in(&path), Some(4));
        assert_eq!(tables_in(&temp.join("missing.bacpac")), None);
    }

    #[test]
    fn the_response_file_is_gone_once_it_has_been_read() {
        let temp = TempDir::new("response");
        let response = Response::write(temp.path(), "/SourceConnectionString:\"x\"").unwrap();
        let path = response.0.clone();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "/SourceConnectionString:\"x\"");
        drop(response);
        assert!(!path.exists());
    }
}
