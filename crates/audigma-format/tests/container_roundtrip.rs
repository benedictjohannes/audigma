//! Container roundtrip tests: WAV and FLAC encoding & decoding with rewind.

use audigma_dsp::rates::PcmRateHz;
use audigma_dsp::{DsdBitRateHz, SampleFormat};
use audigma_format::decoder::AudioDecoder;
use audigma_format::dsf::{DsfBitOrder, DsfPadMode};
use audigma_format::dsf_reader::DsfReader;
use audigma_format::dsf_writer::DsfFileWriter;
use audigma_format::flac_writer::FlacEncoder;
use audigma_format::metadata::{AudioMetadata, Picture, PictureType};
use audigma_format::wav_writer::PcmWavWriter;
use std::io::Cursor;

fn create_sample_metadata() -> AudioMetadata {
    let mut meta = AudioMetadata::default();
    meta.title = Some("Audigma Anthem".to_string());
    meta.artist = Some("Audigma Artist".to_string());
    meta.album = Some("Hi-Res Album".to_string());
    meta.date = Some("2025".to_string());
    meta.genre = Some("Audiophile".to_string());
    meta.track_number = Some(7);
    meta.track_total = Some(10);
    meta.comment = Some("Bit-perfect test".to_string());
    meta.pictures.push(Picture {
        picture_type: PictureType::CoverFront,
        mime_type: "image/jpeg".to_string(),
        description: "Front Cover".to_string(),
        width: 100,
        height: 100,
        color_depth: 24,
        num_colors: 0,
        data: vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01, 0xFF, 0xD9,
        ],
    });
    meta
}

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

#[test]
fn test_flac_metadata_roundtrip() {
    let sample_rate = PcmRateHz::PCM_48000;
    let frames = 256;
    let orig_l = vec![0.1f64; frames];
    let orig_r = vec![-0.1f64; frames];

    let meta = create_sample_metadata();
    let mut flac_bytes = Vec::new();
    {
        let mut encoder = FlacEncoder::new(&mut flac_bytes, sample_rate, 16)
            .unwrap()
            .with_metadata(meta.clone());
        encoder.write_stereo_samples(&orig_l, &orig_r).unwrap();
        encoder.finalize().unwrap();
    }

    let cursor = Cursor::new(flac_bytes);
    let decoder = AudioDecoder::try_new(cursor, Some("flac")).unwrap();
    let read_meta = decoder.metadata().expect("Metadata should be present");

    assert_eq!(read_meta.title.as_deref(), Some("Audigma Anthem"));
    assert_eq!(read_meta.artist.as_deref(), Some("Audigma Artist"));
    assert_eq!(read_meta.album.as_deref(), Some("Hi-Res Album"));
    assert_eq!(read_meta.date.as_deref(), Some("2025"));
    assert_eq!(read_meta.genre.as_deref(), Some("Audiophile"));
    assert_eq!(read_meta.track_number, Some(7));
    assert_eq!(read_meta.track_total, Some(10));
    assert_eq!(read_meta.comment.as_deref(), Some("Bit-perfect test"));
    assert_eq!(read_meta.pictures.len(), 1);
    assert_eq!(read_meta.pictures[0].picture_type, PictureType::CoverFront);
    assert_eq!(read_meta.pictures[0].mime_type, "image/jpeg");
    assert_eq!(read_meta.pictures[0].data, meta.pictures[0].data);
}

#[test]
fn test_dsf_to_flac_metadata_roundtrip() {
    let dsd_rate = DsdBitRateHz::DSD64;
    let pcm_rate = PcmRateHz::PCM_44100;
    let meta = create_sample_metadata();

    // 1. Create DSF in memory with ID3v2 metadata
    let mut dsf_buffer = Cursor::new(Vec::new());
    {
        let mut writer = DsfFileWriter::new(
            &mut dsf_buffer,
            dsd_rate,
            DsfBitOrder::LsbFirst,
            DsfPadMode::SpecZero,
        )
        .unwrap()
        .with_metadata(meta.clone());
        // Write 1 block of DSD stereo data (4096 bytes per channel)
        let ch0 = vec![0xAAu8; 4096];
        let ch1 = vec![0x55u8; 4096];
        writer.write_stereo_dsd_bytes(&ch0, &ch1).unwrap();
        writer.finalize().unwrap();
    }

    // 2. Read DSF and verify metadata
    dsf_buffer.set_position(0);
    let dsf_reader = DsfReader::try_new(dsf_buffer).unwrap();
    let dsf_meta = dsf_reader
        .metadata()
        .expect("DSF metadata should be present");
    assert_eq!(dsf_meta.title.as_deref(), Some("Audigma Anthem"));
    assert_eq!(dsf_meta.artist.as_deref(), Some("Audigma Artist"));
    assert_eq!(dsf_meta.album.as_deref(), Some("Hi-Res Album"));
    assert_eq!(dsf_meta.track_number, Some(7));
    assert_eq!(dsf_meta.pictures.len(), 1);

    // 3. Forward DSF metadata into FLAC
    let mut flac_bytes = Vec::new();
    {
        let mut encoder = FlacEncoder::new(&mut flac_bytes, pcm_rate, 24)
            .unwrap()
            .with_metadata(dsf_meta.clone());
        let l = vec![0.0f64; 100];
        let r = vec![0.0f64; 100];
        encoder.write_stereo_samples(&l, &r).unwrap();
        encoder.finalize().unwrap();
    }

    // 4. Decode FLAC and verify metadata survived
    let cursor = Cursor::new(flac_bytes);
    let flac_decoder = AudioDecoder::try_new(cursor, Some("flac")).unwrap();
    let final_meta = flac_decoder
        .metadata()
        .expect("FLAC metadata should be present");
    assert_eq!(final_meta.title.as_deref(), Some("Audigma Anthem"));
    assert_eq!(final_meta.artist.as_deref(), Some("Audigma Artist"));
    assert_eq!(final_meta.album.as_deref(), Some("Hi-Res Album"));
    assert_eq!(final_meta.track_number, Some(7));
    assert_eq!(final_meta.pictures.len(), 1);
    assert_eq!(
        final_meta.pictures[0].picture_type,
        PictureType::CoverFront
    );
    assert_eq!(final_meta.pictures[0].data, meta.pictures[0].data);
}

#[test]
fn test_flac_to_wav_to_dsf_metadata_roundtrip() {
    let pcm_rate = PcmRateHz::PCM_48000;
    let dsd_rate = DsdBitRateHz::DSD128;
    let meta = create_sample_metadata();

    // 1. FLAC with metadata
    let mut flac_bytes = Vec::new();
    {
        let mut encoder = FlacEncoder::new(&mut flac_bytes, pcm_rate, 24)
            .unwrap()
            .with_metadata(meta.clone());
        let l = vec![0.0f64; 100];
        let r = vec![0.0f64; 100];
        encoder.write_stereo_samples(&l, &r).unwrap();
        encoder.finalize().unwrap();
    }

    // 2. Decode FLAC, extract metadata
    let flac_decoder = AudioDecoder::try_new(Cursor::new(flac_bytes), Some("flac")).unwrap();
    let flac_meta = flac_decoder.metadata().unwrap().clone();

    // 3. Write WAV with metadata (RIFF INFO tags)
    let mut wav_buffer = Cursor::new(Vec::new());
    {
        let mut wav_writer =
            PcmWavWriter::new_with_metadata(&mut wav_buffer, pcm_rate, SampleFormat::S24Le, Some(flac_meta.clone())).unwrap();
        let l = vec![0.0f64; 100];
        let r = vec![0.0f64; 100];
        wav_writer.write_stereo_samples(&l, &r).unwrap();
        wav_writer.finalize().unwrap();
    }

    // 4. Decode WAV, verify RIFF INFO tags
    wav_buffer.set_position(0);
    let wav_decoder = AudioDecoder::try_new(wav_buffer, Some("wav")).unwrap();
    let wav_meta = wav_decoder
        .metadata()
        .expect("WAV metadata should be present");
    assert_eq!(wav_meta.title.as_deref(), Some("Audigma Anthem"));
    assert_eq!(wav_meta.artist.as_deref(), Some("Audigma Artist"));
    assert_eq!(wav_meta.album.as_deref(), Some("Hi-Res Album"));
    assert_eq!(wav_meta.genre.as_deref(), Some("Audiophile"));
    assert_eq!(wav_meta.track_number, Some(7));

    // 5. Forward WAV metadata to DSF
    let mut dsf_buffer = Cursor::new(Vec::new());
    {
        let mut dsf_writer = DsfFileWriter::new(
            &mut dsf_buffer,
            dsd_rate,
            DsfBitOrder::LsbFirst,
            DsfPadMode::SpecZero,
        )
        .unwrap()
        .with_metadata(wav_meta.clone());
        let ch0 = vec![0x00u8; 4096];
        let ch1 = vec![0x00u8; 4096];
        dsf_writer.write_stereo_dsd_bytes(&ch0, &ch1).unwrap();
        dsf_writer.finalize().unwrap();
    }

    // 6. Decode DSF, verify ID3v2 tags
    dsf_buffer.set_position(0);
    let dsf_reader = DsfReader::try_new(dsf_buffer).unwrap();
    let dsf_meta = dsf_reader.metadata().expect("DSF metadata should be present");
    assert_eq!(dsf_meta.title.as_deref(), Some("Audigma Anthem"));
    assert_eq!(dsf_meta.artist.as_deref(), Some("Audigma Artist"));
    assert_eq!(dsf_meta.album.as_deref(), Some("Hi-Res Album"));
    assert_eq!(dsf_meta.genre.as_deref(), Some("Audiophile"));
    assert_eq!(dsf_meta.track_number, Some(7));
}
