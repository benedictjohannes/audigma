#[cfg(feature = "pipewire")]
use audigma_engine::traits::AudioInput;

#[cfg(feature = "pipewire")]
use audigma_io::pipewire::{PipeWirePcmOutput, PipeWireVirtualSink};

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

    match PipeWirePcmOutput::new("audigma_test_out", None, 48000, 2, 8) {
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
