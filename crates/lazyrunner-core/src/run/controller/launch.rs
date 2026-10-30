//! What an app runs on, picked with `x` then `p`: a .NET app's launch profile, an npm app's variant. Remembered per app
//! and branch in the project's state folder (`launch-choices.json`), not the profile, which is shared; the profile's
//! own is the default, and choosing it again forgets the choice. The apps lazyrunner runs carry the choice, so their
//! ports, starts and restarts follow it.

use serde_json::{Map, Value, json};

use super::Controller;
use crate::apptypes;
use crate::output::Hue;
use crate::profile::{AppKind, Variant};

const FILE: &str = "launch-choices.json";

/// One thing an app can run on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub name: String,
    /// Its URL, or its npm script and port.
    pub detail: String,
    /// What the app runs on now.
    pub current: bool,
    /// The profile's own (an npm app's: whichever of its variants applies first).
    pub default: bool,
}

/// An npm app's default: no variant pinned.
pub const AUTOMATIC: &str = "automatic";

impl Controller {
    /// What app `key` can run on, in order.
    pub fn launch_choices(&self, key: &str) -> Vec<Choice> {
        let (Some(app), Some(original)) = (self.app(key), self.project.apps.iter().find(|app| app.key == key)) else {
            return Vec::new();
        };
        let chosen = self.launch_choice(key);
        match (&app.kind, &original.kind) {
            (AppKind::Dotnet { launch_profile: running, .. }, AppKind::Dotnet { launch_profile: own, .. }) => {
                apptypes::launch_profiles(&self.root, original)
                    .into_iter()
                    .map(|(name, url)| Choice {
                        current: name.eq_ignore_ascii_case(running),
                        default: name.eq_ignore_ascii_case(own),
                        detail: url.unwrap_or_default(),
                        name,
                    })
                    .collect()
            }
            (AppKind::Npm { .. }, AppKind::Npm { .. }) => {
                let automatic = Choice {
                    name: AUTOMATIC.into(),
                    detail: "the first of its variants this branch has".into(),
                    current: chosen.is_none(),
                    default: true,
                };
                let variants = apptypes::available_variants(&self.root, original).into_iter().map(|variant| Choice {
                    current: chosen == Some(variant_name(&variant)),
                    detail: variant_detail(&variant),
                    name: variant_name(&variant).to_string(),
                    default: false,
                });
                std::iter::once(automatic).chain(variants).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The choice for app `key` on this branch, when it isn't the profile's own.
    pub fn launch_choice(&self, key: &str) -> Option<&str> {
        self.launch_chosen.get(key).map(String::as_str)
    }

    /// Runs app `key` on `name` from now on, on this branch (the default forgets the choice), and restarts it when it
    /// runs.
    pub fn choose_launch(&mut self, key: &str, name: &str) {
        let Some(choice) = self.launch_choices(key).into_iter().find(|choice| choice.name == name) else { return };
        let mut all = self.saved_launch_choices();
        let branch = all.entry(self.branch.clone()).or_insert_with(|| json!({}));
        if let Some(apps) = branch.as_object_mut() {
            if choice.default {
                apps.remove(key);
            } else {
                apps.insert(key.to_string(), json!(name));
            }
        }
        if let Err(error) = crate::config::write(&self.state_dir.join(FILE), &Value::Object(all)) {
            self.add_log(format!("✗ Couldn't remember the choice: {error}"), Hue::Red);
        }
        self.apply_launch_choices();
        self.update_modes();
        let app = self.app(key).map(|app| app.name.clone()).unwrap_or_default();
        let on = if choice.default { format!("its own ({name})") } else { name.to_string() };
        if self.is_running(key) {
            self.restart(&format!("{app} on {on}"), Some(&[key.to_string()]), false);
        } else {
            self.add_log(format!("{app} runs on {on} from its next start"), Hue::Cyan);
        }
    }

    /// The apps lazyrunner runs, with the choices for this branch: a .NET app's launch profile, an npm app's variant
    /// (the only one it has then).
    pub(super) fn apply_launch_choices(&mut self) {
        let all = self.saved_launch_choices();
        let chosen: Map<String, Value> = all.get(&self.branch).and_then(Value::as_object).cloned().unwrap_or_default();
        self.launch_chosen.clear();
        for index in 0..self.apps.len() {
            let key = self.apps[index].key.clone();
            let Some(original) = self.project.apps.iter().find(|app| app.key == key) else { continue };
            let name = chosen.get(&key).and_then(Value::as_str);
            let kind = match (&original.kind, name) {
                (AppKind::Dotnet { project, swagger, args, .. }, Some(name)) => Some(AppKind::Dotnet {
                    project: project.clone(),
                    launch_profile: name.to_string(),
                    swagger: swagger.clone(),
                    args: args.clone(),
                }),
                (AppKind::Npm { dir, variants }, Some(name)) => {
                    variants.iter().find(|variant| variant_name(variant) == name).map(|variant| AppKind::Npm {
                        dir: dir.clone(),
                        variants: vec![Variant { when: None, ..variant.clone() }],
                    })
                }
                _ => None,
            };
            if kind.is_some()
                && let Some(name) = name
            {
                self.launch_chosen.insert(key.clone(), name.to_string());
            }
            self.apps[index].kind = kind.unwrap_or_else(|| original.kind.clone());
            if !self.apps[index].traits().variants {
                let port = apptypes::port(&self.root, &self.apps[index], &self.apps);
                self.ports.insert(key, port);
            }
        }
    }

    fn saved_launch_choices(&self) -> Map<String, Value> {
        crate::json::read(&self.state_dir.join(FILE)).and_then(|value| value.as_object().cloned()).unwrap_or_default()
    }
}

/// How a variant is named in the picker and the choices file: its label, else its script.
fn variant_name(variant: &Variant) -> &str {
    if variant.label.is_empty() { &variant.script } else { &variant.label }
}

fn variant_detail(variant: &Variant) -> String {
    match variant.port {
        Some(port) => format!("npm run {} · :{port}", variant.script),
        None => format!("npm run {} · a watch build", variant.script),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::super::{Controller, Setup};
    use crate::paths::Paths;
    use crate::profile::{AppKind, ProfileDirs, import};
    use crate::testing::{TempDir, sample_profile, sample_repo, write};

    #[tokio::test]
    async fn a_chosen_launch_profile_is_what_the_app_runs_on_this_branch_and_the_default_forgets_it() {
        let temp = TempDir::new("launch-choice");
        let repo = sample_repo(&temp.join("repo"));
        let paths = Paths {
            config_file: temp.join("config.json"),
            state_dir: temp.join("state"),
            profiles: ProfileDirs {
                user: temp.join("profiles"),
                bundled: temp.join("bundled"),
                only: Some(sample_profile()),
            },
        };
        crate::config::write(&paths.config_file, &json!({})).unwrap();
        let project = import(&sample_profile(), &paths.profiles).unwrap();
        let (events, _rx) = mpsc::unbounded_channel();
        let mut controller =
            Controller::new(Setup { project, root: repo.clone(), paths, events, guard: None }).unwrap();
        let AppKind::Dotnet { project: csproj, launch_profile: own, .. } = controller.apps[0].kind.clone() else {
            panic!("Orders is a .NET app")
        };
        let settings =
            repo.join(std::path::Path::new(&csproj).parent().unwrap()).join("Properties/launchSettings.json");
        let mut profiles = serde_json::Map::new();
        profiles.insert(own.clone(), json!({ "commandName": "Project", "applicationUrl": "http://localhost:47201" }));
        profiles.insert(
            "Orders-Https".into(),
            json!({ "commandName": "Project", "applicationUrl": "https://localhost:47211" }),
        );
        profiles.insert("IIS Express".into(), json!({ "commandName": "IISExpress" }));
        write(&settings, &json!({ "profiles": profiles }).to_string());
        let key = controller.apps[0].key.clone();
        let names: Vec<String> = controller.launch_choices(&key).into_iter().map(|choice| choice.name).collect();
        assert_eq!(names, [own.clone(), "Orders-Https".into()], "the profiles dotnet run can run, not IIS Express");

        controller.choose_launch(&key, "Orders-Https");
        assert!(
            matches!(&controller.apps[0].kind, AppKind::Dotnet { launch_profile, .. } if launch_profile == "Orders-Https")
        );
        assert_eq!(controller.port(&key), Some(47211), "its port follows");
        assert_eq!(controller.launch_choice(&key), Some("Orders-Https"));
        let said = controller.log.back().map(|entry| entry.text.clone()).unwrap_or_default();
        assert!(said.contains("from its next start"), "{said}");

        let main = std::mem::replace(&mut controller.branch, "feature".into());
        controller.apply_launch_choices();
        assert_eq!(controller.launch_choice(&key), None, "another branch runs the profile's own");
        controller.branch = main;
        controller.apply_launch_choices();
        assert_eq!(controller.launch_choice(&key), Some("Orders-Https"), "back on its branch, the choice is back");

        controller.choose_launch(&key, &own);
        assert_eq!(controller.launch_choice(&key), None, "choosing its own forgets the choice");
        assert_eq!(controller.port(&key), Some(47201));
    }
}
