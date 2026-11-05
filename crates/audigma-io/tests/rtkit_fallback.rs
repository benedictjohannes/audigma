//! Manual check of the RealtimeKit fallback (`cargo test -p audigma-io --test rtkit_fallback -- --ignored`).
//! Needs a running `rtkit-daemon` on the system bus. Does not touch audio devices.
#![cfg(all(target_os = "linux", feature = "dbus"))]

use audigma_engine::ThreadPlacement;

#[test]
#[ignore = "requires rtkit-daemon on the system bus"]
fn forced_rtkit_grants_realtime_with_reset_on_fork() {
    // Safety: single test in this binary; no other thread reads the environment yet.
    unsafe { std::env::set_var("AUDIGMA_FORCE_RTKIT", "1") };
    let result = std::thread::spawn(|| {
        let first = audigma_io::apply(&ThreadPlacement::realtime(18));
        // Second request on another thread reuses the shared bus connection.
        let second = std::thread::spawn(|| audigma_io::apply(&ThreadPlacement::realtime(19)))
            .join()
            .unwrap();
        let policy = unsafe { libc::sched_getscheduler(0) };
        let mut rl = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut rl) };
        (first, second, policy, rl.rlim_cur, rl.rlim_max)
    })
    .join()
    .unwrap();
    let (first, second, policy, soft, hard) = result;
    let first = first.expect("first RTKit request");
    second.expect("second RTKit request");
    assert_eq!(
        first.realtime,
        Some(audigma_io::RealtimePriorityAcquisition::RealtimeKit)
    );
    assert_eq!(policy & !libc::SCHED_RESET_ON_FORK, libc::SCHED_RR);
    assert_ne!(policy & libc::SCHED_RESET_ON_FORK, 0);
    assert_eq!((soft, hard), (200_000, 200_000));
}
