//! How fast an app logged each row's share of its lines, as the braille the rail's ordinary rows are drawn with: four
//! steps a row, each a dot on the left at the column's usual pace (so a quiet stretch reads as a dotted line) and one on
//! the right too where it logged faster; a row with a burst in it is drawn brighter. A row stands for as many lines as
//! any other, so how fast they came is what tells a burst from a quiet stretch.

use std::time::{Duration, Instant};

use super::columns::Shown;

/// The braille dots of each of a cell's four steps: the left one, and both.
const LEFT: [u32; 4] = [0x01, 0x02, 0x04, 0x40];
const BOTH: [u32; 4] = [0x09, 0x12, 0x24, 0xC0];
const STEPS: usize = LEFT.len();
const BRAILLE: u32 = 0x2800;
/// Against the column's usual pace (its median step's: an app that logs in groups with quiet between them isn't
/// faster than usual all the time): from this much faster a step is busy, from this much a burst.
const BUSY: f64 = 2.0;
const BURST: f64 = 8.0;
/// Lines that came at the same moment still took this long: no step is infinitely fast.
const SHORTEST: Duration = Duration::from_millis(1);

/// A row's braille, and whether a burst is in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Density {
    pub glyph: char,
    pub burst: bool,
}

/// The braille for each of `rows` rows of the rail.
pub fn strip(list: &[Shown], rows: usize) -> Vec<Density> {
    let paces = paces(list, rows * STEPS);
    (0..rows)
        .map(|row| {
            let steps = &paces[row * STEPS..(row + 1) * STEPS];
            let dots = steps.iter().enumerate().fold(0, |dots, (step, pace)| match pace {
                Some(pace) if *pace >= BUSY => dots | BOTH[step],
                Some(_) => dots | LEFT[step],
                None => dots,
            });
            Density {
                glyph: char::from_u32(BRAILLE + dots).unwrap_or(' '),
                burst: steps.iter().flatten().any(|pace| *pace >= BURST),
            }
        })
        .collect()
}

/// Each step's pace against the column's usual one (`None`: no lines at all).
fn paces(list: &[Shown], steps: usize) -> Vec<Option<f64>> {
    let mut spans: Vec<Option<(Instant, Instant, usize)>> = vec![None; steps];
    for (at, item) in list.iter().enumerate() {
        let step = at * steps / list.len().max(1);
        let born = item.line.born;
        spans[step] = Some(spans[step].map_or((born, born, 1), |(first, _, count)| (first, born, count + 1)));
    }
    let mut previous: Option<Instant> = None;
    let rates: Vec<Option<f64>> = spans
        .iter()
        .map(|span| {
            let (first, last, count) = (*span)?;
            // A step of one line is as fast as the wait since the line before it; the very first one's is unknown.
            let since = if count > 1 { Some(first) } else { previous };
            previous = Some(last);
            Some(rate(count, last.saturating_duration_since(since?)))
        })
        .collect();
    let mut known: Vec<f64> = rates.iter().flatten().copied().collect();
    known.sort_by(f64::total_cmp);
    let Some(&usual) = known.get(known.len() / 2) else { return rates };
    // A step with no lines (fewer lines than steps) or an unknown pace goes on at the pace before it - the first ones
    // at the first known - so the line stays unbroken.
    let first = rates.iter().flatten().next().map(|rate| rate / usual);
    let mut before = first;
    rates
        .into_iter()
        .map(|rate| {
            before = rate.map(|rate| rate / usual).or(before);
            before
        })
        .collect()
}

fn rate(lines: usize, took: Duration) -> f64 {
    lines as f64 / took.max(SHORTEST).as_secs_f64()
}

#[cfg(test)]
mod tests {
    use lazyrunner_core::output::{Level, OutputLine};

    use super::*;

    const QUIET: Density = Density { glyph: '⡇', burst: false };

    /// The strip of `rows` rows for lines that came at these times (milliseconds in).
    fn strip_of(times: &[u64], rows: usize) -> Vec<Density> {
        let start = Instant::now();
        let lines: Vec<OutputLine> = times
            .iter()
            .map(|millis| OutputLine {
                text: String::new(),
                level: Level::Info,
                born: start + Duration::from_millis(*millis),
                hue: None,
            })
            .collect();
        let list: Vec<Shown> =
            lines.iter().enumerate().map(|(position, line)| Shown { position, line, context: false }).collect();
        strip(&list, rows)
    }

    #[test]
    fn a_burst_among_steady_lines_is_the_one_row_that_stands_out() {
        // A line a second, then 20 at the same moment (a compiler's report), then a line a second again.
        let times: Vec<u64> =
            (0..60).map(|n| n * 1000).chain([60_000; 20]).chain((0..20).map(|n| 61_000 + n * 1000)).collect();
        assert_eq!(strip_of(&times, 5), [QUIET, QUIET, QUIET, Density { glyph: '⣿', burst: true }, QUIET]);
    }

    #[test]
    fn an_app_that_logs_in_groups_with_quiet_between_reads_as_its_usual_pace() {
        // Five lines at a time every five seconds, as a queue that is polled: all of it is the app's usual pace.
        let times: Vec<u64> = (0..100).map(|n| n / 5 * 5000).collect();
        assert_eq!(strip_of(&times, 5), [QUIET; 5]);
    }

    #[test]
    fn fewer_lines_than_steps_still_draw_an_unbroken_line() {
        let times: Vec<u64> = (0..7).map(|n| n * 1000).collect();
        assert_eq!(strip_of(&times, 5), [QUIET; 5]);
    }
}
