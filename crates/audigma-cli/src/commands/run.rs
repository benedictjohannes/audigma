//! Handler for `audigma run` daemon subcommand.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::{Duration, Instant};

use audigma_dsp::config::TargetRateSpec;
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::PauseReason;
use audigma_engine::StreamExitReason;
use audigma_engine::live_pipeline::{
    DaemonControlHandle, DaemonOutputFormat, DaemonState, DaemonVolumeMode, DspSession,
    LivePipeline,
};
use audigma_engine::traits::{AudioInput, AudioOutput, EngineError};
use audigma_io::alsa::{
    AlsaAudioOutput, AlsaHardwareMixer, AlsaOutputConfig, is_alsa_device_present,
    resolve_card_index, wait_for_alsa_device,
};
#[cfg(feature = "dbus")]
use audigma_io::dbus::DeviceReservation;
#[cfg(feature = "pipewire")]
use audigma_io::pipewire::{PipeWirePcmOutput, PipeWireVirtualSink};
use audigma_settings::DeviceWaitExpiry;
use audigma_settings::{InputBackend, OutputBackend, Settings};
use clap::Args;

use super::resolve_config_path;
use crate::error::CliError;
use crate::ipc::{ControlServer, Instance};

/// Atomic pointer to the daemon running flag for signal handler shutdown (§9, §13.12).
static DAEMON_RUNNING_PTR: AtomicPtr<AtomicBool> = AtomicPtr::new(std::ptr::null_mut());

extern "C" fn sigint_sigterm_handler(_: libc::c_int) {
    let ptr = DAEMON_RUNNING_PTR.load(Ordering::SeqCst);
    if !ptr.is_null() {
        unsafe {
            (*ptr).store(false, Ordering::SeqCst);
        }
    }
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Optional configuration file path (default: $AUDIGMA_CONFIG, else ~/.config/audigma/config.toml)
    #[arg(short = 'c', long = "config", env = "AUDIGMA_CONFIG")]
    pub config: Option<PathBuf>,

    #[command(flatten)]
    pub instance: super::InstanceArgs,
}

/// Loads, validates and instance-names the configuration (`run` start and every `resume`).
fn load_settings(config_path: &std::path::Path, instance: &Instance) -> Result<Settings, CliError> {
    if !config_path.exists() {
        return Err(CliError::ConfigNotFound {
            path: config_path.to_path_buf(),
        });
    }
    let content = std::fs::read_to_string(config_path)?;
    let mut settings = Settings::from_toml_str(&content)?;
    settings.validate()?;
    for warning in settings.warnings() {
        tracing::warn!(event = "config_warning", "{warning}");
    }

    // An explicit input node name wins; otherwise derive one from the instance so that
    // concurrent instances never create identically named virtual sinks.
    // `[input]` is optional (every key defaults), so a config without it still runs.
    let input = settings.input.get_or_insert_with(Default::default);
    if input
        .node_name
        .as_deref()
        .is_none_or(|n| n.trim().is_empty())
    {
        input.node_name = Some(format!("audigma_sink_{}", instance.name()));
    }
    Ok(settings)
}

/// Daemon-level telemetry values derived from a configuration.
struct StreamSummary {
    input_rate_hz: u32,
    output_format: DaemonOutputFormat,
    output_rate_hz: u32,
    filter: String,
    preamp_db: f64,
}

fn summarize(settings: &Settings) -> Result<StreamSummary, CliError> {
    let in_cfg = settings
        .input
        .as_ref()
        .ok_or_else(|| CliError::Config("missing [input] section in config".to_string()))?;
    if settings.output.is_none() {
        return Err(CliError::Config(
            "missing [output] section in config".to_string(),
        ));
    }
    let input_rate = in_cfg.rate_hz.unwrap_or(PcmRateHz::PCM_48000);
    let plan = settings.explain_for_input_rate(input_rate)?;
    let (output_format, output_rate_hz) = match plan.stream_spec.target {
        TargetRateSpec::Dsd { bit_rate } => (DaemonOutputFormat::Dsd, bit_rate.get()),
        TargetRateSpec::Pcm { rate, .. } => (DaemonOutputFormat::Pcm, rate.get()),
    };
    Ok(StreamSummary {
        input_rate_hz: input_rate.get(),
        output_format,
        output_rate_hz,
        filter: plan.dsp_config.filter_description(),
        preamp_db: plan.dsp_config.preamp_db,
    })
}

/// Whether the configured output exists, for the device-wait logic.
///
/// ALSA outputs are present while the card is; a PipeWire output with `target_node` set is
/// present while a node with that `node.name` is in the registry. A PipeWire output without a
/// target (the default sink) has nothing to wait for.
struct OutputPresence {
    alsa_device: Option<String>,
    #[cfg(feature = "pipewire")]
    pipewire: Option<audigma_io::PipeWireSinkWatcher>,
}

impl OutputPresence {
    fn new(out_cfg: &audigma_settings::OutputConfig, device: &str) -> Self {
        #[cfg(feature = "pipewire")]
        let pipewire = (out_cfg.backend == Some(OutputBackend::Pipewire))
            .then(|| out_cfg.target_node.as_deref().map(str::trim))
            .flatten()
            .filter(|n| !n.is_empty())
            .map(audigma_io::PipeWireSinkWatcher::start);
        Self {
            alsa_device: (out_cfg.backend == Some(OutputBackend::Alsa)).then(|| device.to_string()),
            #[cfg(feature = "pipewire")]
            pipewire,
        }
    }

    fn is_present(&self) -> bool {
        if let Some(device) = &self.alsa_device {
            return is_alsa_device_present(device);
        }
        #[cfg(feature = "pipewire")]
        if let Some(watcher) = &self.pipewire {
            return watcher.is_present();
        }
        true
    }

    /// Blocks up to `ms` until the output is present (hotplug event for ALSA, registry event
    /// for PipeWire). Returns whether it is present.
    fn wait_present(&self, ms: i64, cancel: &AtomicBool) -> bool {
        if let Some(device) = &self.alsa_device {
            return wait_for_alsa_device(device, ms, cancel).unwrap_or(false);
        }
        #[cfg(feature = "pipewire")]
        if let Some(watcher) = &self.pipewire {
            return watcher.wait_present(Duration::from_millis(ms.max(0) as u64), cancel);
        }
        true
    }

    /// The pinned PipeWire sink's presence flag, for the output stream to poll.
    fn pipewire_flag(&self) -> Option<Arc<AtomicBool>> {
        #[cfg(feature = "pipewire")]
        if let Some(watcher) = &self.pipewire {
            return Some(watcher.presence_flag());
        }
        None
    }

    /// A PipeWire target that is pinned but absent. A streaming I/O error in this state is the
    /// output stream starving because its sink went away, so it is handled as a device loss.
    fn pipewire_target_missing(&self) -> bool {
        #[cfg(feature = "pipewire")]
        if let Some(watcher) = &self.pipewire {
            return !watcher.is_present();
        }
        false
    }
}

/// How often the output-loop guard samples the PipeWire graph.
#[cfg(feature = "pipewire")]
const OUTPUT_LOOP_POLL: Duration = Duration::from_secs(5);

/// Starts a low-priority thread that reports when the PipeWire output stream is linked into
/// audigma's own input sink.
///
/// The session manager links a stream to the default sink when its `target.object` is unset or
/// matches no node, and the default sink can be audigma's own virtual sink. The audio then
/// recirculates inside audigma and nothing reaches a device, so the daemon looks healthy while
/// playing silence. The check looks at the actual links, so it also catches a stale
/// `output.target_node` and a default sink that changes while running. It logs once when the
/// loop appears and once when it clears.
#[cfg(feature = "pipewire")]
fn spawn_output_loop_guard(settings: &Settings, instance: &Instance, running: Arc<AtomicBool>) {
    let Some(sink_node) = settings.input.as_ref().and_then(|i| i.node_name.clone()) else {
        return;
    };
    let stream_node = format!("audigma_pcm_out_{}", instance.name());
    let spawned = std::thread::Builder::new()
        .name("audigma-loop-guard".to_string())
        .spawn(move || {
            let mut looped = false;
            let mut next_poll = Instant::now() + Duration::from_secs(2);
            while running.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
                if Instant::now() < next_poll {
                    continue;
                }
                next_poll = Instant::now() + OUTPUT_LOOP_POLL;
                let Ok(targets) = audigma_io::output_link_targets(&stream_node) else {
                    continue;
                };
                let now = targets.contains(&sink_node);
                if now && !looped {
                    tracing::error!(
                        target: "audigma::daemon",
                        event = "output_loop",
                        stream = %stream_node,
                        sink = %sink_node,
                        "the PipeWire output is linked to audigma's own input sink, so audio recirculates and nothing is played; set `output.target_node` to a hardware sink, or choose another default sink"
                    );
                } else if !now && looped {
                    tracing::info!(
                        target: "audigma::daemon",
                        event = "output_loop_cleared",
                        stream = %stream_node,
                        "the PipeWire output is no longer linked to audigma's own sink"
                    );
                }
                looped = now;
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(target: "audigma::daemon", "could not start the output-loop guard: {e}");
    }
}

pub fn run_run(args: RunArgs) -> Result<(), CliError> {
    let instance = match args.instance.instance.clone() {
        Some(name) => Instance::new(name)?,
        None => Instance::default_instance(),
    };
    let daemon_start_time = Instant::now();
    let config_path = resolve_config_path(args.config.as_deref());
    let mut settings = load_settings(&config_path, &instance)?;

    // Take the single-instance lock before touching any device or PipeWire node, so a duplicate
    // instance fails fast without disturbing the running one.
    let instance_lock = instance.acquire_lock()?;

    #[cfg(feature = "pipewire")]
    if settings.output_backend() == Some(OutputBackend::Pipewire)
        && settings
            .output
            .as_ref()
            .is_none_or(|o| o.target_node.as_deref().is_none_or(|n| n.trim().is_empty()))
    {
        tracing::warn!(
            target: "audigma::daemon",
            event = "config_warning",
            "`output.target_node`: unset with a PipeWire output; if audigma's own sink becomes the default sink, the output is linked back into its own input and playback is silent. Setting it is strongly recommended"
        );
    }

    let summary = summarize(&settings)?;
    let channels = settings
        .input
        .as_ref()
        .and_then(|i| i.channels)
        .unwrap_or(2);

    // Resolve placement once, before any thread is spawned or affinity narrowed, then restrict
    // the supervisor to the housekeeping set so helper threads inherit it. Placement is fixed
    // for the process lifetime: a `[runtime]` change needs a restart, not a resume.
    let placement = crate::runtime::resolve(&settings, usize::from(channels), false)?;
    crate::runtime::apply_housekeeping(&placement);

    let control = DaemonControlHandle::new(
        summary.input_rate_hz,
        summary.output_format,
        summary.output_rate_hz,
        &summary.filter,
        summary.preamp_db,
    );

    // Install SIGINT / SIGTERM handler to stop daemon cleanly (§9, §13.12)
    DAEMON_RUNNING_PTR.store(Arc::as_ptr(&control.running) as *mut _, Ordering::SeqCst);
    unsafe {
        libc::signal(
            libc::SIGINT,
            sigint_sigterm_handler as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            sigint_sigterm_handler as *const () as libc::sighandler_t,
        );
    }

    // System tray (`[runtime] systray`); read once at process start. The guard removes the icon
    // when the daemon exits. A `required` failure aborts before the control socket is bound.
    let _tray = crate::tray::start(settings.runtime_config().systray, &instance, &control)?;

    // Start Unix Control Socket. It outlives every pipeline session so `resume`/`stop` stay
    // reachable while paused.
    let _server = ControlServer::start(instance.clone(), instance_lock, control.clone())
        .map_err(CliError::Ipc)?;
    tracing::info!(
        target: "audigma::ipc",
        event = audigma_log::CONTROL_SOCKET_BOUND,
        instance = instance.name(),
        endpoint = %instance.display_endpoint(),
        "control socket bound"
    );

    // With a PipeWire output, watch for the output stream being linked into audigma's own sink.
    #[cfg(feature = "pipewire")]
    if settings.output_backend() == Some(OutputBackend::Pipewire) {
        spawn_output_loop_guard(&settings, &instance, control.running.clone());
    }

    // Supervisor loop: one pipeline session per iteration; a pause parks here until resume.
    let session_result: Result<(), CliError> = loop {
        let result = run_session(&settings, &placement, &control, &instance, &config_path);
        if result.is_err() || !control.is_running() || !control.pause_requested() {
            break result;
        }

        tracing::info!(
            target: "audigma::daemon",
            event = audigma_log::DAEMON_PAUSE,
            instance = instance.name(),
            "pipeline released; daemon paused"
        );
        let mut reloaded: Option<Settings> = None;
        let resumed = control.park_until_resume(Duration::from_millis(100), || {
            let new = load_settings(&config_path, &instance).map_err(|e| e.to_string())?;
            let new_summary = summarize(&new).map_err(|e| e.to_string())?;
            if new.runtime != settings.runtime {
                tracing::warn!(
                    target: "audigma::daemon",
                    event = "config_warning",
                    "[runtime] changed while paused; threading and placement apply only after a restart"
                );
            }
            control.reconfigure(
                new_summary.input_rate_hz,
                new_summary.output_format,
                new_summary.output_rate_hz,
                &new_summary.filter,
                new_summary.preamp_db,
            );
            reloaded = Some(new);
            Ok(())
        });
        if !resumed {
            break Ok(());
        }
        if let Some(new) = reloaded {
            settings = new;
        }
        tracing::info!(
            target: "audigma::daemon",
            event = audigma_log::DAEMON_RESUME,
            instance = instance.name(),
            "resuming pipeline"
        );
    };

    // Unregister signal handler
    DAEMON_RUNNING_PTR.store(std::ptr::null_mut(), Ordering::SeqCst);
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
    }

    session_result?;

    tracing::info!(
        target: "audigma::daemon",
        event = audigma_log::DAEMON_STOP,
        reason = "clean",
        uptime_s = daemon_start_time.elapsed().as_secs(),
        "daemon stopped cleanly"
    );

    Ok(())
}

/// Runs one pipeline session (devices, PipeWire sink, DSP workers, feed loop) until the daemon
/// is stopped or paused, then releases all of it.
fn run_session(
    settings: &Settings,
    placement: &audigma_engine::Placement,
    control: &DaemonControlHandle,
    instance: &Instance,
    config_path: &std::path::Path,
) -> Result<(), CliError> {
    let in_cfg = settings
        .input
        .as_ref()
        .ok_or_else(|| CliError::Config("missing [input] section in config".to_string()))?;
    let out_cfg = settings
        .output
        .as_ref()
        .ok_or_else(|| CliError::Config("missing [output] section in config".to_string()))?;

    let input_rate = in_cfg.rate_hz.unwrap_or(PcmRateHz::PCM_48000);
    let channels = in_cfg.channels.unwrap_or(2);

    let worker_setup = crate::runtime::worker_setup(placement);
    #[cfg(feature = "pipewire")]
    let pw_placement = placement.pipewire();

    let plan = settings.explain_for_input_rate(input_rate)?;
    let target_spec = plan.stream_spec.target;

    tracing::info!(
        target: "audigma::daemon",
        event = audigma_log::DAEMON_START,
        instance = instance.name(),
        config = %config_path.display(),
        input_backend = ?in_cfg.backend,
        input_rate_hz = input_rate.get(),
        channels,
        output_backend = ?out_cfg.backend,
        filter = %plan.dsp_config.filter_description(),
        "starting live daemon"
    );

    // Set up Input Backend helper closure
    let out_buf_ms = out_cfg.buffer_ms.unwrap_or(40);
    let target_preroll_secs = (out_buf_ms as f64 / 1000.0 + 0.050).clamp(0.060, 0.500);

    let create_input_driver =
        |in_cfg: &audigma_settings::InputConfig| -> Result<Box<dyn AudioInput>, EngineError> {
            match in_cfg.backend() {
                #[cfg(feature = "pipewire")]
                InputBackend::Pipewire => {
                    let mut sink = PipeWireVirtualSink::from_input_config_with_placement(
                        in_cfg,
                        input_rate.get(),
                        128,
                        Some(pw_placement.clone()),
                    )
                    .map_err(|e| EngineError::AudioBackend {
                        backend: "pipewire",
                        message: format!("failed to bind PipeWire virtual sink: {e}"),
                    })?;
                    sink.set_preroll_secs(target_preroll_secs);
                    Ok(Box::new(sink))
                }
                #[cfg(not(feature = "pipewire"))]
                InputBackend::Pipewire => Err(EngineError::AudioBackend {
                    backend: "pipewire",
                    message: "PipeWire feature is disabled in this build".to_string(),
                }),
            }
        };

    let mut input_driver: Option<Box<dyn AudioInput>> = Some(create_input_driver(in_cfg)?);

    // Helper closure to create/open output backend driver
    let create_output_driver = |out_cfg: &audigma_settings::OutputConfig,
                                target: &TargetRateSpec,
                                target_presence: Option<Arc<AtomicBool>>|
     -> Result<Box<dyn AudioOutput>, EngineError> {
        #[cfg(not(feature = "pipewire"))]
        let _ = &target_presence;
        match out_cfg.backend {
            Some(OutputBackend::Alsa) => {
                let alsa_config =
                    AlsaOutputConfig::from_settings(settings, target).map_err(|e| {
                        EngineError::DeviceUnavailable {
                            device: settings
                                .output
                                .as_ref()
                                .and_then(|o| o.device.clone())
                                .unwrap_or_else(|| "default".to_string()),
                            reason: format!("failed to configure ALSA output: {e}"),
                        }
                    })?;
                let target_device = alsa_config.device.clone();
                let alsa_out = AlsaAudioOutput::open(alsa_config).map_err(|e| match e {
                    audigma_io::AlsaOutputError::DeviceDisconnected(reason) => {
                        EngineError::DeviceDisconnected(reason)
                    }
                    audigma_io::AlsaOutputError::Io(err) => EngineError::Io(err),
                    other => EngineError::DeviceUnavailable {
                        device: target_device,
                        reason: format!("failed to open ALSA output: {other}"),
                    },
                })?;
                Ok(Box::new(alsa_out))
            }
            #[cfg(feature = "pipewire")]
            Some(OutputBackend::Pipewire) => {
                let target_node = out_cfg.target_node.as_deref();
                let pw_rate = match target {
                    TargetRateSpec::Pcm { rate, .. } => rate.get(),
                    TargetRateSpec::Dsd { bit_rate } => {
                        return Err(EngineError::InvalidTopology {
                            message: format!(
                                "PipeWire PCM output cannot carry DSD bit rate {bit_rate}"
                            ),
                        });
                    }
                };
                let mut pw_out = PipeWirePcmOutput::new(
                    &format!("audigma_pcm_out_{}", instance.name()),
                    target_node,
                    pw_rate,
                    channels as u32,
                    out_buf_ms,
                    Some(pw_placement.clone()),
                )
                .map_err(|e| EngineError::AudioBackend {
                    backend: "pipewire",
                    message: format!("failed to initialize PipeWire output: {e}"),
                })?;
                if let Some(flag) = target_presence {
                    pw_out.set_target_presence(flag);
                }
                Ok(Box::new(pw_out))
            }
            #[cfg(not(feature = "pipewire"))]
            Some(OutputBackend::Pipewire) => Err(EngineError::AudioBackend {
                backend: "pipewire",
                message: "PipeWire feature is disabled in this build".to_string(),
            }),
            None => Err(EngineError::InvalidTopology {
                message: "[output] backend is required for 'audigma run'".to_string(),
            }),
        }
    };

    // Set up and spawn audigma-rt-log drainer thread (§8.4)
    let mut rt_drainer = audigma_log::RtLogDrainer::new();
    if let Some(ref mut input) = input_driver {
        if let Some(consumer) = input.take_rt_consumer() {
            let dropped = input
                .dropped_rt_events()
                .unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicU64::new(0)));
            rt_drainer.add_channel(consumer, dropped);
        }
        if let Some(recorder) = input.decimation_recorder() {
            rt_drainer.add_decimation_recorder(recorder);
        }
    }
    let mut _rt_drainer_handle = rt_drainer.spawn();

    // Daemon loop runs on its own `audigma-loop` thread; this thread stays SCHED_OTHER as supervisor.
    let (startup_tx, startup_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let loop_result: Result<(), CliError> = std::thread::scope(|scope| {
        let handle = std::thread::Builder::new()
            .name("audigma-loop".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn_scoped(scope, || -> Result<(), CliError> {
                // Loop placement: housekeeping affinity (or `loop_cpu`) + RT `p`
                // (RESET_ON_FORK: children start SCHED_OTHER)
                let loop_fatal = crate::runtime::rt_failure_is_fatal(placement, &placement.loop_);
                let loop_rt = crate::runtime::apply_logged(
                    "audigma-loop",
                    None,
                    &placement.loop_,
                    loop_fatal,
                );
                let fatal = loop_fatal && loop_rt.is_err();
                // Startup handshake: tell the supervisor whether placement succeeded.
                let _ = startup_tx.send(loop_rt.clone());
                if fatal {
                    return Err(CliError::Realtime(loop_rt.unwrap_err()));
                }

                    // D-Bus device reservation if exclusive ALSA output is selected
                    #[cfg(feature = "dbus")]
                    let mut _reservation: Option<DeviceReservation> =
                        if out_cfg.backend == Some(OutputBackend::Alsa) && out_cfg.reserve_device.unwrap_or(true) {
                            let card_idx = out_cfg
                                .device
                                .as_deref()
                                .and_then(resolve_card_index)
                                .unwrap_or(0);

                            match DeviceReservation::acquire(card_idx, "audigma", 100) {
                                Ok(res) => {
                                    tracing::info!(
                                        target: "audigma::daemon",
                                        event = audigma_log::DEVICE_LOCK,
                                        acquired = true,
                                        card = card_idx,
                                        name = res.well_known_name(),
                                        "device lock acquired via D-Bus org.freedesktop.ReserveDevice1"
                                    );
                                    Some(res)
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        target: "audigma::daemon",
                                        event = audigma_log::DEVICE_LOCK,
                                        acquired = false,
                                        card = card_idx,
                                        error = %e,
                                        "failed to acquire D-Bus reservation"
                                    );
                                    None
                                }
                            }
                        } else {
                            None
                        };

                    // Prepare and Run Live Pipeline
                    let mut pipeline =
                        LivePipeline::prepare_with_workers(settings.clone(), input_rate, Some(worker_setup.clone()))?;
                    // Worker pool persists across rate switches and device reconnects.
                    let mut dsp_session = DspSession::new();
                    let mut current_input_rate = input_rate;
                    let mut current_target_spec = target_spec;
                    let mut generation: u64 = 1;

                    let target_device = match out_cfg.backend {
                        Some(OutputBackend::Pipewire) => out_cfg
                            .target_node
                            .clone()
                            .filter(|n| !n.trim().is_empty())
                            .unwrap_or_else(|| "default".to_string()),
                        _ => out_cfg
                            .device
                            .clone()
                            .unwrap_or_else(|| "default".to_string()),
                    };
                    let presence = OutputPresence::new(out_cfg, &target_device);
                    let custom_mixer_control = out_cfg.mixer_control();
                    let vol_control_mode = out_cfg.volume_control();
                    let device_wait_ms = out_cfg.device_wait_ms();
                    let expiry = out_cfg.device_wait_expiry();
                    let keep_audio_sink_ms = out_cfg.keep_audio_sink_ms();

                    let mut hw_mixer: Option<AlsaHardwareMixer> = if out_cfg.backend == Some(OutputBackend::Alsa) {
                        match AlsaHardwareMixer::open(&target_device, custom_mixer_control) {
                            Ok(m) => {
                                tracing::info!(
                                    target: "audigma::alsa",
                                    event = "hardware_mixer_bound",
                                    control = m.element_name(),
                                    "bound to ALSA mixer control"
                                );
                                Some(m)
                            }
                            Err(e) => {
                                tracing::info!(
                                    target: "audigma::alsa",
                                    event = audigma_log::HARDWARE_MIXER_UNAVAILABLE,
                                    device = %target_device,
                                    error = %e,
                                    "hardware mixer unavailable"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };

                    let active_vol_mode = match vol_control_mode {
                        audigma_settings::VolumeControlMode::HardwareSoftwareFallback => {
                            if hw_mixer.is_some() {
                                DaemonVolumeMode::Hardware
                            } else {
                                DaemonVolumeMode::Software
                            }
                        }
                        audigma_settings::VolumeControlMode::Hardware => {
                            if hw_mixer.is_none() {
                                tracing::warn!(
                                    target: "audigma::daemon",
                                    event = audigma_log::VOLUME_MODE,
                                    "hardware volume requested but no mixer element found; digital gain locked at 0 dBFS"
                                );
                            }
                            DaemonVolumeMode::Hardware
                        }
                        audigma_settings::VolumeControlMode::Software => DaemonVolumeMode::Software,
                        audigma_settings::VolumeControlMode::None => DaemonVolumeMode::None,
                    };
                    control.set_volume_mode(active_vol_mode);
                    tracing::info!(
                        target: "audigma::daemon",
                        event = audigma_log::VOLUME_MODE,
                        active = %active_vol_mode,
                        configured = ?vol_control_mode,
                        "volume mode configured"
                    );

                    let mut current_output: Option<Box<dyn AudioOutput>> = None;
                let mut drain_bufs: Vec<Vec<f64>> = vec![vec![0.0; 256]; channels as usize];

                    // Resilient daemon loop
                    while control.is_active() {
                        // Step 1: Ensure output device is connected and opened
                        if current_output.is_none() {
                            if !presence.is_present() {
                                if device_wait_ms == 0 {
                                    if expiry == DeviceWaitExpiry::Pause {
                                        tracing::warn!(
                                            target: "audigma::daemon",
                                            event = "device_wait_timeout",
                                            device = %target_device,
                                            "output device absent and device_wait_ms = 0; pausing"
                                        );
                                        control.request_pause(PauseReason::DeviceWaitExpired);
                                        break;
                                    }
                                    return Err(EngineError::DeviceDisconnected(format!(
                                        "target output '{target_device}' is not present at startup"
                                    ))
                                    .into());
                                }

                                tracing::info!(
                                    target: "audigma::daemon",
                                    event = audigma_log::DEVICE_WAIT_TICK,
                                    device = %target_device,
                                    elapsed_s = 0,
                                    sink_active = true,
                                    device_wait_ms,
                                    keep_audio_sink_ms,
                                    "waiting for output device"
                                );
                                control.set_state(DaemonState::WaitingForDevice);
                                control.set_disconnect_info(Some(target_device.clone()), Some(0));

                                let wait_start = Instant::now();
                                let mut sink_dropped = false;
                                let mut last_log_secs = 0u64;

                                while control.is_active() {
                                    let elapsed_ms = wait_start.elapsed().as_millis() as u64;
                                    let elapsed_secs = elapsed_ms / 1000;
                                    control.set_disconnect_info(Some(target_device.clone()), Some(elapsed_ms));

                                    // Periodic waiting heartbeat every 5 seconds
                                    if elapsed_secs >= last_log_secs + 5 {
                                        last_log_secs = elapsed_secs;
                                        tracing::info!(
                                            target: "audigma::daemon",
                                            event = audigma_log::DEVICE_WAIT_TICK,
                                            device = %target_device,
                                            elapsed_s = elapsed_secs,
                                            sink_active = !sink_dropped,
                                            "still waiting for output device"
                                        );
                                    }

                                    // Check device_wait_ms timeout
                                    if device_wait_ms > 0 && elapsed_ms >= device_wait_ms as u64 {
                                        if expiry == DeviceWaitExpiry::Pause {
                                            tracing::warn!(
                                                target: "audigma::daemon",
                                                event = "device_wait_timeout",
                                                device = %target_device,
                                                elapsed_ms,
                                                timeout_ms = device_wait_ms,
                                                "device_wait_ms timeout reached; pausing"
                                            );
                                            control.request_pause(PauseReason::DeviceWaitExpired);
                                            break;
                                        }
                                        tracing::error!(
                                            target: "audigma::daemon",
                                            event = "device_wait_timeout",
                                            device = %target_device,
                                            elapsed_ms,
                                            timeout_ms = device_wait_ms,
                                            "device_wait_ms timeout reached; exiting"
                                        );
                                        return Err(EngineError::DeviceDisconnected(format!(
                                            "timed out waiting for device '{target_device}' after {device_wait_ms} ms"
                                        )).into());
                                    }

                                    // Check keep_audio_sink_ms grace period
                                    if !sink_dropped
                                        && keep_audio_sink_ms >= 0
                                        && elapsed_ms >= keep_audio_sink_ms as u64
                                    {
                                        tracing::warn!(
                                            target: "audigma::daemon",
                                            event = "keep_audio_sink_expired",
                                            device = %target_device,
                                            keep_audio_sink_ms,
                                            "keep_audio_sink_ms expired; unregistering virtual sink from PipeWire graph"
                                        );
                                        input_driver = None; // Drop virtual sink to unregister from PipeWire
                                        sink_dropped = true;
                                    }

                                    // If virtual sink is still registered, bit-bucket drain incoming frames pacingly
                                    if let Some(ref mut input) = input_driver {
                                        for buf in drain_bufs.iter_mut() {
                                buf.fill(0.0);
                            }
                            let mut refs: Vec<&mut [f64]> =
                                drain_bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
                            let _ = input.read_channels(&mut refs);
                                    }

                                    // Wait for inotify event or slice
                                    if presence.wait_present(200, &control.running)
                                    {
                                        tracing::info!(
                                            target: "audigma::daemon",
                                            event = "hotplug_event_detected",
                                            device = %target_device,
                                            "hotplug event detected: output device is present"
                                        );
                                        break;
                                    }
                                }

                                if !control.is_active() {
                                    break;
                                }

                                // Re-register input virtual sink if it was dropped
                                if input_driver.is_none() {
                                    tracing::info!(
                                        target: "audigma::daemon",
                                        event = "input_sink_rebind",
                                        "re-registering virtual sink in PipeWire graph"
                                    );
                                    input_driver = Some(create_input_driver(in_cfg)?);
                                }

                                // Re-acquire D-Bus device reservation if needed
                                #[cfg(feature = "dbus")]
                                if out_cfg.backend == Some(OutputBackend::Alsa)
                                    && out_cfg.reserve_device.unwrap_or(true)
                                {
                                    let card_idx = resolve_card_index(&target_device).unwrap_or(0);
                                    match DeviceReservation::acquire(card_idx, "audigma", 100) {
                                        Ok(res) => {
                                            tracing::info!(
                                                target: "audigma::daemon",
                                                event = audigma_log::DEVICE_LOCK,
                                                acquired = true,
                                                card = card_idx,
                                                name = res.well_known_name(),
                                                "re-acquired D-Bus device reservation"
                                            );
                                            _reservation = Some(res);
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                target: "audigma::daemon",
                                                event = audigma_log::DEVICE_LOCK,
                                                acquired = false,
                                                card = card_idx,
                                                error = %e,
                                                "failed to re-acquire D-Bus reservation"
                                            );
                                        }
                                    }
                                }
                            }

                            // Open output device
                            match create_output_driver(out_cfg, &current_target_spec, presence.pipewire_flag()) {
                                Ok(out) => {
                                    tracing::info!(
                                        target: "audigma::alsa",
                                        event = audigma_log::OUTPUT_DEVICE_OPENED,
                                        device = %target_device,
                                        backend = ?out_cfg.backend,
                                        "opened output device; resuming streaming"
                                    );
                                    current_output = Some(out);
                                    control.set_disconnect_info(None, None);

                                    // Re-bind ALSA hardware mixer if applicable
                                    if out_cfg.backend == Some(OutputBackend::Alsa) {
                                        hw_mixer =
                                            match AlsaHardwareMixer::open(&target_device, custom_mixer_control) {
                                                Ok(mut m) => {
                                                    tracing::info!(
                                                        target: "audigma::alsa",
                                                        event = "hardware_mixer_rebound",
                                                        control = m.element_name(),
                                                        "re-bound ALSA hardware mixer"
                                                    );
                                                    let cur_vol = control.get_volume();
                                                    let is_muted = control.is_muted();
                                                    let _ = m.set_volume_db(cur_vol);
                                                    let _ = m.set_mute(is_muted);
                                                    Some(m)
                                                }
                                                Err(_) => None,
                                            };
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        target: "audigma::alsa",
                                        event = "output_open_retry",
                                        device = %target_device,
                                        error = %e,
                                        "failed to open output device; retrying in 500ms"
                                    );
                                    std::thread::sleep(Duration::from_millis(500));
                                    continue;
                                }
                            }
                        }

                        // Ensure input driver is active
                        if input_driver.is_none() {
                            input_driver = Some(create_input_driver(in_cfg)?);
                        }

                        let input = input_driver.as_mut().unwrap();
                        let output = current_output.as_mut().unwrap();

                        // Run streaming loop
                        match pipeline.run_streaming_with_session(
                            &mut dsp_session,
                            &mut **input,
                            &mut **output,
                            control.clone(),
                            hw_mixer
                                .as_mut()
                                .map(|m| m as &mut dyn audigma_engine::HardwareVolumeBridge),
                        ) {
                            Ok(StreamExitReason::Stopped) => break,
                            Ok(StreamExitReason::RateChanged {
                                new_rate,
                                channels: _,
                            }) => {
                                let new_plan = settings.explain_for_input_rate(new_rate)?;
                                let new_target_spec = new_plan.stream_spec.target;

                                let need_reopen_output = match (current_target_spec, new_target_spec) {
                                    (
                                        TargetRateSpec::Dsd { bit_rate: old_br },
                                        TargetRateSpec::Dsd { bit_rate: new_br },
                                    ) => old_br != new_br,
                                    (
                                        TargetRateSpec::Pcm { rate: old_r, .. },
                                        TargetRateSpec::Pcm { rate: new_r, .. },
                                    ) => old_r != new_r,
                                    _ => true,
                                };

                                let (output_format, output_rate_hz) = match new_target_spec {
                                    TargetRateSpec::Dsd { bit_rate } => (DaemonOutputFormat::Dsd, bit_rate.get()),
                                    TargetRateSpec::Pcm { rate, .. } => (DaemonOutputFormat::Pcm, rate.get()),
                                };

                                tracing::info!(
                                    target: "audigma::clock",
                                    event = audigma_log::RATE_SWITCH,
                                    from_hz = current_input_rate.get(),
                                    to_hz = new_rate.get(),
                                    generation = generation + 1,
                                    reopened_output = need_reopen_output,
                                    target_format = ?output_format,
                                    "dynamic sample rate switch detected"
                                );
                                current_input_rate = new_rate;

                                if need_reopen_output {
                                    tracing::info!(
                                        target: "audigma::output",
                                        event = audigma_log::OUTPUT_REOPENED,
                                        device = %target_device,
                                        reason = "rate_switch",
                                        "re-opening output stream for new target rate"
                                    );
                                    current_output = None;
                                }

                                // Re-prepare the LivePipeline for the new input rate
                                pipeline = LivePipeline::prepare_with_workers(
                                    settings.clone(),
                                    new_rate,
                                    Some(worker_setup.clone()),
                                )?;
                                current_target_spec = new_target_spec;

                                // Bump generation and update active rate on virtual sink
                                generation += 1;
                                control.generation.store(generation, Ordering::SeqCst);
                                control.inc_rate_switch();
                                input.set_generation(generation);
                                input.set_active_rate(new_rate);
                                input.set_preroll_secs(target_preroll_secs);

                                // Update telemetry
                                control.set_stream_config(new_rate.get(), output_format, output_rate_hz);

                                continue;
                            }
                            Err(e)
                                if matches!(e, EngineError::DeviceDisconnected(_))
                                    || (matches!(e, EngineError::Io(_))
                                        && presence.pipewire_target_missing()) =>
                            {
                                let reason = match &e {
                                    EngineError::DeviceDisconnected(r) => r.clone(),
                                    other => other.to_string(),
                                };
                                tracing::warn!(
                                    target: "audigma::alsa",
                                    event = audigma_log::OUTPUT_DEVICE_DISCONNECTED,
                                    device = %target_device,
                                    error = %reason,
                                    "output device disconnected"
                                );
                                current_output = None; // Force output driver teardown
                                hw_mixer = None;
                                #[cfg(feature = "dbus")]
                                {
                                    tracing::info!(
                                        target: "audigma::daemon",
                                        event = audigma_log::DEVICE_LOCK,
                                        acquired = false,
                                        "releasing D-Bus reservation for disconnected device"
                                    );
                                    _reservation = None; // Release D-Bus reservation while device is gone
                                }
                                control.set_state(DaemonState::WaitingForDevice);
                                control.set_disconnect_info(Some(target_device.clone()), Some(0));
                            }

                            Err(e) => {
                                tracing::error!(
                                    target: "audigma::live",
                                    event = audigma_log::STREAMING_ERROR,
                                    error = %e,
                                    error_kind = "runtime",
                                    "runtime streaming error"
                                );
                                return Err(e.into());
                            }
                        }
                    }

                Ok(())
            })?;
        if let Ok(startup) = startup_rx.recv() {
            match startup {
                Ok(()) => {
                    tracing::debug!(target: "audigma::daemon", "audigma-loop placement ready")
                }
                Err(e) => {
                    tracing::debug!(target: "audigma::daemon", error = %e, "audigma-loop placement failed")
                }
            }
        }
        match handle.join() {
            Ok(res) => res,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    });

    loop_result
}
