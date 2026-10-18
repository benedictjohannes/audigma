use audigma_io::alsa::probe_alsa_devices;

#[test]
fn test_probe_alsa_devices_returns_report() {
    let report = probe_alsa_devices().expect("probe_alsa_devices should not fail");
    assert!(
        !report.cards.is_empty(),
        "expected at least one audio card on host"
    );

    // Check if K17 is present
    if let Some(k17_card) = report.cards.iter().find(|c| c.id == "K17") {
        assert_eq!(k17_card.usb_id.as_deref(), Some("2972:0107"));
        let pcm0 = &k17_card.devices[0];
        assert_eq!(pcm0.device_path, "hw:CARD=K17,DEV=0");

        if let Some(caps) = &pcm0.capabilities {
            assert!(caps.native_dsd.supported);
            assert!(caps.native_dsd.formats.contains(&"DSD_U32_BE".to_string()));
            assert!(caps.dop.supported);
            assert!(caps.dop.supported_containers.contains(&"s32le".to_string()));
        }
    }
}
