//! Noticing a branch that changes the profile: what lazyrunner says when the checkout's profile is no longer the one
//! it runs. The UI asks this after a branch switch (P2/P3 wire it in).

use super::Project;

#[derive(Debug, PartialEq, Eq)]
pub enum Tone {
    Warning,
    Error,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub text: String,
    pub tone: Tone,
}

/// `running` is the profile lazyrunner runs, `active` the keys of its apps that are on on this machine, `now` what
/// the checkout has now (an error message when it doesn't load), and `trusted` whether `now` needs no trust question.
/// `None` when nothing changed.
pub fn describe(
    running: &Project,
    active: &[&str],
    now: Result<Option<&Project>, String>,
    trusted: bool,
) -> Option<Change> {
    let now = match now {
        Err(message) => {
            let one_line = message.lines().map(str::trim).collect::<Vec<_>>().join(" ");
            return Some(Change { text: format!("✗ {one_line}"), tone: Tone::Error });
        }
        Ok(None) => {
            let text = format!(
                "No profile matches this checkout any more - lazyrunner keeps {}'s until you reopen it",
                running.name
            );
            return Some(Change { text, tone: Tone::Warning });
        }
        Ok(Some(now)) if now.path == running.path && now.hash == running.hash => return None,
        Ok(Some(now)) => now,
    };
    let mut changes: Vec<String> = Vec::new();
    changes.extend(now.apps.iter().filter(|app| running.app(&app.key).is_none()).map(|app| format!("+ {}", app.name)));
    changes.extend(running.apps.iter().filter(|app| now.app(&app.key).is_none()).map(|app| format!("- {}", app.key)));
    for app in &running.apps {
        if let Some(new) = now.app(&app.key)
            && active.contains(&app.key.as_str())
            && (new.name != app.name || new.kind != app.kind)
        {
            changes.push(format!("~ {}", new.name));
        }
    }
    let what = if changes.is_empty() { "its settings".to_string() } else { changes.join(", ") };
    let trust = if trusted { "" } else { " - it needs your trust first" };
    let file = now.path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    Some(Change {
        text: format!(
            "This branch changes the project profile ({file}: {what}): , then p reopens lazyrunner with it{trust}"
        ),
        tone: Tone::Warning,
    })
}
