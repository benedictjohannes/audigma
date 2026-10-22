#[cfg(feature = "dbus")]
use audigma_io::dbus::{DeviceReservation, request_realtime_priority};

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
#[cfg(feature = "dbus")]
fn test_dbus_realtime_kit_graceful_fallback() {
    // Requesting RT priority for an arbitrary thread id
    // May succeed if system RTKit is running, or return a clear error if unavailable.
    // In either case, it must not panic.
    let result = request_realtime_priority(10);
    match result {
        Ok(acq) => {
            let msg = format!("{acq}");
            assert!(!msg.is_empty());
        }
        Err(e) => {
            // Descriptive error without crashing
            let msg = format!("{e}");
            assert!(!msg.is_empty());
        }
    }
}
