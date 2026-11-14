//! Per-instance persistence of the user volume and mute state (`[runtime] persist_volume`,
//! `Design.md` §6.1.2).
//!
//! The daemon restores `<state dir>/<instance>.state.toml` once at startup and a poller thread
//! saves it again whenever the volume or mute state changes. Polling the control atomics keeps
//! the filesystem off every writer (IPC, tray, input driver) and debounces bursts for free.
//!
//! The volume is stored in the unit the user chose: a percentage maps to dB through a curve
//! that depends on the active volume mode and the DAC's mixer range, so converting it here
//! would freeze the wrong curve.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use audigma_engine::DaemonControlHandle;
use serde::{Deserialize, Serialize};

use crate::ipc::Instance;

/// How often the control atomics are compared with what was last saved.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The user's volume, in the unit it was set in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Volume {
    Db(f64),
    Percent(f64),
}

/// Everything persisted for one instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SavedState {
    pub volume: Volume,
    pub muted: bool,
}

/// On-disk shape. `volume_source_percent` says which volume key is authoritative, so the file
/// never relies on a precedence rule: the flagged key must be present and valid, the other is
/// ignored. The writer stores only the flagged key (the daemon's percent atomic is unset while a
/// dB value is active). Unknown keys are ignored so later versions can add fields.
#[derive(Debug, Serialize, Deserialize)]
struct StateFile {
    volume_source_percent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_percent: Option<f64>,
    #[serde(default)]
    muted: bool,
}

impl From<SavedState> for StateFile {
    fn from(s: SavedState) -> Self {
        let (volume_db, volume_percent) = match s.volume {
            Volume::Db(db) => (Some(db), None),
            Volume::Percent(pct) => (None, Some(pct)),
        };
        Self {
            volume_source_percent: volume_percent.is_some(),
            volume_db,
            volume_percent,
            muted: s.muted,
        }
    }
}

impl StateFile {
    fn into_state(self) -> Result<SavedState, String> {
        let volume = if self.volume_source_percent {
            match self.volume_percent {
                Some(pct) if pct.is_finite() && pct >= 0.0 => Volume::Percent(pct),
                Some(pct) => return Err(format!("invalid volume_percent {pct}")),
                None => {
                    return Err("volume_source_percent = true but volume_percent is missing".into());
                }
            }
        } else {
            match self.volume_db {
                Some(db) if db.is_finite() => Volume::Db(db),
                Some(db) => return Err(format!("invalid volume_db {db}")),
                None => return Err("volume_source_percent = false but volume_db is missing".into()),
            }
        };
        Ok(SavedState {
            volume,
            muted: self.muted,
        })
    }
}

/// `$XDG_STATE_HOME/audigma`, else `~/.local/state/audigma`; `None` when neither is known.
pub fn state_dir() -> Option<PathBuf> {
    state_dir_with(|k| std::env::var(k).ok())
}

fn state_dir_with(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let non_empty = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(xdg) = non_empty("XDG_STATE_HOME") {
        Some(PathBuf::from(xdg).join("audigma"))
    } else {
        non_empty("HOME").map(|home| PathBuf::from(home).join(".local/state/audigma"))
    }
}

/// State file of `instance` inside `dir`. Instance names are validated (`is_valid_instance_name`),
/// so they are safe as a file stem.
pub fn state_path(dir: &Path, instance: &str) -> PathBuf {
    dir.join(format!("{instance}.state.toml"))
}

/// Reads the state file. `Ok(None)`: no file yet.
pub fn load(path: &Path) -> Result<Option<SavedState>, String> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    toml::from_str::<StateFile>(&text)
        .map_err(|e| e.to_string())?
        .into_state()
        .map(Some)
}

/// Writes the state file atomically (temp file in the same directory, then rename), so a crash
/// never leaves a half-written file behind.
pub fn save(path: &Path, state: SavedState) -> io::Result<()> {
    let text = toml::to_string(&StateFile::from(state)).map_err(io::Error::other)?;
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    drop(file);
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// The user's current volume intent and mute state. A percentage takes priority over dB, as in
/// the daemon's gain computation.
pub fn snapshot(control: &DaemonControlHandle) -> SavedState {
    SavedState {
        volume: match control.get_volume_percent() {
            Some(pct) => Volume::Percent(pct),
            None => Volume::Db(control.get_volume()),
        },
        muted: control.is_muted(),
    }
}

/// Applies a saved state through the same setters IPC uses (which clamp dB).
pub fn restore(control: &DaemonControlHandle, state: SavedState) {
    match state.volume {
        Volume::Db(db) => control.set_volume(db),
        Volume::Percent(pct) => control.set_volume_percent(pct),
    }
    control.set_muted(state.muted);
}

/// Saves volume and mute changes in the background until dropped. Dropping stops the poller
/// after one final comparison, so a change made just before shutdown is not lost.
pub struct Persister {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Persister {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

/// Restores the instance's saved state into `control` and starts saving changes. Returns `None`
/// (after a warning) when no state directory can be determined or the thread cannot start.
/// A missing file is normal on first run; an unreadable one is reported and ignored.
pub fn start(instance: &Instance, control: &DaemonControlHandle) -> Option<Persister> {
    let Some(dir) = state_dir() else {
        tracing::warn!(
            target: "audigma::daemon",
            event = audigma_log::VOLUME_PERSIST,
            "neither $XDG_STATE_HOME nor $HOME is set; volume is not persisted"
        );
        return None;
    };
    start_at(state_path(&dir, instance.name()), control, POLL_INTERVAL)
}

fn start_at(path: PathBuf, control: &DaemonControlHandle, poll: Duration) -> Option<Persister> {
    match load(&path) {
        Ok(Some(state)) => {
            restore(control, state);
            tracing::info!(
                target: "audigma::daemon",
                event = audigma_log::VOLUME_PERSIST,
                path = %path.display(),
                volume = ?state.volume,
                muted = state.muted,
                "restored saved volume"
            );
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(
            target: "audigma::daemon",
            event = audigma_log::VOLUME_PERSIST,
            path = %path.display(),
            error = %e,
            "ignoring unreadable volume state file"
        ),
    }

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    // Snapshot here, not in the thread: a change made right after `start_at` returns must
    // count as a change.
    let saved = snapshot(control);
    let control = control.clone();
    let spawned = thread::Builder::new()
        .name("audigma-persist".to_string())
        .spawn(move || run_poller(&path, &control, &thread_stop, poll, saved));
    match spawned {
        Ok(handle) => Some(Persister {
            stop,
            handle: Some(handle),
        }),
        Err(e) => {
            tracing::warn!(
                target: "audigma::daemon",
                event = audigma_log::VOLUME_PERSIST,
                error = %e,
                "cannot start the volume persister; volume is not persisted"
            );
            None
        }
    }
}

/// `saved` is the state already on disk (or restored from it), so an unchanged volume never
/// rewrites the file.
fn run_poller(
    path: &Path,
    control: &DaemonControlHandle,
    stop: &AtomicBool,
    poll: Duration,
    mut saved: SavedState,
) {
    let mut failing = false;
    loop {
        thread::park_timeout(poll);
        let stopping = stop.load(Ordering::SeqCst);
        let current = snapshot(control);
        if current != saved {
            match save(path, current) {
                Ok(()) => {
                    saved = current;
                    failing = false;
                }
                // Retried on the next tick; reported once per failure streak.
                Err(e) if !failing => {
                    failing = true;
                    tracing::warn!(
                        target: "audigma::daemon",
                        event = audigma_log::VOLUME_PERSIST,
                        path = %path.display(),
                        error = %e,
                        "cannot save volume state"
                    );
                }
                Err(_) => {}
            }
        }
        if stopping {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audigma_engine::DaemonOutputFormat;

    fn handle() -> DaemonControlHandle {
        DaemonControlHandle::new(44_100, DaemonOutputFormat::Pcm, 192_000, "sinc", 0.0)
    }

    /// Polls so rarely that only the final flush on drop can write.
    const NEVER: Duration = Duration::from_secs(3600);

    #[test]
    fn state_dir_prefers_xdg_then_home() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let both = &[("XDG_STATE_HOME", "/xdg"), ("HOME", "/home/u")];
        assert_eq!(state_dir_with(env(both)), Some("/xdg/audigma".into()));
        assert_eq!(
            state_dir_with(env(&both[1..])),
            Some("/home/u/.local/state/audigma".into())
        );
        // An empty XDG_STATE_HOME counts as unset, per the XDG spec.
        let empty = &[("XDG_STATE_HOME", ""), ("HOME", "/home/u")];
        assert_eq!(
            state_dir_with(env(empty)),
            Some("/home/u/.local/state/audigma".into())
        );
        assert_eq!(state_dir_with(env(&[])), None);
    }

    #[test]
    fn save_then_load_round_trips_both_units() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(&dir.path().join("nested"), "k17");
        for state in [
            SavedState {
                volume: Volume::Db(-12.5),
                muted: false,
            },
            SavedState {
                volume: Volume::Percent(37.0),
                muted: true,
            },
        ] {
            save(&path, state).unwrap();
            assert_eq!(load(&path).unwrap(), Some(state));
        }
        assert!(!path.with_extension("tmp").exists());
    }

    #[test]
    fn load_handles_missing_foreign_and_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.state.toml");
        assert_eq!(load(&path).unwrap(), None);

        // The flag picks the source; the other key is ignored, even when invalid or absent.
        // Unknown keys are ignored. `muted` defaults to false.
        for (text, want) in [
            (
                "volume_source_percent = true\nvolume_db = -3.0\nvolume_percent = 50.0\nfuture = 1\n",
                Volume::Percent(50.0),
            ),
            (
                "volume_source_percent = false\nvolume_db = -3.0\nvolume_percent = 50.0\n",
                Volume::Db(-3.0),
            ),
            (
                "volume_source_percent = false\nvolume_db = -3.0\nvolume_percent = -1.0\n",
                Volume::Db(-3.0),
            ),
            (
                "volume_source_percent = true\nvolume_db = nan\nvolume_percent = 50.0\n",
                Volume::Percent(50.0),
            ),
        ] {
            fs::write(&path, text).unwrap();
            assert_eq!(
                load(&path).unwrap(),
                Some(SavedState {
                    volume: want,
                    muted: false
                }),
                "{text}"
            );
        }

        for bad in [
            // The flagged source is missing or invalid.
            "volume_source_percent = true\nvolume_db = -3.0\n",
            "volume_source_percent = false\nvolume_percent = 50.0\n",
            "volume_source_percent = true\nvolume_percent = -1.0\n",
            "volume_source_percent = false\nvolume_db = nan\n",
            // The flag is required, and typed.
            "volume_db = -3.0\n",
            "muted = true\n",
            "volume_source_percent = \"yes\"\nvolume_db = -3.0\n",
            "volume_source_percent = false\nvolume_db = \"loud\"\n",
            "not toml at all [",
        ] {
            fs::write(&path, bad).unwrap();
            assert!(load(&path).is_err(), "{bad}");
        }
    }

    #[test]
    fn restore_applies_state_and_snapshot_reads_it_back() {
        let control = handle();
        restore(
            &control,
            SavedState {
                volume: Volume::Db(-20.0),
                muted: true,
            },
        );
        let snap = snapshot(&control);
        assert_eq!(snap.volume, Volume::Db(-20.0));
        assert!(snap.muted);

        restore(
            &control,
            SavedState {
                volume: Volume::Percent(60.0),
                muted: false,
            },
        );
        assert_eq!(snapshot(&control).volume, Volume::Percent(60.0));
    }

    #[test]
    fn restart_restores_what_the_last_run_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(dir.path(), "default");

        let first = handle();
        let persister = start_at(path.clone(), &first, NEVER).unwrap();
        first.set_volume(-7.5);
        first.set_muted(true);
        drop(persister); // final flush

        let second = handle();
        let _persister = start_at(path.clone(), &second, NEVER).unwrap();
        assert_eq!(second.get_volume(), -7.5);
        assert_eq!(second.get_volume_percent(), None);
        assert!(second.is_muted());
    }

    #[test]
    fn unchanged_state_never_creates_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(dir.path(), "default");
        let control = handle();
        drop(start_at(path.clone(), &control, NEVER).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn corrupt_file_is_ignored_and_replaced_on_next_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(dir.path(), "default");
        fs::write(&path, "garbage [").unwrap();

        let control = handle();
        let persister = start_at(path.clone(), &control, NEVER).unwrap();
        assert_eq!(control.get_volume_percent(), Some(100.0));
        control.set_volume_percent(40.0);
        drop(persister);

        assert_eq!(load(&path).unwrap().unwrap().volume, Volume::Percent(40.0));
    }

    #[test]
    fn running_poller_saves_without_waiting_for_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path(dir.path(), "default");
        let control = handle();
        let _persister = start_at(path.clone(), &control, Duration::from_millis(10)).unwrap();
        control.set_volume(-1.0);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while load(&path).ok().flatten().map(|s| s.volume) != Some(Volume::Db(-1.0)) {
            assert!(std::time::Instant::now() < deadline, "state never saved");
            thread::sleep(Duration::from_millis(10));
        }
    }
}
