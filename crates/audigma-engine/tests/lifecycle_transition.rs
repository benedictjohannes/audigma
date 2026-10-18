use audigma_engine::lifecycle::{
    BlockAction, IdleCarrierFormat, LifecycleController, LifecycleState,
};
use audigma_format::dop::DopContainerFormat;

#[test]
fn test_lifecycle_pipeline_transition_sequence() {
    let mut lc = LifecycleController::new(
        IdleCarrierFormat::DoP(DopContainerFormat::S32Le),
        128, // FIR group delay
        64,  // fade frames
    );

    // Initial state
    assert_eq!(lc.state(), LifecycleState::Stopped);

    // Prepare
    lc.prepare().expect("prepare failed");
    assert_eq!(lc.state(), LifecycleState::Preparing);

    // Prime
    lc.prime().expect("prime failed");
    assert_eq!(lc.state(), LifecycleState::Priming);

    // Start
    lc.start().expect("start failed");
    assert_eq!(lc.state(), LifecycleState::Running);

    // Feed audio blocks through fade-in
    let mut left = vec![0.8; 64];
    let mut right = vec![0.8; 64];
    let action = lc.process_input_block(&mut left, &mut right);
    match action {
        BlockAction::FadingIn { current_gain } => {
            assert_eq!(current_gain, 1.0); // 64 frames reached 1.0
            assert_eq!(left[63], 0.8);
        }
        _ => panic!("expected fading in to complete at frame 64"),
    }

    // Steady-state audio
    let mut left_steady = vec![0.5; 128];
    let mut right_steady = vec![0.5; 128];
    assert_eq!(
        lc.process_input_block(&mut left_steady, &mut right_steady),
        BlockAction::Normal
    );
    assert_eq!(left_steady[0], 0.5);

    // Request stop (fade 64 + group delay 128 = 192 drain frames total)
    lc.request_stop();
    assert_eq!(lc.state(), LifecycleState::DrainingMuting);

    // Drain block 1 (64 frames)
    let mut l_d1 = vec![0.5; 64];
    let mut r_d1 = vec![0.5; 64];
    let a1 = lc.process_input_block(&mut l_d1, &mut r_d1);
    match a1 {
        BlockAction::Draining {
            drain_frames_remaining,
            ..
        } => {
            assert_eq!(drain_frames_remaining, 128);
            assert_eq!(lc.current_gain(), 0.0); // fade out finished
        }
        _ => panic!("expected draining block"),
    }

    // Drain block 2 (128 frames) -> clears the tail
    let mut l_d2 = vec![0.5; 128];
    let mut r_d2 = vec![0.5; 128];
    let a2 = lc.process_input_block(&mut l_d2, &mut r_d2);
    assert_eq!(a2, BlockAction::Drained);
    assert_eq!(lc.state(), LifecycleState::Stopped);

    // In Stopped state, input is muted to zero and idle frames preserve DoP markers
    let mut l_stop = vec![1.0; 32];
    let mut r_stop = vec![1.0; 32];
    assert_eq!(
        lc.process_input_block(&mut l_stop, &mut r_stop),
        BlockAction::Muted
    );
    assert_eq!(l_stop[0], 0.0);

    // Idle frames check
    let mut idle_buf = vec![0u8; 16]; // 2 frames
    let written = lc.emit_idle_frames(&mut idle_buf);
    assert_eq!(written, 16);
    // Frame 0 marker at index 3, frame 1 marker at index 11
    assert!(idle_buf[3] == 0x05 || idle_buf[3] == 0xFA);
    assert!(idle_buf[11] == 0x05 || idle_buf[11] == 0xFA);
    assert_ne!(idle_buf[3], idle_buf[11]); // Must alternate!
}
