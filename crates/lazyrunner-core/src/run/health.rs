//! Health (#35): each running app against its targets (the profile's `"health"`, else the defaults) - p95 latency
//! and the 5xx rate over the last minute (the .NET apps whose requests are logged), memory, CPU, and memory that keeps
//! growing. A breach that lasts [`SUSTAIN`] is an alert until the app recovers; `a` acknowledges it.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::profile::Health;

/// How long a breach lasts before it's an alert: one slow request isn't one.
pub const SUSTAIN: Duration = Duration::from_secs(60);
/// The fewest requests in a minute a 5xx rate is judged on.
pub const FEWEST_REQUESTS: usize = 10;
/// Memory that grew more than this much over the last minute keeps growing.
pub const GROWTH: f64 = 0.2;
/// The samples of p95 and 5xx rate kept for each app's sparklines, one per metrics sample.
pub const SERIES: usize = 30;
/// How long a recovery shows on its card.
pub const RECOVERY_SHOWS: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Metric {
    P95,
    Errors,
    Memory,
    Cpu,
    Growth,
}

impl Metric {
    pub const ALL: [Self; 5] = [Self::P95, Self::Errors, Self::Memory, Self::Cpu, Self::Growth];

    pub fn name(self) -> &'static str {
        match self {
            Self::P95 => "p95",
            Self::Errors => "5xx",
            Self::Memory => "memory",
            Self::Cpu => "cpu",
            Self::Growth => "memory growth",
        }
    }

    /// A reading as it's said: `420 ms`, `2.5%`, `812 MB`, `12%`, `+25%`.
    pub fn say(self, value: f64) -> String {
        match self {
            Self::P95 => format!("{value:.0} ms"),
            Self::Errors => format!("{:.1}%", value * 100.0),
            Self::Memory => format!("{value:.0} MB"),
            Self::Cpu => format!("{value:.0}%"),
            Self::Growth => format!("+{:.0}%", value * 100.0),
        }
    }

    /// Its target for an app: the line a reading crosses.
    pub fn target(self, health: &Health) -> f64 {
        match self {
            Self::P95 => f64::from(health.p95_ms),
            Self::Errors => health.error_rate(),
            Self::Memory => f64::from(health.memory_mb),
            Self::Cpu => f64::from(health.cpu_percent),
            Self::Growth => GROWTH,
        }
    }

    /// How long a breach of it lasts before it's an alert: memory growth is measured over a minute already.
    pub fn sustain(self) -> Duration {
        if self == Self::Growth { Duration::ZERO } else { SUSTAIN }
    }
}

/// A reading over its target: since when, the latest reading and target, and once it lasted, when it became an alert
/// and whether it's acknowledged.
#[derive(Clone, Debug, PartialEq)]
pub struct Breach {
    pub app: String,
    pub metric: Metric,
    pub since: Instant,
    pub value: f64,
    pub target: f64,
    pub fired: Option<Instant>,
    pub acknowledged: bool,
}

impl Breach {
    pub fn alert(&self) -> bool {
        self.fired.is_some()
    }

    /// How far it is toward being an alert, 0 to 1.
    pub fn progress(&self, now: Instant) -> f64 {
        let sustain = self.metric.sustain().as_secs_f64();
        if sustain == 0.0 { 1.0 } else { (now.saturating_duration_since(self.since).as_secs_f64() / sustain).min(1.0) }
    }
}

/// What a step changed: a breach became an alert, or an alert's app recovered after so long.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Fired(Breach),
    Recovered(Breach, Duration),
}

/// The breaches and alerts, each app's p95 and 5xx rate over time, and the recent recoveries.
#[derive(Debug, Default)]
pub struct Watch {
    pub breaches: Vec<Breach>,
    pub p95: HashMap<String, VecDeque<f32>>,
    pub errors: HashMap<String, VecDeque<f32>>,
    pub recovered: Vec<(String, Metric, Instant)>,
}

impl Watch {
    /// A reading of `metric` for `app` (value, target; none: nothing to judge): a breach starts, lasts, becomes an
    /// alert, or ends.
    pub fn step(&mut self, app: &str, metric: Metric, reading: Option<(f64, f64)>, now: Instant) -> Option<Change> {
        let at = self.breaches.iter().position(|breach| breach.app == app && breach.metric == metric);
        let over = reading.filter(|(value, target)| value > target);
        let at = match (at, over) {
            (None, None) => return None,
            (Some(at), None) => {
                let breach = self.breaches.remove(at);
                if !breach.alert() {
                    return None;
                }
                self.recovered.push((app.to_string(), metric, now));
                let lasted = now.saturating_duration_since(breach.since);
                return Some(Change::Recovered(breach, lasted));
            }
            (None, Some((value, target))) => {
                let breach =
                    Breach { app: app.into(), metric, since: now, value, target, fired: None, acknowledged: false };
                self.breaches.push(breach);
                self.breaches.len() - 1
            }
            (Some(at), Some((value, target))) => {
                (self.breaches[at].value, self.breaches[at].target) = (value, target);
                at
            }
        };
        let breach = &mut self.breaches[at];
        if breach.alert() || now.saturating_duration_since(breach.since) < metric.sustain() {
            return None;
        }
        breach.fired = Some(now);
        Some(Change::Fired(breach.clone()))
    }

    /// An app that stopped: its breaches go without a word, its series start again.
    pub fn forget(&mut self, app: &str) {
        self.breaches.retain(|breach| breach.app != app);
        self.p95.remove(app);
        self.errors.remove(app);
    }

    /// Acknowledges `app`'s alerts until it recovers; how many there were.
    pub fn acknowledge(&mut self, app: &str) -> usize {
        let mine =
            self.breaches.iter_mut().filter(|breach| breach.app == app && breach.alert() && !breach.acknowledged);
        mine.map(|breach| breach.acknowledged = true).count()
    }

    pub fn alerts(&self) -> impl Iterator<Item = &Breach> {
        self.breaches.iter().filter(|breach| breach.alert())
    }

    /// The breaches of an app, alerts or not yet.
    pub fn of<'a>(&'a self, app: &'a str) -> impl Iterator<Item = &'a Breach> {
        self.breaches.iter().filter(move |breach| breach.app == app)
    }

    /// Whether `app`'s `metric` recovered a moment ago (its card glints).
    pub fn just_recovered(&self, app: &str, metric: Metric, now: Instant) -> bool {
        self.recovered.iter().any(|(key, which, at)| {
            key == app && *which == metric && now.saturating_duration_since(*at) < RECOVERY_SHOWS
        })
    }

    /// Keeps a sample of `app`'s p95 (none: no requests) and 5xx rate.
    pub fn sample(&mut self, app: &str, p95: Option<f64>, errors: Option<f64>, now: Instant) {
        let push = |series: &mut HashMap<String, VecDeque<f32>>, value: Option<f64>| {
            let kept = series.entry(app.to_string()).or_default();
            kept.push_back(value.unwrap_or(0.0) as f32);
            while kept.len() > SERIES {
                kept.pop_front();
            }
        };
        push(&mut self.p95, p95);
        push(&mut self.errors, errors);
        self.recovered.retain(|(_, _, at)| now.saturating_duration_since(*at) < RECOVERY_SHOWS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_breach_is_an_alert_once_it_lasts_and_a_recovery_says_how_long_it_was() {
        let mut watch = Watch::default();
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        assert_eq!(watch.step("orders", Metric::P95, Some((900.0, 500.0)), at(0)), None);
        assert_eq!(watch.step("orders", Metric::P95, Some((800.0, 500.0)), at(30)), None, "not for long enough");
        assert!((watch.breaches[0].progress(at(30)) - 0.5).abs() < 1e-9);
        let Some(Change::Fired(fired)) = watch.step("orders", Metric::P95, Some((950.0, 500.0)), at(60)) else {
            panic!("a minute over: an alert")
        };
        assert_eq!((fired.value, fired.target), (950.0, 500.0));
        assert_eq!(watch.step("orders", Metric::P95, Some((990.0, 500.0)), at(61)), None, "it fires once");
        assert_eq!(watch.acknowledge("orders"), 1);
        assert!(watch.alerts().all(|alert| alert.acknowledged));
        let Some(Change::Recovered(_, lasted)) = watch.step("orders", Metric::P95, Some((200.0, 500.0)), at(240))
        else {
            panic!("back under: recovered")
        };
        assert_eq!(lasted, Duration::from_secs(240));
        assert!(watch.just_recovered("orders", Metric::P95, at(241)));
        assert_eq!(watch.alerts().count(), 0);
    }

    #[test]
    fn a_short_breach_ends_quietly_and_memory_growth_is_an_alert_at_once() {
        let mut watch = Watch::default();
        let now = Instant::now();
        watch.step("orders", Metric::Cpu, Some((95.0, 90.0)), now);
        assert_eq!(watch.step("orders", Metric::Cpu, Some((20.0, 90.0)), now + Duration::from_secs(5)), None);
        assert!(watch.breaches.is_empty() && watch.recovered.is_empty(), "a near miss isn't a recovery");
        let fired = watch.step("orders", Metric::Growth, Some((0.3, GROWTH)), now);
        assert!(matches!(fired, Some(Change::Fired(_))), "{fired:?}");
        watch.forget("orders");
        assert!(watch.breaches.is_empty(), "a stopped app's alerts go");
    }
}
