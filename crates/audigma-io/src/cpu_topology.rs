//! CPU topology discovery (Linux sysfs + affinity mask).
//!
//! Produces the plain [`CpuTopology`] description that `audigma_engine::resolve_placement`
//! consumes. Discovery is best-effort: any failure yields an error and the caller runs unpinned.
//! On other platforms [`read_cpu_topology`] always reports "unsupported".

use audigma_engine::{CpuInfo, CpuTopology};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TopologyError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("malformed cpu list {0:?}")]
    BadList(String),
    #[error("CPU topology discovery is not supported on this platform")]
    Unsupported,
    #[error("sched_getaffinity failed: {0}")]
    Affinity(std::io::Error),
}

/// Parses a kernel cpu list such as `0-3,8,10-11`.
pub fn parse_cpu_list(s: &str) -> Result<Vec<usize>, TopologyError> {
    let bad = || TopologyError::BadList(s.to_string());
    let mut out = Vec::new();
    for part in s.trim().split(',').filter(|p| !p.trim().is_empty()) {
        match part.trim().split_once('-') {
            Some((a, b)) => {
                let (a, b): (usize, usize) = (
                    a.trim().parse().map_err(|_| bad())?,
                    b.trim().parse().map_err(|_| bad())?,
                );
                if a > b {
                    return Err(bad());
                }
                out.extend(a..=b);
            }
            None => out.push(part.trim().parse().map_err(|_| bad())?),
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Reads the topology from the real system.
#[cfg(target_os = "linux")]
pub fn read_cpu_topology() -> Result<CpuTopology, TopologyError> {
    read_cpu_topology_from(
        std::path::Path::new("/sys/devices/system/cpu"),
        current_affinity()?,
    )
}

#[cfg(not(target_os = "linux"))]
pub fn read_cpu_topology() -> Result<CpuTopology, TopologyError> {
    Err(TopologyError::Unsupported)
}

/// The calling thread's current affinity mask (call on the supervisor before narrowing it).
#[cfg(target_os = "linux")]
pub fn current_affinity() -> Result<Vec<usize>, TopologyError> {
    unsafe {
        let mut set: libc::cpu_set_t = core::mem::zeroed();
        if libc::sched_getaffinity(0, core::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return Err(TopologyError::Affinity(std::io::Error::last_os_error()));
        }
        Ok((0..libc::CPU_SETSIZE as usize)
            .filter(|c| libc::CPU_ISSET(*c, &set))
            .collect())
    }
}

#[cfg(not(target_os = "linux"))]
pub fn current_affinity() -> Result<Vec<usize>, TopologyError> {
    Err(TopologyError::Unsupported)
}

fn read_trimmed(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_u64(path: &std::path::Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

/// Reads the topology from a sysfs-shaped tree rooted at `root` (`/sys/devices/system/cpu`).
pub fn read_cpu_topology_from(
    root: &std::path::Path,
    allowed: Vec<usize>,
) -> Result<CpuTopology, TopologyError> {
    let online_path = root.join("online");
    let online_text =
        std::fs::read_to_string(&online_path).map_err(|source| TopologyError::Io {
            path: online_path.display().to_string(),
            source,
        })?;
    let online = parse_cpu_list(&online_text)?;

    let cpus = online
        .into_iter()
        .map(|id| {
            let dir = root.join(format!("cpu{id}"));
            let topo = dir.join("topology");
            let core_cpus = ["core_cpus_list", "thread_siblings_list"]
                .iter()
                .find_map(|f| read_trimmed(&topo.join(f)))
                .and_then(|s| parse_cpu_list(&s).ok())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| vec![id]);
            CpuInfo {
                id,
                highest_perf: read_u64(&dir.join("acpi_cppc/highest_perf")),
                max_freq_khz: read_u64(&dir.join("cpufreq/cpuinfo_max_freq")),
                capacity: read_u64(&dir.join("cpu_capacity")),
                core_cpus,
            }
        })
        .collect();
    Ok(CpuTopology { cpus, allowed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use audigma_engine::resolve_placement;
    use audigma_settings::RuntimeConfig;
    use std::fs;
    use std::path::Path;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn parses_cpu_lists() {
        assert_eq!(
            parse_cpu_list("0-3,8,10-11\n").unwrap(),
            vec![0, 1, 2, 3, 8, 10, 11]
        );
        assert_eq!(parse_cpu_list("").unwrap(), Vec::<usize>::new());
        assert!(parse_cpu_list("3-1").is_err());
        assert!(parse_cpu_list("a").is_err());
    }

    #[test]
    fn reads_fake_dev_machine_and_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "online", "0-15\n");
        for i in 0..16usize {
            let n = i % 8;
            let perf = match n {
                0 => 208,
                2 => 202,
                4 | 6 => 196,
                _ => 135,
            };
            write(
                root,
                &format!("cpu{i}/acpi_cppc/highest_perf"),
                &format!("{perf}\n"),
            );
            write(
                root,
                &format!("cpu{i}/topology/core_cpus_list"),
                &format!("{n},{}\n", n + 8),
            );
        }
        let topo = read_cpu_topology_from(root, (0..16).collect()).unwrap();
        assert_eq!(topo.cpus.len(), 16);
        assert_eq!(topo.cpus[9].core_cpus, vec![1, 9]);
        assert_eq!(topo.cpus[0].highest_perf, Some(208));
        assert_eq!(topo.cpus[0].max_freq_khz, None);
        let p = resolve_placement(Some(&topo), &RuntimeConfig::default(), 2).unwrap();
        assert_eq!(p.dsp[0].cpus, Some(vec![0]));
        assert_eq!(p.dsp[1].cpus, Some(vec![2]));
    }

    #[test]
    fn falls_back_to_thread_siblings_and_freq_metric() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "online", "0-1");
        for i in 0..2usize {
            write(root, &format!("cpu{i}/cpufreq/cpuinfo_max_freq"), "4000000");
            write(
                root,
                &format!("cpu{i}/topology/thread_siblings_list"),
                &format!("{i}"),
            );
        }
        let topo = read_cpu_topology_from(root, vec![0, 1]).unwrap();
        assert_eq!(topo.cpus[1].core_cpus, vec![1]);
        assert_eq!(topo.cpus[1].max_freq_khz, Some(4_000_000));
        assert_eq!(topo.cpus[1].highest_perf, None);
    }

    #[test]
    fn missing_root_is_an_error_and_missing_topology_defaults() {
        assert!(read_cpu_topology_from(Path::new("/nonexistent-audigma"), vec![0]).is_err());
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "online", "0");
        let topo = read_cpu_topology_from(dir.path(), vec![0]).unwrap();
        assert_eq!(topo.cpus[0].core_cpus, vec![0]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn real_system_affinity_is_non_empty() {
        assert!(!current_affinity().unwrap().is_empty());
        // The live sysfs read must not panic; it may legitimately fail in containers.
        let _ = read_cpu_topology();
    }
}
