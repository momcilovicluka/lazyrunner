//! Health (#35) in the controller: once a second every app that's up is read against its targets; an alert is
//! logged, notified and flashes the app's title twice, and so its recovery is logged.

use std::time::{Duration, Instant};

use super::Controller;
use crate::output::Hue;
use crate::run::format_duration;
use crate::run::health::{Breach, Change, FEWEST_REQUESTS, Metric};

/// A new alert's title flash: two pulses.
const ALERT_FLASH: Duration = Duration::from_millis(2000);

impl Controller {
    /// Every app that's up against its targets: breaches start and end, alerts fire and recover. A stopped app's go.
    pub(super) fn tick_health(&mut self, now: Instant) {
        for index in 0..self.apps.len() {
            let key = self.apps[index].key.clone();
            if !self.states[index].is_up() {
                self.health.forget(&key);
                continue;
            }
            for metric in Metric::ALL {
                let reading = self.reading(index, metric);
                match self.health.step(&key, metric, reading, now) {
                    Some(Change::Fired(breach)) => self.alerted(index, &breach, now),
                    Some(Change::Recovered(breach, lasted)) => self.recovered(index, &breach, lasted),
                    None => {}
                }
            }
        }
    }

    /// App `index`'s reading of `metric` and its target; none when there's nothing to judge (no requests logged, too
    /// few of them, no sample yet).
    pub fn reading(&self, index: usize, metric: Metric) -> Option<(f64, f64)> {
        let (app, state) = (&self.apps[index], &self.states[index]);
        let value = match metric {
            Metric::P95 | Metric::Errors if !app.traits().counts_requests => None,
            Metric::P95 => self.served_last_minute(&app.key).p95_ms,
            Metric::Errors => {
                let minute = self.served_last_minute(&app.key);
                (minute.count >= FEWEST_REQUESTS).then(|| minute.failed as f64 / minute.count as f64)
            }
            Metric::Memory => state.memory_mb.map(f64::from),
            Metric::Cpu => state.cpu.map(f64::from),
            Metric::Growth => state.memory_growth(),
        };
        value.map(|value| (value, metric.target(&app.health)))
    }

    fn alerted(&mut self, index: usize, breach: &Breach, now: Instant) {
        let name = self.apps[index].name.clone();
        let said = breach.metric.say(breach.value);
        let (log, title, message) = match breach.metric {
            Metric::Growth => (
                format!("! {name}'s memory keeps growing: {said} over the last minute"),
                format!("{name}'s memory keeps growing"),
                format!("{said} over the last minute"),
            ),
            metric => {
                let (what, target) = (metric.name(), metric.say(breach.target));
                let lasted = format_duration(metric.sustain().as_secs_f64());
                (
                    format!("! {name} {what} {said}, over its {target} target for {lasted}"),
                    format!("{name} {what} {said}"),
                    format!("over its {target} target for {lasted}"),
                )
            }
        };
        self.add_log(log, Hue::Red);
        self.notifications.push((title, message));
        self.states[index].flash_until = Some(now + ALERT_FLASH);
    }

    fn recovered(&mut self, index: usize, breach: &Breach, lasted: Duration) {
        let name = self.apps[index].name.clone();
        let after = format_duration(lasted.as_secs_f64());
        let log = match breach.metric {
            Metric::Growth => format!("✓ {name}'s memory stopped growing after {after}"),
            metric => format!("✓ {name} {} back under {} after {after}", metric.name(), metric.say(breach.target)),
        };
        self.add_log(log, Hue::Green);
    }

    /// `a` on the Health tab: app `key`'s alerts are acknowledged until it recovers.
    pub fn acknowledge_alerts(&mut self, key: &str) {
        let name = self.app(key).map_or_else(|| key.to_string(), |app| app.name.clone());
        let text = match self.health.acknowledge(key) {
            0 => format!("{name} has no alert to acknowledge."),
            1 => format!("{name}'s alert acknowledged: it stays amber until {name} recovers."),
            count => format!("{name}'s {count} alerts acknowledged: they stay amber until {name} recovers."),
        };
        self.add_log(text, Hue::DarkGray);
    }

    /// Each app's p95 and 5xx rate over the last minute, with every metrics sample: the Health tab's sparklines.
    pub(super) fn sample_health(&mut self, now: Instant) {
        for index in 0..self.apps.len() {
            if !self.apps[index].traits().counts_requests || !self.states[index].is_up() {
                continue;
            }
            let key = self.apps[index].key.clone();
            let minute = self.served_last_minute(&key);
            let errors = (minute.count > 0).then(|| minute.failed as f64 / minute.count as f64);
            self.health.sample(&key, minute.p95_ms, errors, now);
        }
    }
}
