//! Web frontend of `audigma configure`: a one-shot localhost server with a JSON API.
//!
//! The server owns one [`Session`] (draft, guided flow, probe, benchmark) and answers
//! [`api::handle`]. It binds `127.0.0.1` on a random port by default, requires a random token on
//! every request and exits after a save, an explicit quit or an idle timeout. There is no `Host`
//! allowlist and no CORS (see the plan, §7): the token is the access control, and a request with
//! an `Origin` must come from the page's own origin.
//!
//! Tests build a [`Server`] with a fake [`Backend`] and talk to it over a socket, or call
//! [`api::handle`] directly.

pub mod api;
mod http;
pub mod jobs;
pub mod session;

use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use audigma_io::{AlsaProbeReport, PipeWireProbeReport};
use audigma_settings::Settings;
use serde_json::Value;

pub use http::{Body, Request, Response};
pub use session::{SaveResponse, Session};

use crate::assist::benchmark::{self, BenchmarkHandle, BenchmarkRequest};
use jobs::Job;

/// The side effects the server needs: probing the machine and running benchmarks. The real
/// implementation is [`SystemBackend`]; tests substitute fakes.
pub trait Backend: Send + Sync + 'static {
    fn probe_alsa(&self) -> Result<AlsaProbeReport, String>;
    fn probe_pipewire(&self) -> Result<PipeWireProbeReport, String>;
    fn start_benchmark(&self, settings: Settings) -> BenchmarkHandle;
}

/// Probes the real machine and benchmarks at full size.
pub struct SystemBackend;

impl Backend for SystemBackend {
    fn probe_alsa(&self) -> Result<AlsaProbeReport, String> {
        crate::assist::probe::probe_alsa()
    }
    fn probe_pipewire(&self) -> Result<PipeWireProbeReport, String> {
        crate::assist::probe::probe_pipewire()
    }
    fn start_benchmark(&self, settings: Settings) -> BenchmarkHandle {
        benchmark::start(BenchmarkRequest::new(settings))
    }
}

/// Why the server stopped.
#[derive(Debug, Clone)]
pub enum EndReason {
    /// The configuration was written.
    Saved(SaveResponse),
    Quit,
    Idle,
}

pub struct ServeOptions {
    pub bind: SocketAddr,
    /// Stop after this long without a request or a running benchmark.
    pub idle_timeout: Option<Duration>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            idle_timeout: Some(Duration::from_secs(30 * 60)),
        }
    }
}

/// State shared by the connection threads.
pub struct Shared {
    session: Arc<Mutex<Session>>,
    pub(crate) backend: Arc<dyn Backend>,
    pub(crate) token: String,
    pub(crate) cookie_name: String,
    pub(crate) meta: Value,
    pub(crate) next_job: AtomicU64,
    jobs: Mutex<Vec<Arc<Job>>>,
    end: Mutex<Option<EndReason>>,
    shutdown: AtomicBool,
    last_activity: Mutex<Instant>,
}

impl Shared {
    pub(crate) fn session(&self) -> MutexGuard<'_, Session> {
        self.session.lock().expect("session")
    }

    pub(crate) fn touch(&self) {
        *self.last_activity.lock().expect("activity") = Instant::now();
    }

    pub(crate) fn add_job(&self, job: Arc<Job>) {
        let mut jobs = self.jobs.lock().expect("jobs");
        jobs.push(job);
        let excess = jobs.len().saturating_sub(8);
        jobs.drain(..excess);
    }

    pub(crate) fn job(&self, id: u64) -> Option<Arc<Job>> {
        self.jobs
            .lock()
            .expect("jobs")
            .iter()
            .find(|j| j.id == id)
            .cloned()
    }

    /// Records why the server stops (the first reason wins).
    pub(crate) fn finish(&self, reason: EndReason) {
        self.end.lock().expect("end").get_or_insert(reason);
    }
}

/// A running server.
pub struct Server {
    pub addr: SocketAddr,
    pub token: String,
    shared: Arc<Shared>,
    accept: std::thread::JoinHandle<()>,
}

/// 24 random bytes from the kernel, hex encoded.
fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

const MAX_CONNECTIONS: usize = 64;

impl Server {
    pub fn start(
        session: Session,
        backend: Arc<dyn Backend>,
        opts: &ServeOptions,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(opts.bind)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let token = random_token()?;
        let meta = crate::schema::meta_json();
        let shared = Arc::new(Shared {
            session: Arc::new(Mutex::new(session)),
            backend,
            token: token.clone(),
            cookie_name: format!("audigma_token_{}", addr.port()),
            meta,
            next_job: AtomicU64::new(1),
            jobs: Mutex::new(Vec::new()),
            end: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            last_activity: Mutex::new(Instant::now()),
        });
        let sh = Arc::clone(&shared);
        let idle = opts.idle_timeout;
        let accept = std::thread::Builder::new()
            .name("audigma-web".into())
            .spawn(move || accept_loop(&listener, &sh, idle))?;
        Ok(Self {
            addr,
            token,
            shared,
            accept,
        })
    }

    /// The URL to open: the token is traded for a cookie on the first visit.
    pub fn url(&self) -> String {
        format!("http://{}/?token={}", self.addr, self.token)
    }

    /// Blocks until the server stops.
    pub fn wait(self) -> EndReason {
        let _ = self.accept.join();
        self.shared
            .end
            .lock()
            .expect("end")
            .take()
            .unwrap_or(EndReason::Quit)
    }

    /// Asks the server to stop (as `POST /api/quit` does).
    pub fn stop(&self) {
        self.shared.finish(EndReason::Quit);
        self.shared.shutdown.store(true, Ordering::Relaxed);
    }
}

fn accept_loop(listener: &TcpListener, sh: &Arc<Shared>, idle: Option<Duration>) {
    let live = Arc::new(AtomicUsize::new(0));
    while !sh.shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                if live.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    drop(stream);
                    continue;
                }
                live.fetch_add(1, Ordering::Relaxed);
                let (conn_sh, conn_live) = (Arc::clone(sh), Arc::clone(&live));
                let spawned = std::thread::Builder::new()
                    .name("audigma-web-conn".into())
                    .spawn(move || {
                        serve_connection(stream, &conn_sh);
                        conn_live.fetch_sub(1, Ordering::Relaxed);
                    });
                if spawned.is_err() {
                    live.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
        if let Some(limit) = idle {
            let quiet = sh.last_activity.lock().expect("activity").elapsed() > limit;
            if quiet && !sh.session().busy {
                sh.finish(EndReason::Idle);
                sh.shutdown.store(true, Ordering::Relaxed);
            }
        }
    }
    // Stop a running benchmark and give it a moment to wind down.
    for job in sh.jobs.lock().expect("jobs").iter() {
        job.cancel();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while sh.session().busy && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn serve_connection(mut stream: TcpStream, sh: &Arc<Shared>) {
    let _ = stream.set_nonblocking(false);
    let resp = match http::read_request(&mut stream) {
        Ok(req) => api::handle(sh, &req),
        Err(http::ReadError::Closed | http::ReadError::Io) => return,
        Err(http::ReadError::TooLarge) => {
            Response::json(413, &session::ApiError::new("the request is too large"))
        }
        Err(http::ReadError::Bad(why)) => Response::json(400, &session::ApiError::new(why)),
    };
    let _ = http::write_response(&mut stream, &resp);
    if resp.then_shutdown {
        sh.shutdown.store(true, Ordering::Relaxed);
    }
}

/// `true` when `addr` is not a loopback address (the caller warns about plain HTTP).
pub fn is_exposed(addr: &SocketAddr) -> bool {
    !addr.ip().is_loopback()
}
