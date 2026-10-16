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

/// The sample repo that tests/fixtures/sample.json is the profile of (as tests/fixtures/New-SampleRepo.ps1 makes
/// it): Sample.sln, two ASP.NET APIs with launch settings, and three npm apps with node_modules.
pub fn sample_repo(path: &Path) -> PathBuf {
    write(path.join("Sample.sln"), "Microsoft Visual Studio Solution File, Format Version 12.00\n");
    for (name, port) in [("Orders", ORDERS_PORT), ("Billing", BILLING_PORT)] {
        let dir = path.join("services").join(name);
        write(
            dir.join(format!("{name}.csproj")),
            "<Project Sdk=\"Microsoft.NET.Sdk.Web\">\n  <PropertyGroup>\n    <TargetFramework>net9.0</TargetFramework>\n  </PropertyGroup>\n</Project>\n",
        );
        write(
            dir.join("Program.cs"),
            &format!(
                "var app = WebApplication.CreateBuilder(args).Build();\napp.MapGet(\"/\", () => \"{name}\");\napp.Run();\n"
            ),
        );
        let settings = format!(
            "{{\n  \"profiles\": {{\n    \"{name}\": {{\n      \"commandName\": \"Project\",\n      \"applicationUrl\": \"http://localhost:{port}\"\n    }}\n  }}\n}}\n"
        );
        write(dir.join("Properties").join("launchSettings.json"), &settings);
    }
    for (dir, scripts) in [
        ("shop", format!(r#"{{ "dev": "node serve.js {SHOP_PORT}" }}"#)),
        ("docs", r#"{ "watch": "node watch.js" }"#.to_string()),
        ("console", format!(r#"{{ "dev": "node serve.js {CONSOLE_PORT}" }}"#)), // no dev:mock: its second variant runs
    ] {
        let dir = path.join("web").join(dir);
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        write(
            dir.join("package.json"),
            &format!("{{ \"name\": \"{name}\", \"private\": true, \"scripts\": {scripts} }}\n"),
        );
        write(dir.join("node_modules").join(".fake"), "");
        write(
            dir.join("serve.js"),
            "require('http').createServer((q, s) => s.end('ok')).listen(Number(process.argv[2]));\n",
        );
        write(dir.join("watch.js"), "setInterval(() => {}, 1 << 30);\n");
    }
    write(path.join(".gitignore"), "bin/\nobj/\ndist/\nnode_modules/\n");
    let _ = std::process::Command::new("git").args(["init", "-q", "-b", "main"]).current_dir(path).output();
    path.to_path_buf()
}
