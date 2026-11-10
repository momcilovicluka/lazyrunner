//! A .NET build as a map (#82): the projects an app's build compiles - its project and all it references - each with
//! when it finished (from MSBuild's own `Project -> output.dll` lines, stamped as they arrive) or that it failed (an
//! error line names its project). From those: each project's state, its own time (since the last of the projects it
//! references was done) and the critical path - the chain of references that decided how long the build took. The
//! code map draws it over its projects; the last build of each app is kept, for `D` to open it again.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::build;
use super::code::same;

/// A project in a build.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectRun {
    pub name: String,
    pub path: PathBuf,
    /// The projects of this build it references, by their place in it.
    pub references: Vec<usize>,
    /// When it was done, in seconds from the build's start.
    pub done_at: Option<f64>,
    pub failed: bool,
}

/// How a project of a build is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectState {
    /// Waiting for a project it references.
    Queued,
    /// Everything it references is done: it compiles.
    Building,
    Done,
    Failed,
    /// The build ended before it was built (another project failed).
    Skipped,
}

/// An app's build, its projects and how each went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BuildRun {
    /// The app's key and name.
    pub key: String,
    pub app: String,
    /// When it started (RFC 3339, local time).
    pub at: String,
    pub projects: Vec<ProjectRun>,
    /// How long it took, once it's over, and whether it went fine.
    pub seconds: Option<f64>,
    pub ok: Option<bool>,
}

/// The project an error line names at its end: `... error CS1002: ; expected [C:\src\Orders.Core\Orders.Core.csproj]`.
static ERROR_PROJECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i): error .*\[([^\]]+\.(?:cs|fs|vb)proj)\]\s*$").expect("a valid pattern"));

fn stem(path: &Path) -> String {
    path.file_stem().map(|stem| stem.to_string_lossy().to_lowercase()).unwrap_or_default()
}

impl BuildRun {
    /// A build of `project` (the app's), starting: the project and all it references, none done yet.
    pub fn new(key: &str, app: &str, project: &Path) -> Self {
        let paths = build::projects(project);
        let place = |path: &Path| paths.iter().position(|known| same(known) == same(path));
        let projects = paths
            .iter()
            .map(|path| ProjectRun {
                name: path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default(),
                path: path.clone(),
                references: build::references(path).iter().filter_map(|reference| place(reference)).collect(),
                done_at: None,
                failed: false,
            })
            .collect();
        let at = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        Self { key: key.to_string(), app: app.to_string(), at, projects, seconds: None, ok: None }
    }

    /// The project of this build by its name (as MSBuild says it) or its path.
    pub fn find(&self, name: &str) -> Option<usize> {
        let name = name.to_lowercase();
        self.projects.iter().position(|project| stem(&project.path) == name || project.name.to_lowercase() == name)
    }

    /// The project at `path` (a code map part's), if this build has it.
    pub fn at_path(&self, path: &Path) -> Option<usize> {
        self.projects.iter().position(|project| same(&project.path) == same(path))
    }

    /// MSBuild said `name` is done, `seconds` into the build.
    pub fn built(&mut self, name: &str, seconds: f64) {
        if let Some(at) = self.find(name)
            && self.projects[at].done_at.is_none()
        {
            self.projects[at].done_at = Some(seconds);
        }
    }

    /// An output line: when it's an error naming its project, that project failed.
    pub fn line(&mut self, text: &str) {
        let Some(found) = ERROR_PROJECT.captures(text) else { return };
        let path = PathBuf::from(&found[1]);
        if let Some(at) = self.at_path(&path).or_else(|| self.find(&stem(&path))) {
            self.projects[at].failed = true;
        }
    }

    /// The build is over: how long it took and whether it went fine.
    pub fn finish(&mut self, seconds: f64, ok: bool) {
        (self.seconds, self.ok) = (Some(seconds), Some(ok));
    }

    pub fn running(&self) -> bool {
        self.seconds.is_none()
    }

    pub fn done(&self) -> usize {
        self.projects.iter().filter(|project| project.done_at.is_some()).count()
    }

    pub fn state(&self, at: usize) -> ProjectState {
        let project = &self.projects[at];
        let ready = || project.references.iter().all(|&reference| self.projects[reference].done_at.is_some());
        match () {
            _ if project.failed => ProjectState::Failed,
            _ if project.done_at.is_some() => ProjectState::Done,
            _ if !self.running() => ProjectState::Skipped,
            _ if ready() => ProjectState::Building,
            _ => ProjectState::Queued,
        }
    }

    /// When the last of the projects it references was done: when it could start.
    fn ready_at(&self, at: usize) -> f64 {
        let done = self.projects[at].references.iter().filter_map(|&reference| self.projects[reference].done_at);
        done.fold(0.0, f64::max)
    }

    /// A project's own time: from when it could start to when it was done.
    pub fn own_seconds(&self, at: usize) -> Option<f64> {
        self.projects[at].done_at.map(|done| (done - self.ready_at(at)).max(0.0))
    }

    /// The chain of references that decided the build's length, the first project built first: from the project done
    /// last, back through the reference done last of each.
    pub fn critical_path(&self) -> Vec<usize> {
        let latest = |places: &mut dyn Iterator<Item = usize>| {
            places
                .filter_map(|at| Some((at, self.projects[at].done_at?)))
                .max_by(|(_, a), (_, b)| a.total_cmp(b))
                .map(|(at, _)| at)
        };
        let Some(mut at) = latest(&mut (0..self.projects.len())) else { return Vec::new() };
        let mut path = vec![at];
        while let Some(next) = latest(&mut self.projects[at].references.iter().copied()) {
            if path.contains(&next) {
                break; // a reference cycle: MSBuild refuses it, the map doesn't loop
            }
            path.push(next);
            at = next;
        }
        path.reverse();
        path
    }

    /// How long the critical path took: when its last project was done.
    pub fn critical_seconds(&self) -> Option<f64> {
        self.critical_path().last().and_then(|&at| self.projects[at].done_at)
    }
}

/// Where an app's last build is kept, in the project's state folder.
pub fn file(state_dir: &Path, key: &str) -> PathBuf {
    state_dir.join(format!("build-map.{key}.json"))
}

/// The app's last build as it was kept.
pub fn load(state_dir: &Path, key: &str) -> Option<BuildRun> {
    let text = std::fs::read_to_string(file(state_dir, key)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Keeps the app's last build; why it couldn't be, when it couldn't.
pub fn save(state_dir: &Path, run: &BuildRun) -> Option<String> {
    let text = serde_json::to_string(run).ok()?;
    std::fs::create_dir_all(state_dir)
        .and_then(|()| std::fs::write(file(state_dir, &run.key), text))
        .err()
        .map(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TempDir, write};

    /// Api -> Core -> Kernel, and Api -> Data -> Kernel.
    fn solution(temp: &TempDir) -> PathBuf {
        let project = |references: &[&str]| {
            let items: String =
                references.iter().map(|path| format!(r#"<ProjectReference Include="{path}" />"#)).collect();
            format!("<Project><ItemGroup>{items}</ItemGroup></Project>")
        };
        write(temp.join("Kernel/Kernel.csproj"), &project(&[]));
        write(temp.join("Core/Core.csproj"), &project(&["..\\Kernel\\Kernel.csproj"]));
        write(temp.join("Data/Data.csproj"), &project(&["../Kernel/Kernel.csproj"]));
        write(temp.join("Api/Api.csproj"), &project(&["..\\Core\\Core.csproj", "..\\Data\\Data.csproj"]));
        temp.join("Api/Api.csproj")
    }

    #[test]
    fn a_builds_projects_go_from_queued_to_done_and_its_critical_path_is_the_chain_done_last() {
        let temp = TempDir::new("build-map");
        let mut run = BuildRun::new("api", "Api", &solution(&temp));
        let at = |run: &BuildRun, name: &str| run.find(name).unwrap();
        assert_eq!(run.projects.len(), 4);
        assert_eq!(run.state(at(&run, "Kernel")), ProjectState::Building, "it references nothing");
        assert_eq!(run.state(at(&run, "Core")), ProjectState::Queued);
        run.built("Kernel", 2.0);
        assert_eq!(
            (run.state(at(&run, "Core")), run.state(at(&run, "Data"))),
            (ProjectState::Building, ProjectState::Building)
        );
        run.built("Data", 3.0);
        run.built("Core", 9.0);
        run.built("Api", 11.0);
        run.finish(12.0, true);
        let names: Vec<&str> = run.critical_path().iter().map(|&at| run.projects[at].name.as_str()).collect();
        assert_eq!(names, ["Kernel", "Core", "Api"], "Core was done after Data");
        assert_eq!(run.critical_seconds(), Some(11.0));
        assert_eq!(run.own_seconds(at(&run, "Core")), Some(7.0), "from Kernel's end");
        assert_eq!(run.own_seconds(at(&run, "Api")), Some(2.0));
        assert_eq!(run.done(), 4);
    }

    #[test]
    fn an_error_line_fails_its_project_and_what_wasnt_built_is_skipped() {
        let temp = TempDir::new("build-map-fail");
        let mut run = BuildRun::new("api", "Api", &solution(&temp));
        run.built("Kernel", 1.0);
        let core = temp.join("Core/Core.csproj");
        run.line(&format!(
            "{}\\Pricing.cs(12,5): error CS1002: ; expected [{}]",
            temp.join("Core").display(),
            core.display()
        ));
        run.line("warning CS0168: unused [C:\\elsewhere\\Data.csproj]");
        run.finish(4.0, false);
        let at = |name: &str| run.find(name).unwrap();
        assert_eq!(run.state(at("Core")), ProjectState::Failed);
        assert_eq!(run.state(at("Data")), ProjectState::Skipped, "a warning isn't a failure");
        assert_eq!(run.state(at("Kernel")), ProjectState::Done);
    }

    #[test]
    fn an_apps_last_build_is_kept() {
        let temp = TempDir::new("build-map-keep");
        let mut run = BuildRun::new("api", "Api", &solution(&temp));
        run.built("Kernel", 1.5);
        run.finish(2.0, false);
        assert_eq!(save(&temp.join("state"), &run), None);
        assert_eq!(load(&temp.join("state"), "api"), Some(run));
        assert_eq!(load(&temp.join("state"), "web"), None);
    }
}
