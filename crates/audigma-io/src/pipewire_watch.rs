//! Presence watcher for one named PipeWire sink.
//!
//! The daemon uses it the way it uses ALSA hotplug for a USB DAC: wait for the pinned
//! `output.target_node` to exist before opening the output stream, and notice when it goes away.
//! A background thread keeps a registry listener open and tracks the node by `node.name`.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;

/// How long `start` waits for the first registry snapshot.
const FIRST_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(3);
/// Granularity of the watcher thread's loop (stop requests are noticed within this).
const LOOP_TICK: Duration = Duration::from_millis(100);
/// Delay before the watcher reconnects after losing the PipeWire connection.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// Which registry objects carry the watched node name.
///
/// Several nodes may share a name briefly (a node being replaced), so presence is "at least one".
#[derive(Debug, Default)]
pub struct PresenceTracker {
    ids: HashSet<u32>,
}

impl PresenceTracker {
    /// Records a node announcement. Returns the new presence when it changed.
    pub fn node_added(&mut self, watched: &str, id: u32, node_name: Option<&str>) -> Option<bool> {
        if node_name != Some(watched) {
            return None;
        }
        let was = self.is_present();
        self.ids.insert(id);
        (was != self.is_present()).then(|| self.is_present())
    }

    /// Records a registry removal. Returns the new presence when it changed.
    pub fn removed(&mut self, id: u32) -> Option<bool> {
        let was = self.is_present();
        self.ids.remove(&id);
        (was != self.is_present()).then(|| self.is_present())
    }

    pub fn is_present(&self) -> bool {
        !self.ids.is_empty()
    }

    fn clear(&mut self) {
        self.ids.clear();
    }
}

#[derive(Default)]
struct State {
    present: bool,
    synced: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// Mirror of `State::present` that other threads can poll without taking the lock.
    present_flag: Arc<AtomicBool>,
}

impl Shared {
    fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            present_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    fn set(&self, present: bool, synced: bool) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.present = present;
        st.synced |= synced;
        self.present_flag.store(present, Ordering::SeqCst);
        self.changed.notify_all();
    }

    fn present(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).present
    }

    /// Waits until the node is present, `timeout` elapses or `cancel` is set. Returns presence.
    fn wait_present(&self, timeout: Duration, cancel: &AtomicBool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while !st.present && !cancel.load(Ordering::SeqCst) {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            // Wake at least every 50 ms so a cancel request is not missed.
            let slice = (deadline - now).min(Duration::from_millis(50));
            st = self
                .changed
                .wait_timeout(st, slice)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        st.present
    }

    fn wait_synced(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while !st.synced {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            st = self
                .changed
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// Watches whether a PipeWire node with a given `node.name` exists.
pub struct PipeWireSinkWatcher {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PipeWireSinkWatcher {
    /// Starts watching `node_name` and waits (up to a few seconds) for the first snapshot, so
    /// `is_present` is meaningful on return. Without a reachable PipeWire daemon the node counts
    /// as absent and the thread keeps trying to connect.
    pub fn start(node_name: &str) -> Self {
        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop, name) = (shared.clone(), stop.clone(), node_name.to_string());
            std::thread::Builder::new()
                .name("audigma-pw-watch".to_string())
                .spawn(move || watch_loop(&name, &shared, &stop))
                .ok()
        };
        shared.wait_synced(FIRST_SNAPSHOT_TIMEOUT);
        Self {
            shared,
            stop,
            thread,
        }
    }

    /// Whether the node currently exists.
    pub fn is_present(&self) -> bool {
        self.shared.present()
    }

    /// A flag that is `true` while the node exists. Output code polls it to notice a removed
    /// sink without waiting for its own timeouts.
    pub fn presence_flag(&self) -> Arc<AtomicBool> {
        self.shared.present_flag.clone()
    }

    /// Blocks until the node appears, `timeout` elapses or `cancel` is set. Returns presence.
    pub fn wait_present(&self, timeout: Duration, cancel: &AtomicBool) -> bool {
        self.shared.wait_present(timeout, cancel)
    }
}

impl Drop for PipeWireSinkWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn watch_loop(name: &str, shared: &Arc<Shared>, stop: &AtomicBool) {
    pw::init();
    while !stop.load(Ordering::SeqCst) {
        if watch_once(name, shared, stop).is_err() {
            // Not connected (no daemon yet) or the connection dropped: nothing is known to exist.
            shared.set(false, true);
            let until = Instant::now() + RECONNECT_DELAY;
            while Instant::now() < until && !stop.load(Ordering::SeqCst) {
                std::thread::sleep(LOOP_TICK);
            }
        }
    }
}

/// One connection's lifetime. Returns `Err` when the connection could not be made or was lost.
fn watch_once(name: &str, shared: &Arc<Shared>, stop: &AtomicBool) -> Result<(), String> {
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| e.to_string())?;
    let registry = core.get_registry_rc().map_err(|e| e.to_string())?;

    let tracker = Rc::new(RefCell::new(PresenceTracker::default()));
    let lost = Rc::new(std::cell::Cell::new(false));
    let pending = core.sync(0).map_err(|e| e.to_string())?;

    let _core_listener = core
        .add_listener_local()
        .done({
            let shared = shared.clone();
            let tracker = tracker.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    shared.set(tracker.borrow().is_present(), true);
                }
            }
        })
        .error({
            let lost = lost.clone();
            move |id, _seq, _res, _msg| {
                if id == pw::core::PW_ID_CORE {
                    lost.set(true);
                }
            }
        })
        .register();
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let tracker = tracker.clone();
            let shared = shared.clone();
            let name = name.to_string();
            move |global| {
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let node_name = global.props.and_then(|p| p.get("node.name"));
                if let Some(present) = tracker.borrow_mut().node_added(&name, global.id, node_name)
                {
                    shared.set(present, false);
                }
            }
        })
        .global_remove({
            let tracker = tracker.clone();
            let shared = shared.clone();
            move |id| {
                if let Some(present) = tracker.borrow_mut().removed(id) {
                    shared.set(present, false);
                }
            }
        })
        .register();

    while !stop.load(Ordering::SeqCst) && !lost.get() {
        mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(LOOP_TICK));
    }
    tracker.borrow_mut().clear();
    if lost.get() {
        Err("PipeWire connection lost".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_reports_only_changes_for_the_watched_name() {
        let mut t = PresenceTracker::default();
        assert_eq!(t.node_added("sink", 5, Some("other")), None);
        assert!(!t.is_present());
        assert_eq!(t.node_added("sink", 7, Some("sink")), Some(true));
        // A second node with the same name does not change presence.
        assert_eq!(t.node_added("sink", 9, Some("sink")), None);
        assert_eq!(t.removed(7), None);
        assert!(t.is_present());
        assert_eq!(t.removed(9), Some(false));
        assert_eq!(t.removed(9), None);
    }

    #[test]
    fn tracker_ignores_nodes_without_a_name() {
        let mut t = PresenceTracker::default();
        assert_eq!(t.node_added("sink", 1, None), None);
        assert!(!t.is_present());
    }

    #[test]
    fn wait_returns_when_presence_arrives() {
        let shared = Arc::new(Shared::new());
        let cancel = AtomicBool::new(false);
        let s2 = shared.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            s2.set(true, true);
        });
        assert!(shared.wait_present(Duration::from_secs(2), &cancel));
        t.join().unwrap();
    }

    #[test]
    fn presence_flag_follows_state() {
        let shared = Shared::new();
        let flag = shared.present_flag.clone();
        assert!(!flag.load(Ordering::SeqCst));
        shared.set(true, true);
        assert!(flag.load(Ordering::SeqCst));
        shared.set(false, false);
        assert!(!flag.load(Ordering::SeqCst));
    }

    #[test]
    fn wait_times_out_and_honours_cancel() {
        let shared = Shared::new();
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        assert!(!shared.wait_present(Duration::from_millis(120), &cancel));
        assert!(started.elapsed() >= Duration::from_millis(100));
        cancel.store(true, Ordering::SeqCst);
        let started = Instant::now();
        assert!(!shared.wait_present(Duration::from_secs(5), &cancel));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
