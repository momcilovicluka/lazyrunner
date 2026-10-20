//! A synthetic app for the spike and the benchmarks: `lazyrunner emit` "compiles" for a moment, says it's ready, then
//! prints realistic log lines at a steady rate until it's stopped.

use std::io::{self, Write};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often the emitter wakes up to print the lines it owes; the rate stays exact however coarse the OS timer is.
const TICK: Duration = Duration::from_millis(10);
const COMPILE_STEPS: u32 = 12;
const ROUTES: [&str; 6] = ["orders", "billing", "users", "cart", "search", "reports"];

#[derive(Clone, Debug)]
pub struct EmitterOptions {
    /// Lines per second once ready; 0 prints nothing after "ready" (an idle app).
    pub rate: u32,
    /// How long the "compiling" phase takes.
    pub ready_after: Duration,
    pub seed: u64,
    /// Stop after this many log lines (the tests use it); `None` streams until killed.
    pub limit: Option<u64>,
}

pub fn run(options: &EmitterOptions) -> io::Result<()> {
    match emit(options, &mut io::BufWriter::with_capacity(64 * 1024, io::stdout().lock())) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()), // lazyrunner went away: so do we
        other => other,
    }
}

fn emit(options: &EmitterOptions, out: &mut impl Write) -> io::Result<()> {
    let started = Instant::now();
    for step in 1..=COMPILE_STEPS {
        writeln!(out, "  compiling {}/{COMPILE_STEPS} {}", step, ROUTES[step as usize % ROUTES.len()])?;
        out.flush()?;
        thread::sleep(options.ready_after / COMPILE_STEPS);
    }
    writeln!(out, "ready in {} ms", started.elapsed().as_millis())?;
    out.flush()?;

    let mut rng = Rng::new(options.seed);
    let streaming = Instant::now();
    let mut sent = 0u64;
    loop {
        if options.rate == 0 {
            thread::sleep(Duration::from_secs(60));
            continue;
        }
        let owed = (streaming.elapsed().as_secs_f64() * f64::from(options.rate)) as u64;
        while sent < owed {
            writeln!(out, "{}", log_line(&mut rng))?;
            sent += 1;
            if options.limit.is_some_and(|limit| sent >= limit) {
                return out.flush();
            }
        }
        out.flush()?;
        thread::sleep(TICK);
    }
}

/// One log line, in the shape web servers print them: mostly requests, some debug, a few warnings and errors, and the
/// odd long line that has to wrap.
pub fn log_line(rng: &mut Rng) -> String {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis()) % 86_400_000;
    let time =
        format!("{:02}:{:02}:{:02}.{:03}", millis / 3_600_000, millis / 60_000 % 60, millis / 1000 % 60, millis % 1000);
    let route = ROUTES[rng.below(ROUTES.len() as u64) as usize];
    let id = rng.below(90_000) + 10_000;
    let roll = rng.below(100);
    let mut line = match roll {
        0..=2 => format!("{time} ERR  upstream timeout calling {route} after 3000 ms (request {id})"),
        3..=9 => format!("{time} WRN  slow query on {route}: {} ms", rng.below(900) + 300),
        10..=19 => format!("{time} DBG  cache {} {route}:{id}", if rng.below(2) == 0 { "hit" } else { "miss" }),
        _ => format!("{time} INF  GET /api/{route}/{id} -> 200 in {} ms", rng.below(80) + 2),
    };
    if rng.below(20) == 0 {
        line.push_str(" payload={");
        for field in 0..6 {
            line.push_str(&format!("\"field{field}\":\"{:08x}\",", rng.next_u64()));
        }
        line.push('}');
    }
    line
}

/// A small deterministic generator (xorshift64*), so the same seed prints the same lines.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_the_compile_phase_then_ready_then_exactly_the_limit() {
        let options = EmitterOptions { rate: 50_000, ready_after: Duration::ZERO, seed: 7, limit: Some(25) };
        let mut out = Vec::new();
        emit(&options, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), COMPILE_STEPS as usize + 1 + 25);
        assert!(lines[0].starts_with("  compiling 1/12"));
        assert!(lines[COMPILE_STEPS as usize].starts_with("ready in "));
    }

    #[test]
    fn the_same_seed_gives_the_same_lines() {
        let strip_time = |line: String| line[12..].to_string();
        let (mut a, mut b) = (Rng::new(42), Rng::new(42));
        for _ in 0..50 {
            assert_eq!(strip_time(log_line(&mut a)), strip_time(log_line(&mut b)));
        }
    }

    #[test]
    fn mixes_levels_like_a_real_server() {
        let mut rng = Rng::new(3);
        let lines: Vec<String> = (0..2000).map(|_| log_line(&mut rng)).collect();
        let count = |tag: &str| lines.iter().filter(|line| line.contains(tag)).count();
        assert!(count(" INF ") > 1200);
        assert!(count(" ERR ") > 20 && count(" ERR ") < 120);
        assert!(count(" WRN ") > 60);
        assert!(lines.iter().any(|line| line.len() > 150), "some lines are long enough to wrap");
    }
}
