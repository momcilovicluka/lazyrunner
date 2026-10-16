//! Where lazyrunner keeps things on this machine. Each has an environment variable that replaces it, so tests run
//! beside a real session without touching its files:
//!
//! | what | Windows | macOS | Linux | variable |
//! |---|---|---|---|---|
//! | settings | `%APPDATA%\lazyrunner\lazyrunner.config.json` | `~/Library/Application Support/lazyrunner/…` | `$XDG_CONFIG_HOME/lazyrunner/…` | `LAZYRUNNER_CONFIG_PATH` |
//! | your profiles | `%APPDATA%\lazyrunner\projects` | `…/lazyrunner/projects` | `$XDG_CONFIG_HOME/lazyrunner/projects` | `LAZYRUNNER_PROFILES_DIR` |
//! | state | `%LOCALAPPDATA%\lazyrunner` | `…/lazyrunner/state` | `$XDG_STATE_HOME/lazyrunner` | `LAZYRUNNER_STATE_DIR` |
//!
//! `LAZYRUNNER_PROFILE` (a file) makes that the only profile.

use std::path::{Path, PathBuf};

use crate::profile::ProfileDirs;

pub const CONFIG_FILE: &str = "lazyrunner.config.json";
pub const TRUST_STORE_FILE: &str = "trusted-profiles.json";

#[derive(Clone, Debug)]
pub struct Paths {
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
    pub profiles: ProfileDirs,
}

impl Paths {
    /// From the environment and the OS's folders. `install_dir` holds the bundled profiles (`projects/`).
    pub fn from_env(install_dir: &Path) -> Self {
        Self::resolve(
            |name| std::env::var_os(name).map(PathBuf::from).filter(|path| !path.as_os_str().is_empty()),
            install_dir,
        )
    }

    pub fn resolve(var: impl Fn(&str) -> Option<PathBuf>, install_dir: &Path) -> Self {
        // Only absolute folders: a relative one would put your profiles (which need no trust) and the trust store in
        // whatever folder lazyrunner runs in, which can be an untrusted repo.
        let var = |name: &str| var(name).filter(|path| path.is_absolute());
        let home = var("HOME")
            .or_else(|| var("USERPROFILE"))
            .or_else(|| std::env::home_dir().filter(|home| home.is_absolute()))
            .unwrap_or_else(|| std::env::temp_dir().join("lazyrunner-home"));
        let (config_dir, state_dir) = os_dirs(&var, &home);
        Self {
            config_file: var("LAZYRUNNER_CONFIG_PATH").unwrap_or_else(|| config_dir.join(CONFIG_FILE)),
            state_dir: var("LAZYRUNNER_STATE_DIR").unwrap_or(state_dir),
            profiles: ProfileDirs {
                user: var("LAZYRUNNER_PROFILES_DIR").unwrap_or_else(|| config_dir.join("projects")),
                bundled: install_dir.join("projects"),
                only: var("LAZYRUNNER_PROFILE"),
            },
        }
    }

    pub fn trust_store(&self) -> PathBuf {
        self.state_dir.join(TRUST_STORE_FILE)
    }

    /// A project's own state folder (build stamps, logs, timings), made on first use.
    pub fn project_state(&self, id: &str) -> std::io::Result<PathBuf> {
        let dir = self.state_dir.join(id);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

/// (settings folder, state folder) for this OS.
fn os_dirs(var: &impl Fn(&str) -> Option<PathBuf>, home: &Path) -> (PathBuf, PathBuf) {
    if cfg!(windows) {
        let roaming = var("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"));
        let local = var("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"));
        (roaming.join("lazyrunner"), local.join("lazyrunner"))
    } else if cfg!(target_os = "macos") {
        let base = home.join("Library").join("Application Support").join("lazyrunner");
        (base.clone(), base.join("state"))
    } else {
        let config = var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let state = var("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local").join("state"));
        (config.join("lazyrunner"), state.join("lazyrunner"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<PathBuf> {
        let pairs: Vec<(String, PathBuf)> = pairs.iter().map(|(k, v)| (k.to_string(), PathBuf::from(v))).collect();
        move |name| pairs.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone())
    }

    #[test]
    fn the_variables_replace_every_folder() {
        let t = std::env::temp_dir().join("t");
        let at = |name: &str| t.join(name).to_string_lossy().into_owned();
        let (config, state, profiles, one) = (at("config.json"), at("state"), at("profiles"), at("one.json"));
        let paths = Paths::resolve(
            env(&[
                ("LAZYRUNNER_CONFIG_PATH", &config),
                ("LAZYRUNNER_STATE_DIR", &state),
                ("LAZYRUNNER_PROFILES_DIR", &profiles),
                ("LAZYRUNNER_PROFILE", &one),
            ]),
            Path::new("/opt/lazyrunner"),
        );
        assert_eq!(paths.config_file, t.join("config.json"));
        assert_eq!(paths.trust_store(), t.join("state").join("trusted-profiles.json"));
        assert_eq!(paths.profiles.user, t.join("profiles"));
        assert_eq!(paths.profiles.only, Some(t.join("one.json")));
        assert_eq!(paths.profiles.bundled, Path::new("/opt/lazyrunner").join("projects"));
    }

    #[test]
    fn without_them_the_os_folders() {
        let paths = Paths::resolve(
            env(&[
                ("HOME", "/home/me"),
                ("APPDATA", "C:/Users/me/AppData/Roaming"),
                ("LOCALAPPDATA", "C:/Users/me/AppData/Local"),
            ]),
            Path::new("/opt/lazyrunner"),
        );
        if cfg!(windows) {
            assert_eq!(paths.config_file, Path::new("C:/Users/me/AppData/Roaming/lazyrunner").join(CONFIG_FILE));
            assert_eq!(paths.state_dir, Path::new("C:/Users/me/AppData/Local/lazyrunner"));
        } else if cfg!(target_os = "macos") {
            assert!(paths.config_file.starts_with("/home/me/Library/Application Support/lazyrunner"));
        } else {
            assert_eq!(paths.config_file, Path::new("/home/me/.config/lazyrunner").join(CONFIG_FILE));
            assert_eq!(paths.state_dir, Path::new("/home/me/.local/state/lazyrunner"));
        }
        assert!(paths.profiles.user.ends_with("projects"));
        assert_eq!(paths.profiles.only, None);
    }

    #[test]
    fn relative_folders_are_ignored() {
        let paths = Paths::resolve(
            env(&[("HOME", "."), ("XDG_CONFIG_HOME", "rel"), ("LAZYRUNNER_STATE_DIR", "state")]),
            Path::new("/opt"),
        );
        assert!(paths.config_file.is_absolute() && paths.state_dir.is_absolute() && paths.profiles.user.is_absolute());
    }

    #[test]
    fn each_project_gets_its_own_state_folder() {
        let root = std::env::temp_dir().join(format!("lazyrunner-paths-{}", std::process::id()));
        let paths = Paths::resolve(env(&[("LAZYRUNNER_STATE_DIR", root.to_str().unwrap())]), Path::new("."));
        let sample = paths.project_state("sample").unwrap();
        let other = paths.project_state("other").unwrap();
        assert!(sample.is_dir() && other.is_dir() && sample != other);
        assert_eq!(sample.parent(), Some(root.as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
