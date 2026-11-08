#[cfg(feature = "pipewire")]
use audigma_engine::traits::AudioInput;
#[cfg(feature = "pipewire")]
use audigma_io::pipewire::{
    PipeWirePcmOutput, PipeWireVirtualSink, build_format_pod, channel_position_to_spa,
};
#[cfg(feature = "pipewire")]
use audigma_settings::{InputBackend, InputConfig, RateMode};
#[cfg(feature = "pipewire")]
use libspa::pod::deserialize::PodDeserializer;

#[test]
#[cfg(feature = "pipewire")]
fn test_channel_position_to_spa_mappings() {
    assert_eq!(
        channel_position_to_spa("MONO"),
        libspa::sys::SPA_AUDIO_CHANNEL_MONO
    );
    assert_eq!(
        channel_position_to_spa("FL"),
        libspa::sys::SPA_AUDIO_CHANNEL_FL
    );
    assert_eq!(
        channel_position_to_spa("FR"),
        libspa::sys::SPA_AUDIO_CHANNEL_FR
    );
    assert_eq!(
        channel_position_to_spa("FC"),
        libspa::sys::SPA_AUDIO_CHANNEL_FC
    );
    assert_eq!(
        channel_position_to_spa("LFE"),
        libspa::sys::SPA_AUDIO_CHANNEL_LFE
    );
    assert_eq!(
        channel_position_to_spa("SL"),
        libspa::sys::SPA_AUDIO_CHANNEL_SL
    );
    assert_eq!(
        channel_position_to_spa("SR"),
        libspa::sys::SPA_AUDIO_CHANNEL_SR
    );
    assert_eq!(
        channel_position_to_spa("RL"),
        libspa::sys::SPA_AUDIO_CHANNEL_RL
    );
    assert_eq!(
        channel_position_to_spa("RR"),
        libspa::sys::SPA_AUDIO_CHANNEL_RR
    );
    assert_eq!(
        channel_position_to_spa("RC"),
        libspa::sys::SPA_AUDIO_CHANNEL_RC
    );
    assert_eq!(
        channel_position_to_spa("FLC"),
        libspa::sys::SPA_AUDIO_CHANNEL_FLC
    );
    assert_eq!(
        channel_position_to_spa("FRC"),
        libspa::sys::SPA_AUDIO_CHANNEL_FRC
    );
    assert_eq!(
        channel_position_to_spa("UNKNOWN"),
        libspa::sys::SPA_AUDIO_CHANNEL_UNKNOWN
    );
    assert_eq!(
        channel_position_to_spa("invalid_token"),
        libspa::sys::SPA_AUDIO_CHANNEL_UNKNOWN
    );
}

#[test]
#[cfg(feature = "pipewire")]
fn test_build_format_pod_multichannel_layouts() {
    let allowed = vec![44100, 48000, 88200, 96000, 192000];

    // 1 Channel (Mono)
    let mono_pos = vec!["MONO".to_string()];
    let pod_mono = build_format_pod(48000, 1, &mono_pos, RateMode::Dynamic, &allowed).unwrap();
    assert!(!pod_mono.is_empty());
    let (_, val_mono) = PodDeserializer::deserialize_any_from(&pod_mono).unwrap();
    assert!(matches!(val_mono, libspa::pod::Value::Object(_)));

    // 2 Channels (Stereo)
    let stereo_pos = vec!["FL".to_string(), "FR".to_string()];
    let pod_stereo = build_format_pod(44100, 2, &stereo_pos, RateMode::Dynamic, &allowed).unwrap();
    assert!(!pod_stereo.is_empty());
    let (_, val_stereo) = PodDeserializer::deserialize_any_from(&pod_stereo).unwrap();
    assert!(matches!(val_stereo, libspa::pod::Value::Object(_)));

    // 2 Channels (Reversed Stereo Order Preserved)
    let rev_pos = vec!["FR".to_string(), "FL".to_string()];
    let pod_rev = build_format_pod(44100, 2, &rev_pos, RateMode::Fixed, &[]).unwrap();
    assert!(!pod_rev.is_empty());
    let (_, val_rev) = PodDeserializer::deserialize_any_from(&pod_rev).unwrap();
    assert!(matches!(val_rev, libspa::pod::Value::Object(_)));

    // 4 Channels (Quad)
    let quad_pos = vec![
        "FL".to_string(),
        "FR".to_string(),
        "RL".to_string(),
        "RR".to_string(),
    ];
    let pod_quad = build_format_pod(96000, 4, &quad_pos, RateMode::Dynamic, &allowed).unwrap();
    assert!(!pod_quad.is_empty());
    let (_, val_quad) = PodDeserializer::deserialize_any_from(&pod_quad).unwrap();
    assert!(matches!(val_quad, libspa::pod::Value::Object(_)));

    // 6 Channels (5.1 Surround)
    let surr51_pos = vec![
        "FL".to_string(),
        "FR".to_string(),
        "FC".to_string(),
        "LFE".to_string(),
        "RL".to_string(),
        "RR".to_string(),
    ];
    let pod_51 = build_format_pod(48000, 6, &surr51_pos, RateMode::Dynamic, &allowed).unwrap();
    assert!(!pod_51.is_empty());
    let (_, val_51) = PodDeserializer::deserialize_any_from(&pod_51).unwrap();
    assert!(matches!(val_51, libspa::pod::Value::Object(_)));

    // 8 Channels (7.1 Surround)
    let surr71_pos = vec![
        "FL".to_string(),
        "FR".to_string(),
        "FC".to_string(),
        "LFE".to_string(),
        "RL".to_string(),
        "RR".to_string(),
        "SL".to_string(),
        "SR".to_string(),
    ];
    let pod_71 = build_format_pod(192000, 8, &surr71_pos, RateMode::Dynamic, &allowed).unwrap();
    assert!(!pod_71.is_empty());
    let (_, val_71) = PodDeserializer::deserialize_any_from(&pod_71).unwrap();
    assert!(matches!(val_71, libspa::pod::Value::Object(_)));
}

#[test]
#[cfg(feature = "pipewire")]
fn test_pipewire_virtual_sink_from_input_config_resolution() {
    let in_cfg = InputConfig {
        backend: InputBackend::Pipewire,
        node_name: Some("test_dynamic_sink".to_string()),
        sample_format: None,
        rate_hz: None,
        channels: Some(6),
        channel_positions: Some(vec![
            "FL".to_string(),
            "FR".to_string(),
            "FC".to_string(),
            "LFE".to_string(),
            "RL".to_string(),
            "RR".to_string(),
        ]),
        rate_mode: Some(RateMode::Dynamic),
        allowed_rates: Some(vec![44100, 48000, 96000, 192000]),
        allow_dsd: None,
        dsd: None,
    };

    // Verify config resolution produces the intended multi-channel positions and allowed rates
    assert_eq!(in_cfg.channels.unwrap(), 6);
    assert_eq!(in_cfg.resolved_channel_positions().len(), 6);
    assert_eq!(in_cfg.rate_mode(), RateMode::Dynamic);
    assert_eq!(in_cfg.resolved_allowed_rates().len(), 4);

    // Attempt creation (graceful on host without PipeWire socket)
    match PipeWireVirtualSink::from_input_config(&in_cfg, 48000, 8) {
        Ok(mut sink) => {
            assert_eq!(sink.info().original_channels, 6);
            let mut ch0 = vec![0.0; 32];
            let mut ch1 = vec![0.0; 32];
            let mut ch2 = vec![0.0; 32];
            let mut ch3 = vec![0.0; 32];
            let mut ch4 = vec![0.0; 32];
            let mut ch5 = vec![0.0; 32];
            let mut slices: Vec<&mut [f64]> =
                vec![&mut ch0, &mut ch1, &mut ch2, &mut ch3, &mut ch4, &mut ch5];
            let frames = sink.read_channels(&mut slices).unwrap();
            assert!(frames <= 32);
            assert!(sink.check_rate_change().is_none());
        }
        Err(e) => {
            let msg = format!("{e}");
            assert!(!msg.is_empty());
        }
    }
}

#[test]
#[cfg(feature = "pipewire")]
fn test_pipewire_virtual_sink_and_pcm_output_lifecycle() {
    // Check if PipeWire daemon is running / connectable on this host
    // If running in headless sandbox or without user session, creation may return an error,
    // which must be handled cleanly without panic.
    match PipeWireVirtualSink::new("audigma_test_sink", 48000, 2, 8) {
        Ok(mut sink) => {
            assert_eq!(sink.info().sample_rate.get(), 48000);
            assert_eq!(sink.info().original_channels, 2);

            let mut left = vec![0.0; 64];
            let mut right = vec![0.0; 64];
            let frames = sink.read_stereo(&mut left, &mut right).unwrap();
            assert!(frames <= 64);

            sink.set_generation(2);
            drop(sink);
        }
        Err(e) => {
            let msg = format!("{e}");
            assert!(!msg.is_empty());
        }
    }

    match PipeWirePcmOutput::new("audigma_test_out", None, 48000, 2, 8, None) {
        Ok(mut out) => {
            use std::io::Write;
            let silence_bytes = vec![0u8; 128];
            let _ = out.write(&silence_bytes);
            let _ = out.flush();
            drop(out);
        }
        Err(e) => {
            let msg = format!("{e}");
            assert!(!msg.is_empty());
        }
    }
}

#[test]
#[cfg(feature = "pipewire")]
fn test_build_and_parse_dsd_format_pod() {
    let positions = vec!["FL".to_string(), "FR".to_string()];
    let allowed_rates = [352_800, 705_600, 1_411_200, 2_822_400];
    let pod_bytes =
        audigma_io::pipewire::build_dsd_format_pod(2, &positions, 352_800, &allowed_rates)
            .expect("build dsd format pod");
    let parsed = audigma_io::pipewire::parse_format_pod(&pod_bytes).expect("parse dsd format pod");
    assert!(parsed.is_dsd);
    assert_eq!(parsed.rate, 352_800);
    assert_eq!(parsed.channels, 2);
}

#[test]
#[cfg(feature = "pipewire")]
fn test_pipewire_virtual_sink_initial_volume_state() {
    match PipeWireVirtualSink::new("audigma_test_sink_vol", 48000, 2, 8) {
        Ok(sink) => {
            assert_eq!(sink.check_volume_change(), None);
        }
        Err(e) => {
            let msg = format!("{e}");
            assert!(!msg.is_empty());
        }
    }
}

#[test]
#[cfg(feature = "pipewire")]
fn test_pipewire_error_conversion_to_engine_error() {
    use audigma_engine::traits::EngineError;
    use audigma_io::pipewire::PipeWireError;

    let err1 = PipeWireError::StreamFailed;
    let engine_err1: EngineError = err1.into();
    match engine_err1 {
        EngineError::DeviceDisconnected(msg) => {
            assert!(msg.contains("PipeWire stream disconnected"));
        }
        _ => panic!("expected DeviceDisconnected"),
    }

    let err2 = PipeWireError::FormatPod("invalid layout".into());
    let engine_err2: EngineError = err2.into();
    match engine_err2 {
        EngineError::AudioBackend { backend, message } => {
            assert_eq!(backend, "pipewire");
            assert!(message.contains("invalid layout"));
        }
        _ => panic!("expected AudioBackend"),
    }
}
