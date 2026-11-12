//! Benchmark jobs: one worker thread per run, an append-only event log, and the readers
//! (Server-Sent Events, polling) that follow it.
//!
//! A job is either *validate* (one benchmark of the draft's own settings) or *discover* (the
//! guided sweep over rate tiers, owned by the server so the frontend only renders events). The
//! log keeps every event, so a reader that reconnects replays from the start.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use audigma_settings::Settings;
use schemars::JsonSchema;
use serde::Serialize;

use super::Backend;
use super::session::{DiscoveryView, Session, Validated};
use crate::assist::benchmark::{BenchmarkEvent, BenchmarkResult, Stage};
use crate::guided::DiscoveryNext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    Validate,
    Discover,
}

/// One entry of a job's log (the payload of a Server-Sent Event).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    Progress {
        stage: Stage,
        fraction: f64,
        /// The tier being measured (discovery only).
        tier: Option<String>,
    },
    /// The state of the sweep after a tier ended (discovery only).
    Discovery {
        discovery: DiscoveryView,
    },
    /// Last event. `result` is set for validation.
    Finished {
        result: Option<Box<BenchmarkResult>>,
    },
    Failed {
        message: String,
    },
    Cancelled,
}

impl JobEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Progress { .. } => "progress",
            Self::Discovery { .. } => "discovery",
            Self::Finished { .. } => "finished",
            Self::Failed { .. } => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished { .. } | Self::Failed { .. } | Self::Cancelled
        )
    }
}

pub enum Wait {
    Event(JobEvent),
    /// Nothing new within the timeout.
    Idle,
    /// The log is complete and fully read.
    Done,
}

#[derive(Default)]
struct Log {
    events: Vec<JobEvent>,
    done: bool,
}

pub struct Job {
    pub id: u64,
    pub kind: JobKind,
    log: Mutex<Log>,
    cv: Condvar,
    cancel: AtomicBool,
}

/// `GET /api/benchmark/{id}`: where the job stands, for clients that poll.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct JobStatus {
    pub id: u64,
    pub kind: JobKind,
    pub done: bool,
    pub last: Option<JobEvent>,
}

impl Job {
    pub fn new(id: u64, kind: JobKind) -> Arc<Self> {
        Arc::new(Self {
            id,
            kind,
            log: Mutex::default(),
            cv: Condvar::new(),
            cancel: AtomicBool::new(false),
        })
    }

    fn push(&self, ev: JobEvent) {
        let mut log = self.log.lock().expect("job log");
        if log.done {
            return;
        }
        log.done = ev.terminal();
        log.events.push(ev);
        self.cv.notify_all();
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_done(&self) -> bool {
        self.log.lock().expect("job log").done
    }

    pub fn status(&self) -> JobStatus {
        let log = self.log.lock().expect("job log");
        JobStatus {
            id: self.id,
            kind: self.kind,
            done: log.done,
            last: log.events.last().cloned(),
        }
    }

    /// The event at `index`, waiting up to `timeout` for it.
    pub fn wait(&self, index: usize, timeout: Duration) -> Wait {
        let log = self.log.lock().expect("job log");
        let (log, _) = self
            .cv
            .wait_timeout_while(log, timeout, |l| l.events.len() <= index && !l.done)
            .expect("job log");
        match log.events.get(index) {
            Some(ev) => Wait::Event(ev.clone()),
            None if log.done => Wait::Done,
            None => Wait::Idle,
        }
    }
}

/// Runs `first` and, for a discovery job, the tiers that follow it. Returns when the log holds
/// its terminal event.
pub fn spawn(
    job: Arc<Job>,
    session: Arc<Mutex<Session>>,
    backend: Arc<dyn Backend>,
    first: Settings,
) {
    let name = format!("audigma-job-{}", job.id);
    let (job_t, session_t) = (Arc::clone(&job), Arc::clone(&session));
    let spawned = std::thread::Builder::new()
        .name(name)
        .spawn(move || pump(&job_t, &session_t, backend.as_ref(), first));
    if let Err(e) = spawned {
        // No thread will ever publish a terminal event: do it here, which also clears `busy`.
        finish(
            &job,
            &session,
            JobEvent::Failed {
                message: format!("could not start the benchmark thread: {e}"),
            },
        );
    }
}

fn pump(job: &Job, session: &Mutex<Session>, backend: &dyn Backend, first: Settings) {
    let mut handle = backend.start_benchmark(first);
    let mut tier: Option<String> = None;
    if job.kind == JobKind::Discover {
        tier = running_tier(session);
    }
    loop {
        if job.cancel.load(Ordering::Relaxed) {
            handle.cancel();
        }
        let ev = match handle.events.recv_timeout(Duration::from_millis(100)) {
            Ok(ev) => ev,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                finish(
                    job,
                    session,
                    JobEvent::Failed {
                        message: "the benchmark thread ended without a result".into(),
                    },
                );
                return;
            }
        };
        match job.kind {
            JobKind::Validate => match ev {
                BenchmarkEvent::Progress { stage, fraction } => job.push(JobEvent::Progress {
                    stage,
                    fraction,
                    tier: None,
                }),
                BenchmarkEvent::Finished(r) => {
                    {
                        let mut s = session.lock().expect("session");
                        s.validated = Some(Validated {
                            draft: s.draft.clone(),
                            result: (*r).clone(),
                        });
                    }
                    finish(job, session, JobEvent::Finished { result: Some(r) });
                    return;
                }
                BenchmarkEvent::Failed(message) => {
                    finish(job, session, JobEvent::Failed { message });
                    return;
                }
                BenchmarkEvent::Cancelled => {
                    finish(job, session, JobEvent::Cancelled);
                    return;
                }
            },
            JobKind::Discover => {
                if let BenchmarkEvent::Progress { stage, fraction } = &ev {
                    job.push(JobEvent::Progress {
                        stage: *stage,
                        fraction: *fraction,
                        tier: tier.clone(),
                    });
                }
                let (next, view, cancelled) = {
                    let mut s = session.lock().expect("session");
                    let next = s.on_discovery(&ev);
                    (
                        next,
                        s.discovery_view(),
                        matches!(ev, BenchmarkEvent::Cancelled),
                    )
                };
                match next {
                    DiscoveryNext::Nothing => {}
                    DiscoveryNext::StartBenchmark(s) => {
                        job.push(JobEvent::Discovery { discovery: view });
                        handle = backend.start_benchmark(*s);
                        tier = running_tier(session);
                    }
                    DiscoveryNext::Finished => {
                        job.push(JobEvent::Discovery { discovery: view });
                        let end = if cancelled {
                            JobEvent::Cancelled
                        } else {
                            JobEvent::Finished { result: None }
                        };
                        finish(job, session, end);
                        return;
                    }
                }
            }
        }
    }
}

fn running_tier(session: &Mutex<Session>) -> Option<String> {
    let s = session.lock().expect("session");
    s.guided.as_ref().and_then(|g| {
        g.discovery
            .tiers
            .iter()
            .find(|(_, st)| *st == crate::guided::TierState::Running)
            .map(|(k, _)| k.clone())
    })
}

/// Clears the busy flag, then publishes the terminal event (so a client that sees it can edit
/// at once).
fn finish(job: &Job, session: &Mutex<Session>, ev: JobEvent) {
    session.lock().expect("session").busy = false;
    job.push(ev);
}
