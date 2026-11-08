//! Container roundtrip tests: WAV and FLAC encoding & decoding with rewind.

use audigma_dsp::SampleFormat;
use audigma_dsp::rates::PcmRateHz;
use audigma_format::decoder::AudioDecoder;
use audigma_format::flac_writer::FlacEncoder;
use audigma_format::wav_writer::PcmWavWriter;
use std::io::Cursor;

#[test]
fn test_wav_encode_and_decode_roundtrip_with_rewind() {
    let sample_rate = PcmRateHz::PCM_48000;
    let frames = 512;

    // Generate known synthetic stereo sine wave
    let mut orig_l = Vec::with_capacity(frames);
    let mut orig_r = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f64 / 48000.0;
        let l = (2.0 * std::f64::consts::PI * 1000.0 * t).sin() * 0.5;
        let r = (2.0 * std::f64::consts::PI * 2000.0 * t).sin() * 0.25;
        orig_l.push(l);
        orig_r.push(r);
    }

    // 1. Encode into WAV memory buffer
    let mut wav_bytes = Cursor::new(Vec::new());
    {
        let mut writer =
            PcmWavWriter::new(&mut wav_bytes, sample_rate, SampleFormat::S24Le).unwrap();
        writer.write_stereo_samples(&orig_l, &orig_r).unwrap();
        writer.finalize().unwrap();
    }

    // 2. Decode using AudioDecoder
    wav_bytes.set_position(0);
    let mut decoder = AudioDecoder::try_new(wav_bytes, Some("wav")).unwrap();
    assert_eq!(decoder.info().sample_rate, sample_rate);
    assert_eq!(decoder.info().original_channels, 2);

    let mut dec_l = Vec::new();
    let mut dec_r = Vec::new();
    let read_count = decoder.read_stereo_chunk(&mut dec_l, &mut dec_r).unwrap();
    assert_eq!(read_count, frames);
    assert_eq!(dec_l.len(), frames);
    assert_eq!(dec_r.len(), frames);

    // Verify sample fidelity within 24-bit precision (error < 1e-6)
    for i in 0..frames {
        assert!(
            (dec_l[i] - orig_l[i]).abs() < 1e-6,
            "L sample {i} mismatch: orig {}, dec {}",
            orig_l[i],
            dec_l[i]
        );
        assert!(
            (dec_r[i] - orig_r[i]).abs() < 1e-6,
            "R sample {i} mismatch: orig {}, dec {}",
            orig_r[i],
            dec_r[i]
        );
    }

    // 3. Test rewind (for two-pass peak analysis)
    decoder.rewind().unwrap();
    let mut rewind_l = Vec::new();
    let mut rewind_r = Vec::new();
    let rewind_count = decoder
        .read_stereo_chunk(&mut rewind_l, &mut rewind_r)
        .unwrap();
    assert_eq!(rewind_count, frames);
    assert_eq!(rewind_l, dec_l);
    assert_eq!(rewind_r, dec_r);
}

#[test]
fn test_flac_encode_and_decode_roundtrip() {
    let sample_rate = PcmRateHz::PCM_96000;
    let frames = 1024;

    // Generate known synthetic stereo signal
    let mut orig_l = Vec::with_capacity(frames);
    let mut orig_r = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f64 / 96000.0;
        let l = (2.0 * std::f64::consts::PI * 440.0 * t).sin() * 0.7;
        let r = (2.0 * std::f64::consts::PI * 880.0 * t).sin() * 0.6;
        orig_l.push(l);
        orig_r.push(r);
    }

    // 1. Encode into FLAC memory buffer (24-bit)
    let mut flac_bytes = Vec::new();
    {
        let mut encoder = FlacEncoder::new(&mut flac_bytes, sample_rate, 24).unwrap();
        encoder.write_stereo_samples(&orig_l, &orig_r).unwrap();
        let written = encoder.finalize().unwrap();
        assert_eq!(written, frames as u64);
    }

    // 2. Decode using AudioDecoder with "flac" hint
    println!("FLAC bytes length: {}", flac_bytes.len());
    println!(
        "FLAC first 64 bytes: {:02X?}",
        &flac_bytes[..flac_bytes.len().min(64)]
    );
    let cursor = Cursor::new(flac_bytes);
    let mut decoder = match AudioDecoder::try_new(cursor, Some("flac")) {
        Ok(d) => d,
        Err(e) => panic!("AudioDecoder failed: {:?}", e),
    };
    assert_eq!(decoder.info().sample_rate, sample_rate);
    assert_eq!(decoder.info().original_channels, 2);

    let mut dec_l = Vec::new();
    let mut dec_r = Vec::new();
    let read_count = decoder.read_stereo_chunk(&mut dec_l, &mut dec_r).unwrap();
    assert_eq!(read_count, frames);

    // Verify 24-bit lossless fidelity
    for i in 0..frames {
        assert!(
            (dec_l[i] - orig_l[i]).abs() < 1e-6,
            "FLAC L sample {i} mismatch: orig {}, dec {}",
            orig_l[i],
            dec_l[i]
        );
        assert!(
            (dec_r[i] - orig_r[i]).abs() < 1e-6,
            "FLAC R sample {i} mismatch: orig {}, dec {}",
            orig_r[i],
            dec_r[i]
        );
    }
}

#[test]
fn test_flac_multiblock_uneven_roundtrip() {
    let sample_rate = PcmRateHz::PCM_44100;
    // 5000 frames spans across default block_size (4096) with an uneven last frame of 904 frames
    let frames = 5000;

    let mut orig_l = Vec::with_capacity(frames);
    let mut orig_r = Vec::with_capacity(frames);
    for i in 0..frames {
        let t = i as f64 / 44100.0;
        let l = (2.0 * std::f64::consts::PI * 300.0 * t).sin() * 0.8;
        let r = (2.0 * std::f64::consts::PI * 600.0 * t).sin() * 0.4;
        orig_l.push(l);
        orig_r.push(r);
    }

    let mut flac_bytes = Vec::new();
    {
        let mut encoder = FlacEncoder::new(&mut flac_bytes, sample_rate, 16).unwrap();
        encoder.write_stereo_samples(&orig_l, &orig_r).unwrap();
        let written = encoder.finalize().unwrap();
        assert_eq!(written, frames as u64);
    }

    let cursor = Cursor::new(flac_bytes);
    let mut decoder = AudioDecoder::try_new(cursor, Some("flac")).unwrap();
    assert_eq!(decoder.info().sample_rate, sample_rate);

    let mut dec_l = Vec::new();
    let mut dec_r = Vec::new();
    let mut total_read = 0;
    loop {
        let read = decoder.read_stereo_chunk(&mut dec_l, &mut dec_r).unwrap();
        if read == 0 {
            break;
        }
        total_read += read;
    }
    assert_eq!(total_read, frames);

    // Verify 16-bit fidelity (precision within ~1/32767 ≈ 3e-5)
    for i in 0..frames {
        assert!(
            (dec_l[i] - orig_l[i]).abs() < 5e-5,
            "FLAC L sample {i} mismatch: orig {}, dec {}",
            orig_l[i],
            dec_l[i]
        );
        assert!(
            (dec_r[i] - orig_r[i]).abs() < 5e-5,
            "FLAC R sample {i} mismatch: orig {}, dec {}",
            orig_r[i],
            dec_r[i]
        );
    }
}
