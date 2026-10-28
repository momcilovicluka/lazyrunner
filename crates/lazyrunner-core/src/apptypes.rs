//! App types: the one place that knows what a dotnet or an npm app is. Everything else asks about a trait
//! (`app.traits().builds`) or calls a function here, so a new type is a new entry plus its commands.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::json::{self, field, field_ci, has_key_ci, keys, truthy};
use crate::profile::{App, AppKind, Variant, join};
use crate::profiletools::{Problem, Severity};

/// What a type of app does.
#[derive(Debug, PartialEq, Eq)]
pub struct Traits {
    /// Built before it starts (dotnet build), one build at a time, skipped when nothing changed.
    pub builds: bool,
    /// Reads the user secrets: restarts when the database is switched.
    pub uses_secrets: bool,
    /// Its log has one line per request (the req/s sparkline).
    pub counts_requests: bool,
    /// Indented lines continue the previous log entry.
    pub indented_logs: bool,
    /// Has a package-lock.json to install with npm ci.
    pub installs: bool,
    /// Runs one of the profile's variants, picked by which script package.json has.
    pub variants: bool,
    /// Rebuilds by itself, so a branch change or code change doesn't restart it.
    pub watches_own_files: bool,
    /// Red and yellow lines count as errors and warnings (no log levels).
    pub colors_errors: bool,
    /// What the frontend settings ("frontend", "stopFrontendOnStop") apply to.
    pub frontend: bool,
    /// Shown while it starts.
    pub start_verb: &'static str,
}

const DOTNET: Traits = Traits {
    builds: true,
    uses_secrets: true,
    counts_requests: true,
    indented_logs: true,
    installs: false,
    variants: false,
    watches_own_files: false,
    colors_errors: false,
    frontend: false,
    start_verb: "starting",
};
const NPM: Traits = Traits {
    builds: false,
    uses_secrets: false,
    counts_requests: false,
    indented_logs: false,
    installs: true,
    variants: true,
    watches_own_files: true,
    colors_errors: true,
    frontend: true,
    start_verb: "compiling",
};

impl App {
    pub fn traits(&self) -> &'static Traits {
        match self.kind {
            AppKind::Dotnet { .. } => &DOTNET,
            AppKind::Npm { .. } => &NPM,
        }
    }
}

fn app_setting<'a>(config: &'a Value, key: &str, setting: &str) -> &'a Value {
    field_ci(field(config, "apps"), key).map_or(&Value::Null, |app| field(app, setting))
}

/// Whether `key` is one of `apps` (keys are compared ignoring case, as people type them).
fn has_app(apps: &[App], key: &str) -> bool {
    apps.iter().any(|app| app.key.eq_ignore_ascii_case(key))
}

/// The column's level filter (`f`): `"apps": { "<key>": { "filter": "errors" } }`; all lines without one.
pub fn level_filter(config: &Value, key: &str) -> crate::output::LevelFilter {
    crate::output::LevelFilter::from_key(app_setting(config, key, "filter").as_str().unwrap_or_default())
}

/// Whether the app runs on this machine: `"apps": { "<key>": { "enabled": false } }` turns one off, and
/// `"frontend": false` every frontend.
pub fn is_enabled(config: &Value, app: &App) -> bool {
    let setting = app_setting(config, &app.key, "enabled");
    if !setting.is_null() {
        return truthy(setting);
    }
    if app.traits().frontend {
        return truthy(field(config, "frontend"));
    }
    true
}

/// Whether Stop all / Restart all include the app: `"apps": { "<key>": { "includeInAll": false } }`, else the
/// frontend setting for frontends (stopFrontendOnStop), else yes.
pub fn in_stop_all(config: &Value, app: &App) -> bool {
    let setting = app_setting(config, &app.key, "includeInAll");
    if !setting.is_null() {
        return truthy(setting);
    }
    if app.traits().frontend {
        return crate::config::Toggle::StopFrontendOnStop.read(config);
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeKind {
    Serve,
    Build,
}

/// How an npm app runs on this checkout: the variant that applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    pub kind: ModeKind,
    pub script: String,
    pub port: Option<u16>,
    pub label: String,
    pub served_by: Option<String>,
    pub ready_file: Option<String>,
}

static SCRIPT_PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)--port[= ](\d+)").expect("a valid pattern"));

fn package_path(dir: &str) -> String {
    join(dir, "package.json")
}

fn scripts_of(package: &Value) -> Value {
    let scripts = field(package, "scripts");
    if truthy(scripts) { scripts.clone() } else { Value::Object(Default::default()) }
}

fn selector(variant: &Variant) -> &str {
    variant.when.as_deref().unwrap_or(&variant.script)
}

/// The variant this checkout runs: the first whose `when` script is in its package.json (one without `when` needs
/// just its own script there), skipping a watch build whose server isn't among `active`. A dev server's port comes
/// from `--port` in its script when it has one.
pub fn mode(root: &Path, app: &App, active: &[App]) -> Option<Mode> {
    let AppKind::Npm { dir, variants } = &app.kind else { return None };
    let package = json::read(&root.join(package_path(dir)))?;
    let scripts = scripts_of(&package);
    for variant in variants {
        if !has_key_ci(&scripts, selector(variant)) {
            continue;
        }
        if variant.served_by.as_ref().is_some_and(|server| !has_app(active, server)) {
            continue;
        }
        let port = variant.port.map(|port| {
            let script = field_ci(&scripts, &variant.script).map(json::text).unwrap_or_default();
            SCRIPT_PORT.captures(&script).and_then(|found| found[1].parse().ok()).unwrap_or(port)
        });
        return Some(Mode {
            kind: if variant.port.is_some() { ModeKind::Serve } else { ModeKind::Build },
            script: variant.script.clone(),
            port,
            label: variant.label.clone(),
            served_by: variant.served_by.clone(),
            ready_file: variant.ready_file.clone(),
        });
    }
    None
}

/// Why an app has no variant on this checkout, for the log.
pub fn no_mode_reason(root: &Path, app: &App, active: &[App]) -> String {
    let AppKind::Npm { dir, variants } = &app.kind else { return String::new() };
    let package = package_path(dir);
    if !root.join(&package).exists() {
        return format!("{package} is missing");
    }
    let scripts: Vec<&str> = variants.iter().map(selector).collect();
    let what = match scripts.as_slice() {
        [one] => format!("has no {one}"),
        [a, b] => format!("has neither {a} nor {b}"),
        _ => format!("has none of {}", scripts.join(", ")),
    };
    let off = variants.iter().find(|variant| variant.served_by.as_ref().is_some_and(|server| !has_app(active, server)));
    let served = off.map_or(String::new(), |variant| {
        let server = variant.served_by.as_deref().unwrap_or_default();
        format!(", or {server}, which would serve {}, is turned off in lazyrunner.config.json", variant.script)
    });
    format!("{package} {what}{served}")
}

/// The launch settings file of a dotnet project, relative to the repo root.
fn launch_settings_path(project: &str) -> String {
    let dir = Path::new(project).parent().map_or(String::new(), |dir| dir.display().to_string());
    join(&join(&dir, "Properties"), "launchSettings.json")
}

/// The port an app listens on in this checkout.
pub fn port(root: &Path, app: &App, active: &[App]) -> Option<u16> {
    match &app.kind {
        AppKind::Npm { .. } => mode(root, app, active)?.port,
        AppKind::Dotnet { project, launch_profile, .. } => {
            let settings = json::read(&root.join(launch_settings_path(project)))?;
            let profile = field_ci(field(&settings, "profiles"), launch_profile)?;
            let url = json::text(field(profile, "applicationUrl"));
            url_port(url.split(';').next().unwrap_or_default())
        }
    }
}

/// The front-end framework an npm app is written with, by the packages its package.json depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framework {
    Angular,
    React,
    Vue,
}

pub fn framework(root: &Path, app: &App) -> Option<Framework> {
    let AppKind::Npm { dir, .. } = &app.kind else { return None };
    let package = json::read(&root.join(package_path(dir)))?;
    let depends =
        |name: &str| ["dependencies", "devDependencies"].iter().any(|at| has_key_ci(field(&package, at), name));
    [("@angular/core", Framework::Angular), ("react", Framework::React), ("vue", Framework::Vue)]
        .into_iter()
        .find_map(|(name, framework)| depends(name).then_some(framework))
}

/// The port of a URL, or its scheme's default (http 80, https 443).
pub fn url_port(url: &str) -> Option<u16> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let explicit = match host_port.rsplit_once(':') {
        Some((host, port)) if !host.ends_with(']') || host.starts_with('[') => port.parse().ok(),
        _ => None,
    };
    explicit.or(match scheme.to_ascii_lowercase().as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    })
}

static TEST_SDK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Microsoft\.NET\.Test\.Sdk").expect("a valid pattern"));

/// Whether a .csproj is a test project (it references Microsoft.NET.Test.Sdk).
pub fn is_test_project(text: &str) -> bool {
    TEST_SDK.is_match(text)
}

/// What `lazyrunner profile check` finds about an app in a checkout, each problem naming its field.
pub fn setup_problems(root: &Path, app: &App, at: &str, active: &[App]) -> Vec<Problem> {
    let mut problems = Vec::new();
    match &app.kind {
        AppKind::Dotnet { project, launch_profile, .. } => {
            let Ok(text) = json::read_text(&root.join(project)) else {
                problems.push(Problem::new(
                    Severity::Error,
                    format!("{at}.project"),
                    format!("{project} doesn't exist"),
                ));
                return problems;
            };
            if is_test_project(&text) {
                let message = format!(
                    "{project} is a test project (it references Microsoft.NET.Test.Sdk) - lazyrunner runs apps, not tests"
                );
                problems.push(Problem::new(Severity::Warning, format!("{at}.project"), message));
            }
            let settings_file = launch_settings_path(project);
            let at = format!("{at}.launchProfile");
            let Some(settings) = json::read(&root.join(&settings_file)) else {
                let message = format!(
                    "there is no {settings_file}, so lazyrunner can't run it with a launch profile or tell its port"
                );
                problems.push(Problem::new(Severity::Error, at, message));
                return problems;
            };
            let names = keys(field(&settings, "profiles"));
            if !names.iter().any(|name| name.eq_ignore_ascii_case(launch_profile)) {
                let message = format!("'{launch_profile}' is not in {settings_file} (it has: {})", names.join(", "));
                problems.push(Problem::new(Severity::Error, at, message));
                return problems;
            }
            if port(root, app, active).is_none() {
                let message = format!(
                    "'{launch_profile}' in {settings_file} has no applicationUrl, so lazyrunner can't tell its port"
                );
                problems.push(Problem::new(Severity::Error, at, message));
            }
        }
        AppKind::Npm { dir, variants } => {
            let package = package_path(dir);
            let Some(data) = json::read(&root.join(&package)) else {
                problems.push(Problem::new(Severity::Error, format!("{at}.dir"), format!("there is no {package}")));
                return problems;
            };
            let scripts = scripts_of(&data);
            for (index, variant) in variants.iter().enumerate() {
                if let Some(when) = variant.when.as_deref().filter(|when| !has_key_ci(&scripts, when)) {
                    let message = format!(
                        "{package} has no {when} on this branch, so this variant doesn't apply here (it will where it has)"
                    );
                    problems.push(Problem::new(Severity::Info, format!("{at}.variants[{index}]"), message));
                } else if !has_key_ci(&scripts, &variant.script) {
                    let message = format!("{package} has no script '{}'", variant.script);
                    problems.push(Problem::new(Severity::Error, format!("{at}.variants[{index}].script"), message));
                }
            }
            if mode(root, app, active).is_none() {
                let message = format!(
                    "no variant applies on this branch, so lazyrunner skips it ({})",
                    no_mode_reason(root, app, active)
                );
                problems.push(Problem::new(Severity::Warning, at, message));
            }
            if !crate::run::tools::has_packages(&root.join(dir)) {
                let message = format!(
                    "{} is missing: install the packages first (i in lazyrunner runs npm ci)",
                    join(dir, "node_modules")
                );
                problems.push(Problem::new(Severity::Info, at, message));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_gives_its_port_or_the_schemes_default() {
        assert_eq!(url_port("http://localhost:47201"), Some(47201));
        assert_eq!(url_port("https://localhost:7001/swagger"), Some(7001));
        assert_eq!(url_port("http://localhost"), Some(80));
        assert_eq!(url_port("https://example.test/a"), Some(443));
        assert_eq!(url_port("http://[::1]:5000"), Some(5000));
        assert_eq!(url_port("localhost:5000"), None);
    }

    #[test]
    fn an_npm_apps_framework_is_what_its_package_json_depends_on() {
        let temp = crate::testing::TempDir::new("framework");
        let app = |dir: &str| App {
            key: dir.into(),
            name: dir.into(),
            icon: None,
            nerd_icon: None,
            kind: AppKind::Npm { dir: dir.into(), variants: Vec::new() },
            tests: Vec::new(),
            migrations: None,
        };
        let write = |dir: &str, package: &str| {
            std::fs::create_dir_all(temp.join(dir)).unwrap();
            std::fs::write(temp.join(dir).join("package.json"), package).unwrap();
        };
        write("spa", r#"{ "dependencies": { "@angular/core": "^20.0.0", "rxjs": "^7.8.0" } }"#);
        write("shop", r#"{ "devDependencies": { "react": "^19.0.0" } }"#);
        write("api", r#"{ "dependencies": { "express": "^5.0.0" } }"#);
        assert_eq!(framework(temp.path(), &app("spa")), Some(Framework::Angular));
        assert_eq!(framework(temp.path(), &app("shop")), Some(Framework::React));
        assert_eq!(framework(temp.path(), &app("api")), None);
        assert_eq!(framework(temp.path(), &app("missing")), None);
    }
}
