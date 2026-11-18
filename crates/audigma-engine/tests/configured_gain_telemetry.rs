use audigma_engine::{DaemonControlHandle, DaemonOutputFormat};

#[test]
fn configured_gain_sums_preamp_headroom_and_decimator_only_for_dsd_input() {
    let control = DaemonControlHandle::new(
        44_100,
        DaemonOutputFormat::Dsd,
        11_289_600,
        "linear, apodization off",
        1.0,
    );
    control.set_profile_headroom_db(-4.5);

    let pcm = control.get_telemetry();
    assert_eq!(pcm.dsd_gain_db, None);
    assert_eq!(pcm.configured_gain_db, -3.5);

    control.set_input_dsd_gain_db(Some(2.0));
    let dsd = control.get_telemetry();
    assert_eq!(dsd.dsd_gain_db, Some(2.0));
    assert_eq!(dsd.configured_gain_db, -1.5);
    assert_eq!(dsd.profile_headroom_db, -4.5);
}
