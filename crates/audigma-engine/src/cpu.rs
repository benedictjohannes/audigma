//! CPU placement and scheduling description for runtime threads.
//!
//! Pure data only: the engine performs no syscalls. The OS-specific side
//! (`sched_setattr`, `sched_setaffinity`, RTKit, sysfs topology reading) lives in `audigma-io`.

use audigma_settings::{CoreSelection, DspThreads, RealtimeMode, RuntimeConfig, SmtSiblings};
use thiserror::Error;

/// Where and how a single runtime thread should run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadPlacement {
    /// CPUs the thread is restricted to. `None` leaves the inherited affinity untouched.
    pub cpus: Option<Vec<usize>>,
    /// Real-time priority (1-99). `None` keeps the thread at `SCHED_OTHER`.
    pub rt_priority: Option<u8>,
    /// Whether a DSP worker may busy-spin before sleeping (only sensible on an exclusive physical core).
    pub spin: bool,
}

impl Default for ThreadPlacement {
    /// Unpinned, normal priority, spinning allowed.
    fn default() -> Self {
        Self {
            cpus: None,
            rt_priority: None,
            spin: true,
        }
    }
}

impl ThreadPlacement {
    /// Unpinned thread at the given real-time priority, spinning allowed.
    pub fn realtime(priority: u8) -> Self {
        Self {
            cpus: None,
            rt_priority: Some(priority),
            spin: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Topology description (filled by the OS-specific reader in `audigma-io`)
// ---------------------------------------------------------------------------

/// One online CPU as reported by the platform reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuInfo {
    pub id: usize,
    /// `acpi_cppc/highest_perf`.
    pub highest_perf: Option<u64>,
    /// `cpufreq/cpuinfo_max_freq` (kHz).
    pub max_freq_khz: Option<u64>,
    /// `cpu_capacity` (ARM).
    pub capacity: Option<u64>,
    /// All hardware threads of this CPU's physical core (including itself), from the full hardware
    /// topology, regardless of the process affinity mask.
    pub core_cpus: Vec<usize>,
}

/// Hardware topology plus the affinity mask the process started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuTopology {
    /// Online CPUs.
    pub cpus: Vec<CpuInfo>,
    /// Affinity mask recorded by the supervisor before anything narrowed it.
    pub allowed: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Resolved placement
// ---------------------------------------------------------------------------

/// Resolved placement for the whole process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// `audigma-loop`.
    pub loop_: ThreadPlacement,
    /// Affinity for housekeeping threads (supervisor, `pw-in`, `pw-out`, ipc, logs, health).
    /// Empty means unrestricted (inherited affinity).
    pub housekeeping: Vec<usize>,
    /// One entry per DSP worker.
    pub dsp: Vec<ThreadPlacement>,
    pub realtime: RealtimeMode,
    /// `[runtime] realtime_priority`; the loop runs at this, `pw-*` at -1, DSP at -2.
    pub realtime_priority: u8,
    /// Human-readable notes the caller should log at warn level.
    pub warnings: Vec<String>,
}

impl Placement {
    /// Placement for `pw-in` / `pw-out`: housekeeping affinity at `p-1`.
    pub fn pipewire(&self) -> ThreadPlacement {
        ThreadPlacement {
            cpus: (!self.housekeeping.is_empty()).then(|| self.housekeeping.clone()),
            rt_priority: self.loop_.rt_priority.map(|p| p.saturating_sub(1).max(1)),
            spin: false,
        }
    }

    /// Strips every real-time request (offline commands never run at RT).
    pub fn without_realtime(mut self) -> Self {
        self.loop_.rt_priority = None;
        for d in &mut self.dsp {
            d.rt_priority = None;
        }
        self.realtime = RealtimeMode::Off;
        self
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlacementError {
    #[error("[runtime] dsp_cpus lists {pins} CPUs but {workers} DSP workers need one each")]
    TooFewPins { pins: usize, workers: usize },
    #[error("[runtime] {key} cpu {cpu} is not online or not allowed for this process")]
    CpuUnavailable { key: &'static str, cpu: usize },
    #[error(
        "[runtime] smt_siblings = \"never\": {workers} DSP workers need distinct physical cores but only {cores} are available"
    )]
    NotEnoughPhysicalCores { workers: usize, cores: usize },
    #[error("[runtime] smt_siblings = \"never\": dsp_cpus {a} and {b} are SMT siblings")]
    PinnedSiblings { a: usize, b: usize },
    #[error("no DSP workers requested")]
    NoWorkers,
}

/// Relative gap between neighbouring core metrics that starts a new class.
const CLASS_GAP: f64 = 0.10;

struct Group {
    rep: usize,
    /// Usable (online and allowed) members, ascending.
    usable: Vec<usize>,
    /// Full hardware membership.
    full: Vec<usize>,
    class: usize,
    value: u64,
}

fn pick_metric(usable: &[&CpuInfo]) -> Option<fn(&CpuInfo) -> Option<u64>> {
    let metrics: [fn(&CpuInfo) -> Option<u64>; 3] =
        [|c| c.highest_perf, |c| c.max_freq_khz, |c| c.capacity];
    metrics
        .into_iter()
        .find(|m| usable.iter().all(|c| m(c).is_some()))
}

/// Groups usable CPUs into physical cores and ranks them into classes.
/// `None` when no metric covers every usable core.
fn build_groups(topo: &CpuTopology) -> (Vec<Group>, bool) {
    let usable: Vec<&CpuInfo> = topo
        .cpus
        .iter()
        .filter(|c| topo.allowed.contains(&c.id))
        .collect();
    let metric = pick_metric(&usable);

    let mut by_key: std::collections::BTreeMap<usize, Vec<&CpuInfo>> = Default::default();
    for c in &usable {
        let key = c.core_cpus.iter().copied().min().unwrap_or(c.id).min(c.id);
        by_key.entry(key).or_default().push(c);
    }
    let mut groups: Vec<Group> = by_key
        .into_values()
        .map(|mut members| {
            members.sort_by_key(|c| c.id);
            let rep = members[0];
            let mut full = rep.core_cpus.clone();
            if !full.contains(&rep.id) {
                full.push(rep.id);
            }
            for m in &members {
                if !full.contains(&m.id) {
                    full.push(m.id);
                }
            }
            full.sort_unstable();
            Group {
                rep: rep.id,
                usable: members.iter().map(|c| c.id).collect(),
                full,
                class: 0,
                value: metric.and_then(|m| m(rep)).unwrap_or(0),
            }
        })
        .collect();

    if metric.is_some() {
        let mut values: Vec<u64> = groups.iter().map(|g| g.value).collect();
        values.sort_unstable_by(|a, b| b.cmp(a));
        values.dedup();
        // class boundaries: value v starts a new class when the gap to its predecessor exceeds CLASS_GAP
        let mut class_of: Vec<(u64, usize)> = Vec::new();
        let mut class = 0usize;
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                let prev = values[i - 1] as f64;
                if prev > 0.0 && (prev - *v as f64) / prev > CLASS_GAP {
                    class += 1;
                }
            }
            class_of.push((*v, class));
        }
        for g in &mut groups {
            g.class = class_of
                .iter()
                .find(|(v, _)| *v == g.value)
                .map_or(0, |(_, c)| *c);
        }
    }
    (groups, metric.is_some())
}

/// Resolves thread placement.
///
/// `topology` is `None` when discovery failed: the result is unpinned (RT priority only) with a
/// warning. Only explicitly invalid pins are errors.
///
/// Worker count: numeric `dsp_threads`, else the length of `dsp_cpus`, else
/// `min(channels, max(1, physical cores - 1))` (one core is left for the loop); always capped
/// at `effective_channels`. Channel `c` runs on worker `c % workers`.
pub fn resolve(
    topology: Option<&CpuTopology>,
    cfg: &RuntimeConfig,
    effective_channels: usize,
) -> Result<Placement, PlacementError> {
    let channels = effective_channels;
    if channels == 0 {
        return Err(PlacementError::NoWorkers);
    }
    let mut warnings = Vec::new();
    let p = cfg.realtime_priority;
    let rt =
        |delta: u8| (cfg.realtime != RealtimeMode::Off).then(|| p.saturating_sub(delta).max(1));

    // Worker count requested explicitly (`None` = `auto` without pins; needs the topology).
    let explicit_workers = match (cfg.dsp_threads, &cfg.dsp_cpus) {
        (DspThreads::Inline, _) => Some(0),
        (DspThreads::Count(n), _) => Some(n as usize),
        (DspThreads::Auto, Some(pins)) => Some(pins.len()),
        (DspThreads::Auto, None) => None,
    };
    let inline = explicit_workers == Some(0);
    if let Some(n) = explicit_workers
        && n > channels
    {
        tracing::debug!(
            requested = n,
            channels,
            "more DSP workers than channels; capped"
        );
    }
    let explicit_workers = explicit_workers.map(|n| if inline { 0 } else { n.clamp(1, channels) });

    let finish = |dsp_cpus: Vec<Option<usize>>,
                  housekeeping: Vec<usize>,
                  loop_cpus: Option<Vec<usize>>,
                  spins: Vec<bool>,
                  warnings: Vec<String>| Placement {
        loop_: ThreadPlacement {
            cpus: loop_cpus,
            rt_priority: rt(0),
            spin: false,
        },
        housekeeping,
        dsp: dsp_cpus
            .into_iter()
            .zip(spins)
            .map(|(cpu, spin)| ThreadPlacement {
                cpus: cpu.map(|c| vec![c]),
                rt_priority: rt(2),
                spin,
            })
            .collect(),
        realtime: cfg.realtime,
        realtime_priority: p,
        warnings,
    };

    let loop_pin = cfg.loop_cpu.map(|c| c as usize);
    if inline {
        // Inline mode: no DSP threads, so no DSP cores and no housekeeping split. The loop thread
        // does the DSP; it floats over the allowed set unless `loop_cpu` pins it.
        if let (Some(l), Some(topo)) = (loop_pin, topology)
            && !topo.allowed.contains(&l)
        {
            return Err(PlacementError::CpuUnavailable {
                key: "loop_cpu",
                cpu: l,
            });
        }
        return Ok(finish(
            Vec::new(),
            Vec::new(),
            loop_pin.map(|c| vec![c]),
            Vec::new(),
            warnings,
        ));
    }
    let Some(topo) = topology else {
        warnings.push("CPU topology unavailable; running unpinned".to_string());
        let workers = explicit_workers.unwrap_or(channels);
        let pins = cfg
            .dsp_cpus
            .as_ref()
            .map(|v| v.iter().map(|c| *c as usize).collect::<Vec<_>>());
        if let Some(pins) = &pins
            && pins.len() < workers
        {
            return Err(PlacementError::TooFewPins {
                pins: pins.len(),
                workers,
            });
        }
        let dsp: Vec<Option<usize>> = (0..workers).map(|i| pins.as_ref().map(|p| p[i])).collect();
        return Ok(finish(
            dsp,
            Vec::new(),
            loop_pin.map(|c| vec![c]),
            vec![false; workers],
            warnings,
        ));
    };

    let (groups, ranked) = build_groups(topo);
    let workers =
        explicit_workers.unwrap_or_else(|| channels.min(groups.len().saturating_sub(1).max(1)));
    let usable: Vec<usize> = groups
        .iter()
        .flat_map(|g| g.usable.iter().copied())
        .collect();
    let is_usable = |c: usize| usable.contains(&c);
    if let Some(l) = loop_pin
        && !is_usable(l)
    {
        return Err(PlacementError::CpuUnavailable {
            key: "loop_cpu",
            cpu: l,
        });
    }
    let group_of = |c: usize| groups.iter().find(|g| g.usable.contains(&c));

    // --- choose DSP cpus -------------------------------------------------
    let assigned: Vec<Option<usize>> = if let Some(pins) = &cfg.dsp_cpus {
        let pins: Vec<usize> = pins.iter().map(|c| *c as usize).collect();
        for &c in &pins {
            if !is_usable(c) {
                return Err(PlacementError::CpuUnavailable {
                    key: "dsp_cpus",
                    cpu: c,
                });
            }
        }
        if pins.len() < workers {
            return Err(PlacementError::TooFewPins {
                pins: pins.len(),
                workers,
            });
        }
        if pins.len() > workers {
            tracing::debug!(
                pins = pins.len(),
                workers,
                "dsp_cpus longer than worker count; using the first entries"
            );
        }
        let pins = &pins[..workers];
        for (i, &a) in pins.iter().enumerate() {
            for &b in &pins[i + 1..] {
                if group_of(a).map(|g| g.rep) == group_of(b).map(|g| g.rep) {
                    if cfg.smt_siblings == SmtSiblings::Never {
                        return Err(PlacementError::PinnedSiblings { a, b });
                    }
                    warnings.push(format!(
                        "dsp_cpus {a} and {b} are SMT siblings and share a physical core"
                    ));
                }
            }
        }
        pins.iter().map(|c| Some(*c)).collect()
    } else if cfg.core_selection == CoreSelection::Os {
        vec![None; workers]
    } else if !ranked {
        warnings.push(
            "core performance metric missing on some CPUs; DSP workers left unpinned".to_string(),
        );
        vec![None; workers]
    } else {
        let mut order: Vec<&Group> = groups.iter().collect();
        order.sort_by(|a, b| {
            let class = match cfg.core_selection {
                CoreSelection::Efficient => b.class.cmp(&a.class),
                _ => a.class.cmp(&b.class),
            };
            class.then(b.value.cmp(&a.value)).then(a.rep.cmp(&b.rep))
        });
        let mut primaries: Vec<usize> = order
            .iter()
            .map(|g| g.rep)
            .filter(|c| Some(*c) != loop_pin)
            .collect();
        let mut siblings: Vec<usize> = Vec::new();
        match cfg.smt_siblings {
            SmtSiblings::Never => {
                if workers > primaries.len() {
                    return Err(PlacementError::NotEnoughPhysicalCores {
                        workers,
                        cores: primaries.len(),
                    });
                }
            }
            SmtSiblings::Avoid => {
                for g in &order {
                    siblings.extend(
                        g.usable
                            .iter()
                            .copied()
                            .filter(|c| *c != g.rep && Some(*c) != loop_pin),
                    );
                }
            }
        }
        if workers > primaries.len() {
            if workers > primaries.len() + siblings.len() {
                warnings.push(format!(
                    "{workers} DSP workers exceed {} usable CPUs; cores are shared",
                    primaries.len() + siblings.len()
                ));
            } else {
                warnings.push(format!(
                    "{workers} DSP workers exceed {} physical cores; using SMT siblings",
                    primaries.len()
                ));
            }
        }
        primaries.extend(siblings);
        if primaries.is_empty() {
            vec![None; workers]
        } else {
            (0..workers)
                .map(|i| Some(primaries[i % primaries.len()]))
                .collect()
        }
    };

    // --- spin rule -------------------------------------------------------
    let mut used: Vec<usize> = assigned.iter().flatten().copied().collect();
    if let Some(l) = loop_pin {
        used.push(l);
    }
    let spins: Vec<bool> = assigned
        .iter()
        .map(|a| {
            let Some(c) = *a else { return false };
            let Some(g) = group_of(c) else { return false };
            used.iter().filter(|u| **u == c).count() == 1
                && g.full
                    .iter()
                    .all(|m| *m == c || (topo.allowed.contains(m) && !used.contains(m)))
        })
        .collect();

    // --- housekeeping ----------------------------------------------------
    let dsp_cpus: Vec<usize> = assigned.iter().flatten().copied().collect();
    let housekeeping = if dsp_cpus.is_empty() {
        Vec::new()
    } else {
        let mut dsp_with_siblings = dsp_cpus.clone();
        for &c in &dsp_cpus {
            if let Some(g) = group_of(c) {
                dsp_with_siblings.extend(g.full.iter().copied());
            }
        }
        let strict: Vec<usize> = usable
            .iter()
            .copied()
            .filter(|c| !dsp_with_siblings.contains(c))
            .collect();
        if !strict.is_empty() {
            strict
        } else {
            usable
                .iter()
                .copied()
                .filter(|c| !dsp_cpus.contains(c))
                .collect()
        }
    };
    let mut housekeeping = housekeeping;
    housekeeping.sort_unstable();

    if let Some(l) = loop_pin
        && let Some(lg) = group_of(l)
        && dsp_cpus.iter().any(|c| lg.usable.contains(c))
    {
        warnings.push(format!("loop_cpu {l} is an SMT sibling of a DSP core"));
    }

    let loop_cpus = match loop_pin {
        Some(l) => Some(vec![l]),
        None => (!housekeeping.is_empty()).then(|| housekeeping.clone()),
    };
    Ok(finish(assigned, housekeeping, loop_cpus, spins, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: usize, perf: Option<u64>, core: &[usize]) -> CpuInfo {
        CpuInfo {
            id,
            highest_perf: perf,
            max_freq_khz: None,
            capacity: None,
            core_cpus: core.to_vec(),
        }
    }

    /// Zen 5 + 5c: 8 cores / 16 threads, siblings N and N+8, even cpus fast.
    fn dev_machine() -> CpuTopology {
        let perf = |n: usize| match n % 8 {
            0 => 208,
            2 => 202,
            4 | 6 => 196,
            _ => 135,
        };
        CpuTopology {
            cpus: (0..16)
                .map(|i| info(i, Some(perf(i)), &[i % 8, i % 8 + 8]))
                .collect(),
            allowed: (0..16).collect(),
        }
    }

    /// `cores` physical cores with `threads` threads each (siblings are `n`, `n + cores`).
    fn homogeneous(cores: usize, threads: usize) -> CpuTopology {
        CpuTopology {
            cpus: (0..cores * threads)
                .map(|i| {
                    let core: Vec<usize> = (0..threads).map(|t| i % cores + t * cores).collect();
                    info(i, Some(100), &core)
                })
                .collect(),
            allowed: (0..cores * threads).collect(),
        }
    }

    fn two_class() -> CpuTopology {
        // 2 big (perf 100) + 4 little (perf 50), no SMT
        CpuTopology {
            cpus: (0..6)
                .map(|i| info(i, Some(if i < 2 { 100 } else { 50 }), &[i]))
                .collect(),
            allowed: (0..6).collect(),
        }
    }

    fn cfg() -> RuntimeConfig {
        RuntimeConfig::default()
    }

    /// `cfg()` with an explicit worker count.
    fn threads(n: u32) -> RuntimeConfig {
        RuntimeConfig {
            dsp_threads: DspThreads::Count(n),
            ..cfg()
        }
    }

    fn pins(p: &Placement) -> Vec<Option<usize>> {
        p.dsp
            .iter()
            .map(|d| d.cpus.as_ref().map(|c| c[0]))
            .collect()
    }

    #[test]
    fn dev_machine_stereo_uses_fast_physical_cores() {
        let p = resolve(Some(&dev_machine()), &cfg(), 2).unwrap();
        assert_eq!(pins(&p), vec![Some(0), Some(2)]);
        assert!(p.dsp.iter().all(|d| d.spin && d.rt_priority == Some(18)));
        assert_eq!(p.loop_.rt_priority, Some(20));
        // housekeeping excludes DSP cores and their siblings
        assert_eq!(
            p.housekeeping,
            vec![1, 3, 4, 5, 6, 7, 9, 11, 12, 13, 14, 15]
        );
        assert_eq!(p.loop_.cpus.as_deref(), Some(&p.housekeeping[..]));
        assert_eq!(p.pipewire().rt_priority, Some(19));
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn dev_machine_eight_channels_spills_fast_then_slow_then_smt() {
        let p = resolve(Some(&dev_machine()), &threads(5), 5).unwrap();
        assert_eq!(pins(&p), [0, 2, 4, 6, 1].map(Some));
        let p = resolve(Some(&dev_machine()), &threads(10), 10).unwrap();
        // 8 physical (fast 0,2,4,6 then slow 1,3,5,7), then fast SMT 8,10
        assert_eq!(pins(&p), [0, 2, 4, 6, 1, 3, 5, 7, 8, 10].map(Some));
        assert_eq!(p.warnings.len(), 1);
        // 8 sits next to busy worker 0: nobody spins
        assert!(!p.dsp[0].spin && !p.dsp[8].spin);
        assert!(p.dsp[4].spin);
        // housekeeping empty strict set -> fallback to allowed - dsp
        assert_eq!(p.housekeeping, vec![9, 11, 12, 13, 14, 15]);
    }

    #[test]
    fn efficient_prefers_slow_cores() {
        let c = RuntimeConfig {
            core_selection: CoreSelection::Efficient,
            ..cfg()
        };
        let p = resolve(Some(&dev_machine()), &c, 2).unwrap();
        assert_eq!(pins(&p), vec![Some(1), Some(3)]);
        let p = resolve(Some(&two_class()), &c, 3).unwrap();
        assert_eq!(pins(&p), [2, 3, 4].map(Some));
    }

    #[test]
    fn performance_on_two_class_without_smt() {
        let p = resolve(Some(&two_class()), &threads(3), 3).unwrap();
        assert_eq!(pins(&p), [0, 1, 2].map(Some));
        assert_eq!(p.housekeeping, vec![3, 4, 5]);
        assert!(p.dsp.iter().all(|d| d.spin));
    }

    #[test]
    fn homogeneous_four_core_smt() {
        let t = homogeneous(4, 2);
        let p = resolve(Some(&t), &cfg(), 2).unwrap();
        assert_eq!(pins(&p), vec![Some(0), Some(1)]);
        assert_eq!(p.housekeeping, vec![2, 3, 6, 7]);
        // 4 workers take all physical cores; housekeeping falls back to the siblings
        let p = resolve(Some(&t), &threads(4), 4).unwrap();
        assert_eq!(pins(&p), [0, 1, 2, 3].map(Some));
        assert_eq!(p.housekeeping, vec![4, 5, 6, 7]);
        assert!(p.dsp.iter().all(|d| d.spin));
    }

    #[test]
    fn smt_never_fails_when_physical_cores_run_out() {
        let c = RuntimeConfig {
            smt_siblings: SmtSiblings::Never,
            ..threads(4)
        };
        let t = homogeneous(4, 2);
        assert!(resolve(Some(&t), &c, 4).is_ok());
        let c = RuntimeConfig {
            dsp_threads: DspThreads::Count(5),
            ..c
        };
        assert_eq!(
            resolve(Some(&t), &c, 5),
            Err(PlacementError::NotEnoughPhysicalCores {
                workers: 5,
                cores: 4
            })
        );
    }

    #[test]
    fn two_cores_two_threads_leaves_housekeeping_unrestricted() {
        let t = homogeneous(2, 1);
        let p = resolve(Some(&t), &threads(2), 2).unwrap();
        assert_eq!(pins(&p), vec![Some(0), Some(1)]);
        assert!(p.housekeeping.is_empty());
        assert_eq!(p.loop_.cpus, None);
        assert_eq!(p.pipewire().cpus, None);
    }

    #[test]
    fn restricted_allowed_mask_limits_candidates_and_spin() {
        let mut t = dev_machine();
        t.allowed = vec![1, 3, 9, 11];
        let p = resolve(Some(&t), &threads(2), 2).unwrap();
        assert_eq!(pins(&p), vec![Some(1), Some(3)]);
        // siblings 9, 11 are allowed and unused by audigma -> exclusive, but housekeeping falls back
        assert_eq!(p.housekeeping, vec![9, 11]);
        // sibling outside the mask: shared with other processes, no spin
        t.allowed = vec![1, 3, 5];
        let p = resolve(Some(&t), &threads(2), 2).unwrap();
        assert_eq!(pins(&p), vec![Some(1), Some(3)]);
        assert!(p.dsp.iter().all(|d| !d.spin));
        assert_eq!(p.housekeeping, vec![5]);
    }

    #[test]
    fn os_mode_is_unpinned_without_spin_but_explicit_pins_override() {
        let c = RuntimeConfig {
            core_selection: CoreSelection::Os,
            ..cfg()
        };
        let p = resolve(Some(&dev_machine()), &c, 2).unwrap();
        assert_eq!(pins(&p), vec![None, None]);
        assert!(p.dsp.iter().all(|d| !d.spin && d.rt_priority == Some(18)));
        assert!(p.housekeeping.is_empty());
        let c = RuntimeConfig {
            dsp_cpus: Some(vec![4, 6]),
            ..c
        };
        let p = resolve(Some(&dev_machine()), &c, 2).unwrap();
        assert_eq!(pins(&p), vec![Some(4), Some(6)]);
        assert!(p.dsp.iter().all(|d| d.spin));
    }

    #[test]
    fn explicit_pins_truncate_and_reject_too_few() {
        let c = RuntimeConfig {
            dsp_cpus: Some(vec![2, 4, 6]),
            ..cfg()
        };
        let p = resolve(Some(&dev_machine()), &c, 2).unwrap();
        assert_eq!(pins(&p), vec![Some(2), Some(4)]);
        let c = RuntimeConfig {
            dsp_cpus: Some(vec![2]),
            ..threads(2)
        };
        assert_eq!(
            resolve(Some(&dev_machine()), &c, 2),
            Err(PlacementError::TooFewPins {
                pins: 1,
                workers: 2
            })
        );
        assert!(resolve(Some(&dev_machine()), &c, 1).is_ok());
        // `auto` + one pin means one worker for all channels, not an error
        let c = RuntimeConfig {
            dsp_cpus: Some(vec![2]),
            ..cfg()
        };
        let p = resolve(Some(&dev_machine()), &c, 2).unwrap();
        assert_eq!(pins(&p), vec![Some(2)]);
    }

    #[test]
    fn explicit_pins_must_be_available_and_siblings_policy() {
        let mut t = dev_machine();
        let c = RuntimeConfig {
            dsp_cpus: Some(vec![0, 99]),
            ..cfg()
        };
        assert_eq!(
            resolve(Some(&t), &c, 2),
            Err(PlacementError::CpuUnavailable {
                key: "dsp_cpus",
                cpu: 99
            })
        );
        t.allowed = vec![0, 2];
        let c = RuntimeConfig {
            dsp_cpus: Some(vec![0, 4]),
            ..cfg()
        };
        assert!(matches!(
            resolve(Some(&t), &c, 2),
            Err(PlacementError::CpuUnavailable { cpu: 4, .. })
        ));
        let sib = RuntimeConfig {
            dsp_cpus: Some(vec![0, 8]),
            ..cfg()
        };
        let p = resolve(Some(&dev_machine()), &sib, 2).unwrap();
        assert_eq!(p.warnings.len(), 1);
        assert!(p.dsp.iter().all(|d| !d.spin));
        let never = RuntimeConfig {
            smt_siblings: SmtSiblings::Never,
            ..sib
        };
        assert_eq!(
            resolve(Some(&dev_machine()), &never, 2),
            Err(PlacementError::PinnedSiblings { a: 0, b: 8 })
        );
    }

    #[test]
    fn loop_cpu_is_excluded_from_dsp_and_validated() {
        let c = RuntimeConfig {
            loop_cpu: Some(0),
            ..cfg()
        };
        let p = resolve(Some(&dev_machine()), &c, 2).unwrap();
        assert_eq!(pins(&p), vec![Some(2), Some(4)]);
        assert_eq!(p.loop_.cpus, Some(vec![0]));
        let c = RuntimeConfig {
            loop_cpu: Some(40),
            ..cfg()
        };
        assert!(matches!(
            resolve(Some(&dev_machine()), &c, 2),
            Err(PlacementError::CpuUnavailable {
                key: "loop_cpu",
                ..
            })
        ));
        let c = RuntimeConfig {
            loop_cpu: Some(8),
            ..cfg()
        };
        let p = resolve(Some(&dev_machine()), &c, 2).unwrap();
        assert!(p.warnings.iter().any(|w| w.contains("SMT sibling")));
        assert!(!p.dsp[0].spin);
    }

    #[test]
    fn missing_metric_or_topology_means_unpinned() {
        let mut t = dev_machine();
        t.cpus[3].highest_perf = None;
        let p = resolve(Some(&t), &cfg(), 2).unwrap();
        assert_eq!(pins(&p), vec![None, None]);
        assert!(p.dsp.iter().all(|d| !d.spin));
        assert_eq!(p.warnings.len(), 1);
        // fallback to cpuinfo_max_freq when it covers every core
        for c in &mut t.cpus {
            c.max_freq_khz = Some(if c.id % 2 == 0 { 5_000_000 } else { 3_500_000 });
        }
        let p = resolve(Some(&t), &cfg(), 2).unwrap();
        assert_eq!(pins(&p), vec![Some(0), Some(2)]);
        let p = resolve(None, &cfg(), 2).unwrap();
        assert_eq!(pins(&p), vec![None, None]);
        assert_eq!(p.dsp[0].rt_priority, Some(18));
        assert_eq!(p.warnings.len(), 1);
    }

    #[test]
    fn realtime_off_and_offline_strip_priorities() {
        let c = RuntimeConfig {
            realtime: RealtimeMode::Off,
            ..cfg()
        };
        let p = resolve(Some(&dev_machine()), &c, 2).unwrap();
        assert_eq!(p.loop_.rt_priority, None);
        assert!(p.dsp.iter().all(|d| d.rt_priority.is_none()));
        assert_eq!(p.pipewire().rt_priority, None);
        let p = resolve(Some(&dev_machine()), &cfg(), 2)
            .unwrap()
            .without_realtime();
        assert_eq!(p.loop_.rt_priority, None);
        assert_eq!(p.realtime, RealtimeMode::Off);
        assert_eq!(pins(&p), vec![Some(0), Some(2)]);
    }

    #[test]
    fn five_and_eight_channels_on_small_machine_share_cores() {
        let t = homogeneous(2, 1);
        let p = resolve(Some(&t), &threads(5), 5).unwrap();
        assert_eq!(p.dsp.len(), 5);
        assert!(p.dsp.iter().all(|d| !d.spin));
        assert!(p.warnings.iter().any(|w| w.contains("shared")));
        let p = resolve(Some(&dev_machine()), &threads(8), 8).unwrap();
        assert_eq!(pins(&p), [0, 2, 4, 6, 1, 3, 5, 7].map(Some));
        assert!(p.dsp.iter().all(|d| d.spin));
    }

    #[test]
    fn auto_worker_count_is_channels_capped_by_physical_cores_minus_one() {
        let count = |t: &CpuTopology, ch| resolve(Some(t), &cfg(), ch).unwrap().dsp.len();
        assert_eq!(count(&dev_machine(), 2), 2);
        assert_eq!(count(&dev_machine(), 8), 7); // 8 physical cores
        assert_eq!(count(&dev_machine(), 10), 7);
        assert_eq!(count(&homogeneous(4, 2), 2), 2);
        assert_eq!(count(&homogeneous(2, 2), 2), 1);
        assert_eq!(count(&homogeneous(2, 1), 2), 1);
        assert_eq!(count(&homogeneous(1, 1), 2), 1);
        // unknown topology: one worker per channel
        assert_eq!(resolve(None, &cfg(), 2).unwrap().dsp.len(), 2);
    }

    #[test]
    fn inline_has_no_workers_and_no_housekeeping_split() {
        let inline = RuntimeConfig {
            dsp_threads: DspThreads::Inline,
            ..cfg()
        };
        for topo in [Some(dev_machine()), None] {
            let p = resolve(topo.as_ref(), &inline, 2).unwrap();
            assert!(p.dsp.is_empty());
            assert!(p.housekeeping.is_empty());
            assert_eq!(p.loop_.cpus, None);
            assert_eq!(p.loop_.rt_priority, Some(p.realtime_priority));
        }
        let pinned = RuntimeConfig {
            loop_cpu: Some(2),
            ..inline.clone()
        };
        let p = resolve(Some(&dev_machine()), &pinned, 2).unwrap();
        assert_eq!(p.loop_.cpus, Some(vec![2]));
        let bad = RuntimeConfig {
            loop_cpu: Some(99),
            ..inline
        };
        assert!(matches!(
            resolve(Some(&dev_machine()), &bad, 2),
            Err(PlacementError::CpuUnavailable { .. })
        ));
    }

    #[test]
    fn numeric_dsp_threads_is_honoured_and_capped_at_channels() {
        let p = resolve(Some(&dev_machine()), &threads(1), 2).unwrap();
        assert_eq!(pins(&p), vec![Some(0)]);
        assert!(p.dsp[0].spin);
        let p = resolve(Some(&dev_machine()), &threads(4), 2).unwrap();
        assert_eq!(p.dsp.len(), 2);
        let p = resolve(Some(&homogeneous(2, 1)), &threads(2), 2).unwrap();
        assert_eq!(p.dsp.len(), 2);
    }
}
