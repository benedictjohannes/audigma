//! Parity test suite ensuring `OfflinePipeline` and `LivePipeline` share identical rate planning,
//! stage resolution, gain staging, carrier format derivation, and passthrough eligibility.

use audigma_dsp::config::{Apodization, FilterPhase, OverloadPolicy, TargetRateSpec};
use audigma_dsp::rates::{DsdBitRateHz, PcmRateHz};
use audigma_engine::pipeline_topology::is_dsd_passthrough_eligible;
use audigma_engine::{IdleCarrierFormat, LivePipeline, OfflinePipeline};
use audigma_format::decoder::AudioStreamInfo;
use audigma_settings::Settings;

fn make_test_settings(target_spec_str: &str, transport: Option<&str>) -> Settings {
    let transport_line = match transport {
        Some(t) => format!("transport = \"{t}\""),
        None => String::new(),
    };
    let toml_str = format!(
        r#"
schema_version = 1
[target]
{target_spec_str}

[gain]
preamp_db = -3.0
overload_policy = "mute-recover"

[resampler]
preset = "balanced"

[output]
backend = "alsa"
container = "raw"
{transport_line}
"#
    );
    Settings::from_toml_str(&toml_str).expect("valid settings toml")
}

#[test]
fn test_parity_dsd_rate_families_and_cascade_stages() {
    let test_cases = [
        // (input_rate, target_toml, expected_stages, expected_intermediate)
        (
            PcmRateHz::PCM_44100,
            "format = \"dsd\"\ndsd_rate = \"dsd256\"\nrate_family = \"source\"",
            8, // 256 = 2^8
            PcmRateHz::PCM_44100,
        ),
        (
            PcmRateHz::PCM_88200,
            "format = \"dsd\"\ndsd_rate = \"dsd256\"\nrate_family = \"source\"",
            7, // 128 = 2^7
            PcmRateHz::PCM_88200,
        ),
        (
            PcmRateHz::PCM_48000,
            "format = \"dsd\"\ndsd_rate = \"dsd64\"\nrate_family = \"source\"",
            6, // 64 = 2^6
            PcmRateHz::PCM_48000,
        ),
        (
            PcmRateHz::PCM_192000,
            "format = \"dsd\"\ndsd_rate = \"dsd256\"\nrate_family = \"44100\"",
            6, // 11.2896M / 176.4k = 64 = 2^6
            PcmRateHz::PCM_176400,
        ),
        (
            PcmRateHz::PCM_96000,
            "format = \"dsd\"\ndsd_rate = \"dsd128\"\nrate_family = \"44100\"",
            6, // 5.6448M / 88.2k = 64 = 2^6
            PcmRateHz::PCM_88200,
        ),
    ];

    for (in_rate, target_toml, expected_stages, expected_intermediate) in test_cases {
        let settings = make_test_settings(target_toml, Some("native"));

        let offline = OfflinePipeline::prepare(settings.clone(), in_rate)
            .unwrap_or_else(|e| panic!("OfflinePipeline prepare failed for {in_rate:?}: {e}"));
        let live = LivePipeline::prepare(settings, in_rate)
            .unwrap_or_else(|e| panic!("LivePipeline prepare failed for {in_rate:?}: {e}"));

        // Topology parity
        assert_eq!(
            offline.topology().intermediate_rate,
            live.topology().intermediate_rate,
            "Intermediate rate mismatch for {in_rate:?}"
        );
        assert_eq!(
            offline.topology().intermediate_rate,
            expected_intermediate,
            "Intermediate rate unexpected for {in_rate:?}"
        );
        assert_eq!(
            offline.topology().cascade_stages,
            live.topology().cascade_stages,
            "Cascade stages mismatch for {in_rate:?}"
        );
        assert_eq!(
            offline.topology().cascade_stages,
            expected_stages,
            "Cascade stages unexpected for {in_rate:?}"
        );
        assert_eq!(
            offline.topology().rate_multiplier,
            live.topology().rate_multiplier,
            "Rate multiplier mismatch for {in_rate:?}"
        );
        assert_eq!(
            offline.topology().requires_rational_src,
            live.topology().requires_rational_src,
            "Rational SRC flag mismatch for {in_rate:?}"
        );
        assert_eq!(
            offline.topology().is_dsd(),
            live.topology().is_dsd(),
            "is_dsd flag mismatch for {in_rate:?}"
        );
    }
}

#[test]
fn test_parity_pcm_rate_stages() {
    let test_cases = [
        (
            PcmRateHz::PCM_44100,
            "format = \"pcm\"\npcm_rate_hz = 176400",
            2,
            PcmRateHz::PCM_44100,
        ),
        (
            PcmRateHz::PCM_48000,
            "format = \"pcm\"\npcm_rate_hz = 384000",
            3,
            PcmRateHz::PCM_48000,
        ),
        (
            PcmRateHz::PCM_96000,
            "format = \"pcm\"\nrate_family = \"44100\"\npcm_rate_hz = 352800",
            2, // 96k -> 88.2k matched tier -> 4x (2 stages) to 352.8k
            PcmRateHz::PCM_88200,
        ),
    ];

    for (in_rate, target_toml, expected_stages, expected_intermediate) in test_cases {
        let settings = make_test_settings(target_toml, None);

        let offline = OfflinePipeline::prepare(settings.clone(), in_rate)
            .unwrap_or_else(|e| panic!("OfflinePipeline prepare failed: {e}"));
        let live = LivePipeline::prepare(settings, in_rate)
            .unwrap_or_else(|e| panic!("LivePipeline prepare failed: {e}"));

        assert_eq!(
            offline.topology().intermediate_rate,
            live.topology().intermediate_rate
        );
        assert_eq!(offline.topology().intermediate_rate, expected_intermediate);
        assert_eq!(
            offline.topology().cascade_stages,
            live.topology().cascade_stages
        );
        assert_eq!(offline.topology().cascade_stages, expected_stages);
        assert_eq!(
            offline.topology().rate_multiplier,
            live.topology().rate_multiplier
        );
        assert_eq!(
            offline.topology().requires_rational_src,
            live.topology().requires_rational_src
        );
        assert_eq!(offline.topology().is_dsd(), live.topology().is_dsd());
    }
}

#[test]
fn test_parity_carrier_format_derivation() {
    let dsd_target = "format = \"dsd\"\ndsd_rate = \"dsd64\"\nrate_family = \"source\"";

    // Native DSD transport
    let s_native = make_test_settings(dsd_target, Some("native"));
    let topo_dsd = *LivePipeline::prepare(s_native.clone(), PcmRateHz::PCM_44100)
        .unwrap()
        .topology();
    assert_eq!(
        IdleCarrierFormat::from_settings(&s_native, &topo_dsd.target_spec),
        IdleCarrierFormat::NativeDsd
    );

    // DoP transport
    let s_dop = make_test_settings(dsd_target, Some("dop"));
    assert!(matches!(
        IdleCarrierFormat::from_settings(&s_dop, &topo_dsd.target_spec),
        IdleCarrierFormat::DoP(_)
    ));

    // PCM transport
    let pcm_target = "format = \"pcm\"\npcm_rate_hz = 192000";
    let s_pcm = make_test_settings(pcm_target, None);
    let topo_pcm = *LivePipeline::prepare(s_pcm.clone(), PcmRateHz::PCM_48000)
        .unwrap()
        .topology();
    assert_eq!(
        IdleCarrierFormat::from_settings(&s_pcm, &topo_pcm.target_spec),
        IdleCarrierFormat::Pcm
    );
}

#[test]
fn test_parity_dsd_passthrough_shared_rules() {
    let bit_rate = DsdBitRateHz::DSD256;
    let target = TargetRateSpec::Dsd { bit_rate };

    let dsd_info = AudioStreamInfo::new_dsd(bit_rate, 2, None);
    let mut dsp_cfg = audigma_dsp::presets::PRESET_BALANCED.to_default_dsp_config(
        FilterPhase::Linear,
        Apodization::Off,
        0.0,
        0.0,
        OverloadPolicy::MuteRecover,
    );

    // At unity gain (preamp=0, volume=0), matching DSD is eligible in both
    assert!(is_dsd_passthrough_eligible(&dsd_info, &target, &dsp_cfg));

    // Non-zero volume disables passthrough in both
    dsp_cfg.user_volume_db = -6.0;
    assert!(!is_dsd_passthrough_eligible(&dsd_info, &target, &dsp_cfg));

    // Preamp attenuation disables passthrough in both
    dsp_cfg.user_volume_db = 0.0;
    dsp_cfg.preamp_db = -3.0;
    assert!(!is_dsd_passthrough_eligible(&dsd_info, &target, &dsp_cfg));
}
