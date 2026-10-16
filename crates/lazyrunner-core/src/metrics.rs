//! CPU and memory of processes, sampled with sysinfo.

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Percent of the whole machine (all cores), 0-100.
    pub cpu: f32,
    pub memory_bytes: u64,
}

pub struct Sampler {
    system: System,
    cores: f32,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
        Self { system: System::new(), cores }
    }

    /// Samples each pid; CPU is measured since the previous call, so the first sample of a pid reads 0.
    pub fn sample(&mut self, pids: &[u32]) -> Vec<Option<Sample>> {
        let wanted: Vec<Pid> = pids.iter().map(|&pid| Pid::from_u32(pid)).collect();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&wanted),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        wanted
            .iter()
            .map(|pid| {
                self.system.process(*pid).map(|process| Sample {
                    cpu: (process.cpu_usage() / self.cores).clamp(0.0, 100.0),
                    memory_bytes: process.memory(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_this_process_and_skips_pids_that_are_gone() {
        let mut sampler = Sampler::new();
        let samples = sampler.sample(&[std::process::id(), u32::MAX - 7]);
        let own = samples[0].expect("this process exists");
        assert!(own.memory_bytes > 0);
        assert!((0.0..=100.0).contains(&own.cpu));
        assert_eq!(samples[1], None);
    }
}
