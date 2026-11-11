//! StatusNotifierItem tray via `ksni` (blocking API, no async runtime).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use audigma_engine::DaemonControlHandle;
use audigma_settings::SystrayMode;
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{MenuItem, StandardItem};

use super::model::{MenuEntry, TrayModel};
use super::{REFRESH_INTERVAL, perform};

struct DaemonTray {
    id: String,
    model: TrayModel,
    control: DaemonControlHandle,
    /// False while no StatusNotifierWatcher (tray host) is reachable.
    host_available: Arc<AtomicBool>,
}

impl ksni::Tray for DaemonTray {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn title(&self) -> String {
        self.model.title()
    }

    fn icon_name(&self) -> String {
        self.model.icon_name().to_string()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.model.title(),
            description: self.model.status_line(),
            ..Default::default()
        }
    }

    fn watcher_online(&self) {
        self.host_available.store(true, Ordering::Relaxed);
        tracing::info!(
            target: "audigma::daemon",
            event = "tray_host_online",
            "tray host available; icon registered"
        );
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        self.host_available.store(false, Ordering::Relaxed);
        tracing::info!(
            target: "audigma::daemon",
            event = "tray_host_offline",
            reason = ?reason,
            "no tray host; the icon appears when one starts"
        );
        true
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.model
            .menu()
            .into_iter()
            .map(|entry| match entry {
                MenuEntry::Status(text) => StandardItem {
                    label: text,
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                MenuEntry::Separator => MenuItem::Separator,
                MenuEntry::Action {
                    label,
                    action,
                    enabled,
                } => StandardItem {
                    label: label.to_string(),
                    enabled,
                    activate: Box::new(move |tray: &mut Self| {
                        // pause/resume block for up to seconds: never on the tray's thread.
                        let control = tray.control.clone();
                        std::thread::spawn(move || perform(&control, action));
                    }),
                    ..Default::default()
                }
                .into(),
            })
            .collect()
    }
}

/// A registered tray plus the thread that pushes model changes to it.
pub(super) struct LinuxTray {
    host_available: Arc<AtomicBool>,
    handle: Handle<DaemonTray>,
    stop: Arc<AtomicBool>,
    refresher: Option<JoinHandle<()>>,
}

impl LinuxTray {
    pub(super) fn start(
        mode: SystrayMode,
        instance: &str,
        control: &DaemonControlHandle,
    ) -> Result<Self, String> {
        let model = TrayModel::from_telemetry(instance, &control.get_telemetry());
        let host_available = Arc::new(AtomicBool::new(true));
        let tray = DaemonTray {
            id: format!("audigma-{instance}"),
            model,
            control: control.clone(),
            host_available: Arc::clone(&host_available),
        };
        // `auto` tolerates a missing watcher/host: the icon appears when one shows up later.
        // `required` fails on a missing watcher or host.
        let handle = tray
            .assume_sni_available(mode == SystrayMode::Auto)
            .spawn()
            .map_err(|e| format!("cannot register the tray icon: {e}"))?;

        let stop = Arc::new(AtomicBool::new(false));
        let refresher = {
            let handle = handle.clone();
            let stop = Arc::clone(&stop);
            let control = control.clone();
            let instance = instance.to_string();
            std::thread::Builder::new()
                .name("audigma-tray".into())
                .spawn(move || refresh_loop(handle, stop, control, instance))
                .map_err(|e| format!("cannot start the tray refresh thread: {e}"))?
        };
        Ok(Self {
            host_available,
            handle,
            stop,
            refresher: Some(refresher),
        })
    }
}

impl LinuxTray {
    /// Whether a tray host accepted the icon (false while waiting for one to start).
    pub(super) fn host_available(&self) -> bool {
        self.host_available.load(Ordering::Relaxed)
    }
}

/// Pushes the model to the tray when it changed, at most once per `REFRESH_INTERVAL`.
fn refresh_loop(
    handle: Handle<DaemonTray>,
    stop: Arc<AtomicBool>,
    control: DaemonControlHandle,
    instance: String,
) {
    let mut last: Option<TrayModel> = None;
    while !stop.load(Ordering::Relaxed) && !handle.is_closed() {
        let model = TrayModel::from_telemetry(&instance, &control.get_telemetry());
        if last.as_ref() != Some(&model) {
            let m = model.clone();
            if handle.update(move |t| t.model = m).is_none() {
                break;
            }
            last = Some(model);
        }
        std::thread::park_timeout(REFRESH_INTERVAL);
    }
}

impl Drop for LinuxTray {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.refresher.take() {
            t.thread().unpark();
            let _ = t.join();
        }
        self.handle.shutdown().wait();
    }
}
