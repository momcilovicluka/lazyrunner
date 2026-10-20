//! For tests only (the `test-support` feature): the sample profile, a sample repo for it, and temporary folders.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// tests/fixtures/sample.json: five apps, two .NET APIs and three npm apps (a dev server, a watch build an API
/// serves, and one with two variants).
pub fn sample_profile() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample.json")
}

pub const ORDERS_PORT: u16 = 58201;
pub const BILLING_PORT: u16 = 58202;
pub const SHOP_PORT: u16 = 58301;
pub const CONSOLE_PORT: u16 = 58303;

/// A folder of its own under the temp folder, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("lazyrunner-tests-{}-{label}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp folder");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, part: impl AsRef<Path>) -> PathBuf {
        self.0.join(part)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a file, making its folders.
pub fn write(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().expect("a file in a folder")).expect("its folder");
    std::fs::write(path, content).expect("the file");
}

/// The sample repo that tests/fixtures/sample.json is the profile of (as the powershell branch's New-SampleRepo.ps1 made
/// it): two tiny ASP.NET APIs (real dotnet build and run), and three npm apps without packages: a dev server, a
/// watch build that an API serves, and one with two variants. A git repo with one commit.
pub fn sample_repo(path: &Path) -> PathBuf {
    write(path.join("Sample.sln"), "Microsoft Visual Studio Solution File, Format Version 12.00\n");
    let framework = dotnet_framework();
    for (name, port) in [("Orders", ORDERS_PORT), ("Billing", BILLING_PORT)] {
        let dir = path.join("services").join(name);
        let project = format!(
            "<Project Sdk=\"Microsoft.NET.Sdk.Web\">\n  <PropertyGroup>\n    <TargetFramework>{framework}</TargetFramework>\n    <ImplicitUsings>enable</ImplicitUsings>\n    <Nullable>enable</Nullable>\n  </PropertyGroup>\n</Project>\n"
        );
        write(dir.join(format!("{name}.csproj")), &project);
        let program = format!(
            "var builder = WebApplication.CreateBuilder(args);\nvar app = builder.Build();\napp.MapGet(\"/\", () => \"{name}\");\napp.MapGet(\"/swagger/index.html\", () => Results.Content(\"<html>{name} swagger</html>\", \"text/html\"));\napp.Run();\n"
        );
        write(dir.join("Program.cs"), &program);
        let settings = format!(
            "{{\n  \"profiles\": {{\n    \"{name}\": {{\n      \"commandName\": \"Project\",\n      \"applicationUrl\": \"http://localhost:{port}\",\n      \"environmentVariables\": {{ \"ASPNETCORE_ENVIRONMENT\": \"Development\" }}\n    }}\n  }}\n}}\n"
        );
        write(dir.join("Properties").join("launchSettings.json"), &settings);
    }
    npm_app(&path.join("web").join("shop"), &format!(r#"{{ "dev": "node serve.js {SHOP_PORT}" }}"#));
    npm_app(&path.join("web").join("docs"), r#"{ "watch": "node watch.js" }"#);
    // No dev:mock, so its second variant runs.
    npm_app(&path.join("web").join("console"), &format!(r#"{{ "dev": "node serve.js {CONSOLE_PORT}" }}"#));
    write(path.join(".gitignore"), "bin/\nobj/\ndist/\nnode_modules/\n");
    let git = |args: &[&str]| {
        let _ = std::process::Command::new("git")
            .args(["-c", "commit.gpgsign=false", "-c", "user.name=tests", "-c", "user.email=tests@localhost"])
            .args(args)
            .current_dir(path)
            .output();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "sample"]);
    path.to_path_buf()
}

/// An npm app: package.json with these scripts, node_modules (lazyrunner won't start one without; these need no
/// packages) and the two fake tools the scripts run.
pub fn npm_app(dir: &Path, scripts: &str) {
    let name = dir.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
    write(
        dir.join("package.json"),
        &format!("{{ \"name\": \"{name}\", \"private\": true, \"scripts\": {scripts} }}\n"),
    );
    write(dir.join("node_modules").join(".fake"), "");
    write(
        dir.join("serve.js"),
        "// A dev server: listens on the port it is given, like vite or ng serve.\nconst port = Number(process.argv[2]);\nrequire('http').createServer((req, res) => res.end('ok')).listen(port, () => console.log(`Local: http://localhost:${port}/`));\n",
    );
    write(
        dir.join("watch.js"),
        "// A watch build that never ends: writes dist/index.html, then keeps watching.\nconst fs = require('fs'), path = require('path');\nconsole.log('Building...');\nsetTimeout(() => {\n  fs.mkdirSync(path.join(__dirname, 'dist'), { recursive: true });\n  fs.writeFileSync(path.join(__dirname, 'dist', 'index.html'), '<html>built</html>');\n  console.log('Build complete. Watching for file changes...');\n}, 1000);\nsetInterval(() => {}, 1 << 30);\n",
    );
}

/// The target framework of the installed .NET SDK ("net10.0"), so the sample APIs build on it.
fn dotnet_framework() -> String {
    let version = std::process::Command::new("dotnet")
        .arg("--version")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    let major = version.split('.').next().and_then(|major| major.parse::<u32>().ok()).unwrap_or(9);
    format!("net{major}.0")
}

/// Whether the tools the sample repo's apps need are installed (the end-to-end tests skip without them).
pub fn has_tools(tools: &[&str]) -> bool {
    tools.iter().all(|tool| crate::run::tools::find_on_path(tool).is_some())
}
