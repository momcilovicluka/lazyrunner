//! For the tests: a pull's credential store, and pulls pretended for the screen's tests (#110, #114).

use std::sync::Arc;
use std::time::Instant;

use super::Controller;
use super::pull::{PullRun, StepRun};
use crate::profile::sources::DatabaseSource;
use crate::run::database_files::Progress;
use crate::run::pull::{Mode, State, Step};

impl Controller {
    /// For the tests: the credential store a pull reads its passwords from.
    #[cfg(any(test, feature = "test-support"))]
    pub fn use_credentials(&mut self, store: Arc<dyn crate::run::credentials::Store>) {
        self.credentials = store;
    }

    /// For the tests of the screen: a pull as if it had run since `started`, its steps where `states` says (in the
    /// mode's order), the running one's progress `progress`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pretend_pull(
        &mut self,
        (source, database): (&str, &str),
        started: Instant,
        states: &[State],
        progress: Progress,
    ) {
        self.pretend_job(Mode::Pull, (source, database), started, states, progress);
    }

    /// For the tests of the screen: a pull of `mode`, as [`Controller::pretend_pull`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn pretend_job(
        &mut self,
        mode: Mode,
        (source, database): (&str, &str),
        started: Instant,
        states: &[State],
        progress: Progress,
    ) {
        let job = self.new_db_job();
        let mut steps = vec![StepRun::default(); Step::ALL.len()];
        for (step, state) in mode.steps().iter().zip(states) {
            let run = &mut steps[step.index()];
            run.state = *state;
            if *state != State::Waiting {
                run.started = Some(started);
            }
        }
        let apps = self.apps.iter().filter(|app| app.traits().uses_secrets).map(|app| (app.key.clone(), 0)).collect();
        let found = self.source(source);
        let source = found.as_ref().map_or_else(|| source.to_string(), |found| found.name.clone());
        let remote =
            found.as_ref().and_then(|found| found.database().map(String::from)).unwrap_or_else(|| "Shop".into());
        self.pull = Some(PullRun {
            job,
            mode,
            from: found.as_ref().map(DatabaseSource::label).unwrap_or_default(),
            source,
            remote,
            server: "localhost".into(),
            database: database.into(),
            file: String::new(),
            started,
            ended: None,
            steps,
            share: progress.share.map(|share| (share, share, started)),
            progress,
            error: None,
            cancelled: false,
            apps,
            target: (mode == Mode::Update).then(|| "Shop".into()),
            old: (mode == Mode::Update).then(|| "Shop_old_20261127".into()),
            cancel: Arc::default(),
            migrating: None,
            before: None,
            local: String::new(),
            stopped: Vec::new(),
        });
    }

    /// For the tests of the screen: the pretended pull's step `step` failed (`why`) or, without one, all done.
    #[cfg(any(test, feature = "test-support"))]
    pub fn pretend_pull_over(&mut self, failed: Option<(Step, &str)>, at: Instant) {
        let Some(pull) = self.pull.as_mut() else { return };
        pull.ended = Some(at);
        for (index, step) in pull.mode.steps().iter().enumerate() {
            let started = pull.started;
            let run = &mut pull.steps[step.index()];
            run.ended = run.ended.or(run.started.map(|_| at));
            if failed.is_none() {
                run.state = State::Done;
                run.started = run.started.or(Some(started));
                run.ended = Some(started + std::time::Duration::from_secs(10 * (index as u64 + 1)));
            }
        }
        if let Some((step, why)) = failed {
            pull.step_mut(step).state = State::Failed;
            pull.error = Some((step, why.to_string()));
        }
    }
}
