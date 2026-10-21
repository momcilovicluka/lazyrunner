//! CPU and memory of processes, sampled with sysinfo.

use std::collections::HashMap;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// CPU and memory of whole process trees (dotnet run and its API, npm and node): what an app costs.
pub struct TreeSampler {
    system: System,
    cores: f32,
}

impl Default for TreeSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeSampler {
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
        Self { system: System::new(), cores }
    }

    /// Per (name, root pid): CPU as a share of the machine since the last call (0 on the first), and memory in MB.
    pub fn sample(&mut self, roots: &[(String, u32)]) -> Vec<(String, f32, f32)> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
        for (pid, process) in self.system.processes() {
            if let Some(parent) = process.parent() {
                children.entry(parent).or_default().push(*pid);
            }
        }
        roots
            .iter()
            .map(|(name, root)| {
                let (mut cpu, mut memory) = (0.0, 0u64);
                let mut pending = vec![Pid::from_u32(*root)];
                let mut seen = std::collections::HashSet::new();
                while let Some(pid) = pending.pop() {
                    if !seen.insert(pid) {
                        continue;
                    }
                    if let Some(process) = self.system.process(pid) {
                        cpu += process.cpu_usage();
                        memory += process.memory();
                    }
                    pending.extend(children.get(&pid).into_iter().flatten().copied());
                }
                (name.clone(), (cpu / self.cores).clamp(0.0, 100.0), memory as f32 / 1_048_576.0)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_sums_its_processes() {
        let mut sampler = TreeSampler::new();
        let samples = sampler.sample(&[("me".into(), std::process::id())]);
        assert_eq!(samples[0].0, "me");
        assert!(samples[0].2 > 0.0, "some memory");
    }
}
