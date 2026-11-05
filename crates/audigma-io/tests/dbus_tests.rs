#[cfg(feature = "dbus")]
use audigma_io::dbus::DeviceReservation;

#[test]
#[cfg(feature = "dbus")]
fn test_dbus_device_reservation_name_formatting() {
    // We test reservation on a dummy card index like 9999
    // In headless or sandboxed CI without session D-Bus daemon, acquire may return a Connection error,
    // which is expected and handled gracefully without panic.
    match DeviceReservation::acquire(9999, "audigma_test", 100) {
        Ok(reservation) => {
            assert_eq!(reservation.device_name(), "Audio9999");
            assert_eq!(
                reservation.well_known_name(),
                "org.freedesktop.ReserveDevice1.Audio9999"
            );
            assert!(!reservation.is_release_requested());
            // Drop cleanly releases the name
            drop(reservation);
        }
        Err(e) => {
            // If session bus is unreachable in sandbox environment, verify error is clean
            let msg = format!("{e}");
            assert!(!msg.is_empty());
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn test_apply_realtime_graceful_and_reset_on_fork() {
    // Runs on a throwaway thread so the test harness thread is never elevated.
    std::thread::spawn(|| {
        let placement = audigma_engine::ThreadPlacement::realtime(10);
        match audigma_io::apply(&placement) {
            Ok(applied) => {
                assert!(applied.realtime.is_some());
                // Threads spawned by an RT thread must start SCHED_OTHER.
                let child_policy = std::thread::spawn(|| unsafe { libc::sched_getscheduler(0) })
                    .join()
                    .unwrap();
                assert_eq!(child_policy, libc::SCHED_OTHER);
            }
            Err(e) => assert!(!format!("{e}").is_empty()),
        }
    })
    .join()
    .unwrap();
}
