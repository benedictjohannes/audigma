//! Handler for `audigma run` daemon subcommand.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use audigma_dsp::config::TargetRateSpec;
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::live::{
    DaemonControlHandle, DaemonOutputFormat, DaemonState, DaemonVolumeMode, LivePipeline,
};
use audigma_engine::traits::{AudioInput, AudioOutput, EngineError};
use audigma_engine::StreamExitReason;
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

use crate::ipc::{ControlServer, default_socket_path};
use super::resolve_config_path;

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Optional configuration file path (defaults to ~/.config/audigma/config.toml)
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,
}

pub fn run_run(args: RunArgs) -> Result<(), EngineError> {
    let config_path = resolve_config_path(args.config.as_deref());
    if !config_path.exists() {
        return Err(EngineError::Pipeline(format!(
            "configuration file not found at: {}",
            config_path.display()
        )));
    }

    let content = std::fs::read_to_string(&config_path)?;
    let settings = Settings::from_toml_str(&content)?;
    settings.validate()?;

    let in_cfg = settings
        .input
        .as_ref()
        .ok_or_else(|| EngineError::Pipeline("missing [input] section in config".to_string()))?;
    let out_cfg = settings
        .output
        .as_ref()
        .ok_or_else(|| EngineError::Pipeline("missing [output] section in config".to_string()))?;

    let input_rate = in_cfg.rate_hz.unwrap_or(PcmRateHz::PCM_48000);
    let channels = in_cfg.channels.unwrap_or(2);

    let plan = settings.explain_for_input_rate(input_rate)?;
    let target_spec = plan.stream_spec.target;

    eprintln!("audigma: starting live daemon");
    eprintln!("  Configuration: {}", config_path.display());
    eprintln!(
        "  Input:         {:?} ({} Hz, {} channels)",
        in_cfg.backend,
        input_rate.get(),
        channels
    );
    eprintln!("  Output:        {:?}", out_cfg.backend);
    eprintln!("  Preset:        {:?}", plan.dsp_config.preset);

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
                    eprintln!(
                        "  DeviceLock:    Acquired {} via D-Bus org.freedesktop.ReserveDevice1",
                        res.well_known_name()
                    );
                    Some(res)
                }
                Err(e) => {
                    eprintln!("  DeviceLock:    Warning: failed to acquire D-Bus reservation: {e}");
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
            Ok(acq) => eprintln!("  RT Priority:   Acquired real-time priority via {acq}"),
            Err(e) => eprintln!("  RT Priority:   Request skipped/fallback: {e}"),
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
                            .map_err(|e| {
                                EngineError::Pipeline(format!(
                                    "failed to bind PipeWire virtual sink: {e}"
                                ))
                            })?;
                    sink.set_preroll_secs(target_preroll_secs);
                    Ok(Box::new(sink))
                }
                InputBackend::Alsa => Err(EngineError::Pipeline(
                    "ALSA live capture input not yet configured".to_string(),
                )),
                InputBackend::File | InputBackend::Stdio => Err(EngineError::Pipeline(
                    "File/Stdio backend is for offline mode; use 'audigma convert'".to_string(),
                )),
                #[cfg(not(feature = "pipewire"))]
                InputBackend::Pipewire => Err(EngineError::Pipeline(
                    "PipeWire feature is disabled in this build".to_string(),
                )),
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
                        EngineError::Pipeline(format!("failed to configure ALSA output: {e}"))
                    })?;
                let alsa_out = AlsaAudioOutput::open(alsa_config).map_err(|e| {
                    EngineError::Pipeline(format!("failed to open ALSA output: {e}"))
                })?;
                Ok(Box::new(alsa_out))
            }
            #[cfg(feature = "pipewire")]
            OutputBackend::Pipewire => {
                let target_node = out_cfg.target_node.as_deref();
                let pw_rate = match target {
                    TargetRateSpec::Pcm { rate, .. } => rate.get(),
                    TargetRateSpec::Dsd { bit_rate } => {
                        return Err(EngineError::Pipeline(format!(
                            "PipeWire PCM output cannot carry DSD bit rate {bit_rate}"
                        )));
                    }
                };
                let pw_out = PipeWirePcmOutput::new(
                    "audigma_pcm_out",
                    target_node,
                    pw_rate,
                    channels as u32,
                    32,
                )
                .map_err(|e| {
                    EngineError::Pipeline(format!("failed to initialize PipeWire output: {e}"))
                })?;
                Ok(Box::new(pw_out))
            }
            #[cfg(not(feature = "pipewire"))]
            OutputBackend::Pipewire => Err(EngineError::Pipeline(
                "PipeWire feature is disabled in this build".to_string(),
            )),
            OutputBackend::File | OutputBackend::Stdio => Err(EngineError::Pipeline(
                "File/Stdio backend is for offline mode; use 'audigma convert'".to_string(),
            )),
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

    // Start Unix Control Socket
    let socket_path = default_socket_path();
    let _server = ControlServer::start(socket_path.clone(), control.clone())
        .map_err(|e| EngineError::Pipeline(format!("failed to start control socket: {e}")))?;
    eprintln!("  ControlSocket: {}", socket_path.display());

    // Real-time thread priority hook for channel workers
    #[cfg(feature = "dbus")]
    let worker_rt_hook: Option<Arc<dyn Fn(usize) + Send + Sync>> =
        Some(Arc::new(|ch: usize| match request_realtime_priority(10) {
            Ok(acq) => eprintln!("  Worker {ch} RT:   Acquired real-time priority via {acq}"),
            Err(e) => eprintln!("  audigma-ch{ch}: RT priority request skipped/fallback: {e}"),
        }));
    #[cfg(not(feature = "dbus"))]
    let worker_rt_hook: Option<Arc<dyn Fn(usize) + Send + Sync>> = None;

    // Prepare and Run Live Pipeline
    let mut pipeline =
        LivePipeline::prepare_with_hook(settings.clone(), input_rate, worker_rt_hook.clone())?;
    let mut current_input_rate = input_rate;
    let mut current_target_spec = target_spec;
    let mut generation: u64 = 1;
    eprintln!("audigma: daemon running. Press Ctrl+C or run 'audigma stop' to exit.");

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
                eprintln!(
                    "  HardwareMixer: Bound to ALSA mixer control '{}'",
                    m.element_name()
                );
                Some(m)
            }
            Err(e) => {
                eprintln!("  HardwareMixer: Note: hardware mixer unavailable ({e})");
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
                eprintln!(
                    "  VolumeControl: Warning: 'hardware' volume requested but no mixer element found; digital gain locked at 0 dBFS"
                );
            }
            DaemonVolumeMode::Hardware
        }
        audigma_settings::VolumeControlMode::Software => DaemonVolumeMode::Software,
        audigma_settings::VolumeControlMode::None => DaemonVolumeMode::None,
    };
    control.set_volume_mode(active_vol_mode);
    eprintln!(
        "  VolumeControl: {} (configured: {:?})",
        active_vol_mode, vol_control_mode
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
                    )));
                }

                eprintln!(
                    "[INFO] audigma: waiting for output device '{target_device}' (device_wait_ms: {device_wait_ms}, keep_audio_sink_ms: {keep_audio_sink_ms})..."
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
                        eprintln!(
                            "[INFO] audigma: still waiting for '{target_device}'... ({elapsed_secs}s elapsed, sink_active: {})",
                            !sink_dropped
                        );
                    }

                    // Check device_wait_ms timeout
                    if device_wait_ms > 0 && elapsed_ms >= device_wait_ms as u64 {
                        eprintln!(
                            "[ERROR] audigma: device_wait_ms timeout ({device_wait_ms} ms) reached; exiting."
                        );
                        return Err(EngineError::DeviceDisconnected(format!(
                            "timed out waiting for device '{target_device}' after {device_wait_ms} ms"
                        )));
                    }

                    // Check keep_audio_sink_ms grace period
                    if !sink_dropped
                        && keep_audio_sink_ms >= 0
                        && elapsed_ms >= keep_audio_sink_ms as u64
                    {
                        eprintln!(
                            "[WARN] audigma: keep_audio_sink_ms ({keep_audio_sink_ms} ms) expired; unregistering virtual sink from PipeWire graph."
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
                        eprintln!(
                            "[INFO] audigma: hotplug event detected: output device '{target_device}' is present!"
                        );
                        break;
                    }
                }

                if !control.is_running() {
                    break;
                }

                // Re-register input virtual sink if it was dropped
                if input_driver.is_none() {
                    eprintln!("[INFO] audigma: re-registering virtual sink in PipeWire graph...");
                    input_driver = Some(create_input_driver(in_cfg)?);
                }

                // Re-acquire D-Bus device reservation if needed
                #[cfg(feature = "dbus")]
                if out_cfg.reserve_device.unwrap_or(true) {
                    let card_idx = resolve_card_index(&target_device).unwrap_or(0);
                    eprintln!(
                        "[INFO] audigma: re-acquiring D-Bus reservation for card {card_idx}..."
                    );
                    match DeviceReservation::acquire(card_idx, "audigma", 100) {
                        Ok(res) => {
                            eprintln!(
                                "[INFO] audigma: acquired {} via D-Bus org.freedesktop.ReserveDevice1",
                                res.well_known_name()
                            );
                            _reservation = Some(res);
                        }
                        Err(e) => {
                            eprintln!(
                                "[WARN] audigma: failed to re-acquire D-Bus reservation: {e}"
                            );
                        }
                    }
                }
            }

            // Open output device
            match create_output_driver(out_cfg, &current_target_spec) {
                Ok(out) => {
                    eprintln!(
                        "[INFO] audigma: opened output device '{target_device}'; resuming streaming."
                    );
                    current_output = Some(out);
                    control.set_disconnect_info(None, None);

                    // Re-bind ALSA hardware mixer if applicable
                    if out_cfg.backend == OutputBackend::Alsa {
                        hw_mixer =
                            match AlsaHardwareMixer::open(&target_device, custom_mixer_control) {
                                Ok(mut m) => {
                                    eprintln!(
                                        "[INFO] audigma: re-bound ALSA hardware mixer '{}'",
                                        m.element_name()
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
                    eprintln!(
                        "[WARN] audigma: failed to open output device '{target_device}': {e}; retrying in 500ms..."
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
                eprintln!(
                    "[INFO] audigma: dynamic sample rate switch detected: {} Hz -> {} Hz",
                    current_input_rate, new_rate
                );
                current_input_rate = new_rate;

                // Check if output rate or ALSA configuration must change
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

                if need_reopen_output && out_cfg.backend == OutputBackend::Alsa {
                    eprintln!("[INFO] audigma: re-locking ALSA device for new target rate...");
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
                // Re-assert the preroll cushion in case the output buffer was
                // reconfigured for the new target rate.
                input.set_preroll_secs(target_preroll_secs);

                // Update telemetry
                let (output_format, output_rate_hz) = match new_target_spec {
                    TargetRateSpec::Dsd { bit_rate } => (DaemonOutputFormat::Dsd, bit_rate.get()),
                    TargetRateSpec::Pcm { rate, .. } => (DaemonOutputFormat::Pcm, rate.get()),
                };
                control.set_stream_config(new_rate.get(), output_format, output_rate_hz);

                continue;
            }
            Err(EngineError::DeviceDisconnected(reason)) => {
                eprintln!("[WARN] audigma: output device disconnected: {reason}");
                current_output = None; // Force output driver teardown
                hw_mixer = None;
                #[cfg(feature = "dbus")]
                {
                    eprintln!(
                        "[INFO] audigma: releasing D-Bus reservation for disconnected device."
                    );
                    _reservation = None; // Release D-Bus reservation while device is gone
                }
                control.set_state(DaemonState::WaitingForDevice);
                control.set_disconnect_info(Some(target_device.clone()), Some(0));
            }

            Err(e) => {
                eprintln!("[ERROR] audigma: runtime streaming error: {e}");
                return Err(e);
            }
        }
    }

    eprintln!("audigma: daemon stopped cleanly.");
    Ok(())
}
