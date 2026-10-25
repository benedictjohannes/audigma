//! Handler for `audigma run` daemon subcommand.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::{Duration, Instant};

use audigma_dsp::config::TargetRateSpec;
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::StreamExitReason;
use audigma_engine::live_pipeline::{
    DaemonControlHandle, DaemonOutputFormat, DaemonState, DaemonVolumeMode, LivePipeline,
};
use audigma_engine::traits::{AudioInput, AudioOutput, EngineError};
use audigma_io::alsa::{
    AlsaAudioOutput, AlsaHardwareMixer, AlsaOutputConfig, is_alsa_device_present,
    resolve_card_index, wait_for_alsa_device,
};
#[cfg(feature = "dbus")]
use audigma_io::dbus::{DeviceReservation, request_realtime_priority};
#[cfg(feature = "pipewire")]
use audigma_io::pipewire::{PipeWirePcmOutput, PipeWireVirtualSink};
use audigma_settings::{InputBackend, OutputBackend, Settings};
use clap::Args;

use super::resolve_config_path;
use crate::error::CliError;
use crate::ipc::{ControlServer, default_socket_path};

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
    /// Optional configuration file path (defaults to ~/.config/audigma/config.toml)
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,
}

pub fn run_run(args: RunArgs) -> Result<(), CliError> {
    let daemon_start_time = Instant::now();
    let config_path = resolve_config_path(args.config.as_deref());
    if !config_path.exists() {
        return Err(CliError::ConfigNotFound { path: config_path });
    }

    let content = std::fs::read_to_string(&config_path)?;
    let settings = Settings::from_toml_str(&content)?;
    settings.validate()?;

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

    let plan = settings.explain_for_input_rate(input_rate)?;
    let target_spec = plan.stream_spec.target;

    tracing::info!(
        target: "audigma::daemon",
        event = audigma_log::DAEMON_START,
        config = %config_path.display(),
        input_backend = ?in_cfg.backend,
        input_rate_hz = input_rate.get(),
        channels,
        output_backend = ?out_cfg.backend,
        preset = ?plan.dsp_config.preset,
        "starting live daemon"
    );

    // D-Bus device reservation if exclusive ALSA output is selected
    #[cfg(feature = "dbus")]
    let mut _reservation: Option<DeviceReservation> =
        if out_cfg.backend == OutputBackend::Alsa && out_cfg.reserve_device.unwrap_or(true) {
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

    // Real-time thread priority
    #[cfg(feature = "dbus")]
    {
        match request_realtime_priority(10) {
            Ok(acq) => tracing::info!(
                target: "audigma::daemon",
                event = audigma_log::RT_PRIORITY,
                acquired = true,
                priority = 10,
                method = %acq,
                "real-time priority acquired"
            ),
            Err(e) => tracing::warn!(
                target: "audigma::daemon",
                event = audigma_log::RT_PRIORITY,
                acquired = false,
                priority = 10,
                error = %e,
                "real-time priority request skipped/fallback"
            ),
        }
    }

    // Set up Input Backend helper closure
    let out_buf_ms = out_cfg.buffer_ms.unwrap_or(40);
    let target_preroll_secs = (out_buf_ms as f64 / 1000.0 + 0.050).clamp(0.060, 0.500);

    let create_input_driver =
        |in_cfg: &audigma_settings::InputConfig| -> Result<Box<dyn AudioInput>, EngineError> {
            match in_cfg.backend {
                #[cfg(feature = "pipewire")]
                InputBackend::Pipewire => {
                    let mut sink =
                        PipeWireVirtualSink::from_input_config(in_cfg, input_rate.get(), 128)
                            .map_err(|e| EngineError::AudioBackend {
                                backend: "pipewire",
                                message: format!("failed to bind PipeWire virtual sink: {e}"),
                            })?;
                    sink.set_preroll_secs(target_preroll_secs);
                    Ok(Box::new(sink))
                }
                InputBackend::Alsa => Err(EngineError::AudioBackend {
                    backend: "alsa",
                    message: "ALSA live capture input not yet configured".to_string(),
                }),
                InputBackend::File | InputBackend::Stdio => Err(EngineError::InvalidTopology {
                    message: "File/Stdio backend is for offline mode; use 'audigma convert'"
                        .to_string(),
                }),
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
                                target: &TargetRateSpec|
     -> Result<Box<dyn AudioOutput>, EngineError> {
        match out_cfg.backend {
            OutputBackend::Alsa => {
                let alsa_config =
                    AlsaOutputConfig::from_settings(&settings, target).map_err(|e| {
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
            OutputBackend::Pipewire => {
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
                let pw_out = PipeWirePcmOutput::new(
                    "audigma_pcm_out",
                    target_node,
                    pw_rate,
                    channels as u32,
                    32,
                )
                .map_err(|e| EngineError::AudioBackend {
                    backend: "pipewire",
                    message: format!("failed to initialize PipeWire output: {e}"),
                })?;
                Ok(Box::new(pw_out))
            }
            #[cfg(not(feature = "pipewire"))]
            OutputBackend::Pipewire => Err(EngineError::AudioBackend {
                backend: "pipewire",
                message: "PipeWire feature is disabled in this build".to_string(),
            }),
            OutputBackend::File | OutputBackend::Stdio => Err(EngineError::InvalidTopology {
                message: "File/Stdio backend is for offline mode; use 'audigma convert'"
                    .to_string(),
            }),
        }
    };

    let (out_fmt, out_rate_hz) = match target_spec {
        TargetRateSpec::Dsd { bit_rate } => (DaemonOutputFormat::Dsd, bit_rate.get()),
        TargetRateSpec::Pcm { rate, .. } => (DaemonOutputFormat::Pcm, rate.get()),
    };

    let control = DaemonControlHandle::new(
        input_rate.get(),
        out_fmt,
        out_rate_hz,
        match plan.dsp_config.preset {
            audigma_dsp::config::FilterPreset::Balanced => "balanced",
            audigma_dsp::config::FilterPreset::Reference => "reference",
            audigma_dsp::config::FilterPreset::LowLatency => "low-latency",
        },
        plan.dsp_config.preamp_db,
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

    // Start Unix Control Socket
    let socket_path = default_socket_path();
    let _server =
        ControlServer::start(socket_path.clone(), control.clone()).map_err(CliError::Ipc)?;
    tracing::info!(
        target: "audigma::ipc",
        event = audigma_log::CONTROL_SOCKET_BOUND,
        path = %socket_path.display(),
        "control socket bound"
    );

    // Real-time thread priority hook for channel workers
    #[cfg(feature = "dbus")]
    let worker_rt_hook: Option<Arc<dyn Fn(usize) + Send + Sync>> =
        Some(Arc::new(|ch: usize| match request_realtime_priority(10) {
            Ok(acq) => tracing::info!(
                target: "audigma::daemon",
                event = audigma_log::RT_PRIORITY,
                worker = ch,
                acquired = true,
                priority = 10,
                method = %acq,
                "worker acquired real-time priority"
            ),
            Err(e) => tracing::warn!(
                target: "audigma::daemon",
                event = audigma_log::RT_PRIORITY,
                worker = ch,
                acquired = false,
                priority = 10,
                error = %e,
                "worker real-time priority request skipped/fallback"
            ),
        }));
    #[cfg(not(feature = "dbus"))]
    let worker_rt_hook: Option<Arc<dyn Fn(usize) + Send + Sync>> = None;

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

    // Prepare and Run Live Pipeline
    let mut pipeline =
        LivePipeline::prepare_with_hook(settings.clone(), input_rate, worker_rt_hook.clone())?;
    let mut current_input_rate = input_rate;
    let mut current_target_spec = target_spec;
    let mut generation: u64 = 1;

    let target_device = out_cfg
        .device
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let custom_mixer_control = out_cfg.mixer_control();
    let vol_control_mode = out_cfg.volume_control();
    let device_wait_ms = out_cfg.device_wait_ms();
    let keep_audio_sink_ms = out_cfg.keep_audio_sink_ms();

    let mut hw_mixer: Option<AlsaHardwareMixer> = if out_cfg.backend == OutputBackend::Alsa {
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

    // Resilient daemon loop
    while control.is_running() {
        // Step 1: Ensure output device is connected and opened
        if current_output.is_none() {
            if out_cfg.backend == OutputBackend::Alsa && !is_alsa_device_present(&target_device) {
                if device_wait_ms == 0 {
                    return Err(EngineError::DeviceDisconnected(format!(
                        "target ALSA device '{target_device}' is not present at startup"
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

                while control.is_running() {
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
                        let mut drain_left = [0.0; 256];
                        let mut drain_right = [0.0; 256];
                        let _ = input.read_stereo(&mut drain_left, &mut drain_right);
                    }

                    // Wait for inotify event or slice
                    if wait_for_alsa_device(&target_device, 200, &control.running).unwrap_or(false)
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

                if !control.is_running() {
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
                if out_cfg.reserve_device.unwrap_or(true) {
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
            match create_output_driver(out_cfg, &current_target_spec) {
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
                    if out_cfg.backend == OutputBackend::Alsa {
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
        match pipeline.run_streaming_with_bridge(
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

                if need_reopen_output && out_cfg.backend == OutputBackend::Alsa {
                    tracing::info!(
                        target: "audigma::alsa",
                        event = audigma_log::OUTPUT_REOPENED,
                        device = %target_device,
                        reason = "rate_switch",
                        "re-locking ALSA device for new target rate"
                    );
                    current_output = None;
                }

                // Re-prepare the LivePipeline for the new input rate
                pipeline = LivePipeline::prepare_with_hook(
                    settings.clone(),
                    new_rate,
                    worker_rt_hook.clone(),
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
            Err(EngineError::DeviceDisconnected(reason)) => {
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

    // Unregister signal handler
    DAEMON_RUNNING_PTR.store(std::ptr::null_mut(), Ordering::SeqCst);
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
    }

    tracing::info!(
        target: "audigma::daemon",
        event = audigma_log::DAEMON_STOP,
        reason = "clean",
        uptime_s = daemon_start_time.elapsed().as_secs(),
        "daemon stopped cleanly"
    );

    Ok(())
}
