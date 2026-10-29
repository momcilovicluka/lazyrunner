//! `lazyrunner hook test <event>`: runs the hooks in your settings that wait for an event, as if it happened, and
//! says how each went.

use anyhow::{Result, bail};
use lazyrunner_core::config::ConfigFile;
use lazyrunner_core::paths::Paths;
use lazyrunner_core::run::hooks::{self, EVENTS, Happening};

use crate::profile_cli::install_dir;

pub fn run(action: &str, event: &str, app: Option<&str>) -> Result<()> {
    if action != "test" {
        bail!("lazyrunner hook test <event> [--app <key>] - the events: {}", EVENTS.join(", "));
    }
    let Some(&event) = EVENTS.iter().find(|known| **known == event) else {
        bail!("{event} isn't an event: {}", EVENTS.join(", "));
    };
    let paths = Paths::from_env(&install_dir());
    let (all, problems) = hooks::read(&ConfigFile::new(&paths.config_file).load(None));
    for problem in problems {
        eprintln!("{problem}");
    }
    let happening = Happening { event, app: app.map(str::to_string), details: "lazyrunner hook test".into() };
    let found: Vec<_> = hooks::matching(&all, &happening).collect();
    if found.is_empty() {
        println!("No hook waits for {event} - \"hooks\" in {}", paths.config_file.display());
        return Ok(());
    }
    let folder = std::env::current_dir()?;
    let project = folder.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
    let mut failed = 0;
    for hook in found {
        match hooks::run(hook, &folder, &project, &happening, hooks::TIMEOUT) {
            None => println!("✓ {}", hook.run),
            Some(problem) => {
                failed += 1;
                println!("✗ {}: {problem}", hook.run);
            }
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
