//! Offline PCM writers (WAV, FLAC, raw) must share one quantizer and honour
//! `target.dither` / `dither_tpdf_seed` exactly like the live path.

use audigma_dsp::config::{DitherType, SampleFormat};
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::OfflinePipeline;
use audigma_engine::traits::{MockAudioInput, SeekableSink};
use audigma_format::AudioStreamInfo;
use audigma_format::decoder::AudioDecoder;
use audigma_settings::{OutputContainer, OutputTransport, PeakAnalysisMode, Settings};
use std::f64::consts::PI;
use std::io::Cursor;

const IN_RATE: PcmRateHz = PcmRateHz::PCM_44100;

fn settings(format: SampleFormat, dither: DitherType, seed: Option<u64>) -> Settings {
    let mut s = Settings::default_offline(OutputContainer::Wav);
    let target = s.target.as_mut().unwrap();
    target.pcm_rate_hz = Some(PcmRateHz::PCM_48000);
    target.sample_format = Some(format);
    target.dither = Some(dither);
    target.dither_tpdf_seed = seed;
    let output = s.output.as_mut().unwrap();
    output.transport = Some(OutputTransport::Pcm);
    s.gain.as_mut().unwrap().peak_analysis = PeakAnalysisMode::SinglePass;
    s
}

/// A quiet low-frequency sine keeps quantization error visible (a few LSB at 16 bit).
fn convert(container: OutputContainer, settings: Settings) -> Vec<u8> {
    let n = (IN_RATE.get() as f64 * 0.05) as usize;
    let sine = |phase: f64| -> Vec<f64> {
        (0..n)
            .map(|i| 3.3e-5 * (2.0 * PI * 441.0 * i as f64 / IN_RATE.get() as f64 + phase).sin())
            .collect()
    };
    let info = AudioStreamInfo::new_pcm(IN_RATE, 2, Some(n as u64), Some(16));
    let mut input = MockAudioInput::new(info, sine(0.0), sine(PI / 2.0), true);
    let mut out = Cursor::new(Vec::new());
    OfflinePipeline::prepare(settings, IN_RATE)
        .unwrap()
        .with_container(container)
        .run(&mut input, Box::new(SeekableSink(&mut out)))
        .unwrap();
    out.into_inner()
}

fn wav_i16(bytes: Vec<u8>) -> Vec<i16> {
    hound::WavReader::new(Cursor::new(bytes))
        .unwrap()
        .samples::<i16>()
        .map(Result::unwrap)
        .collect()
}

fn raw_i16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect()
}

fn flac_i16(bytes: Vec<u8>) -> Vec<i16> {
    let mut dec = AudioDecoder::try_new(Cursor::new(bytes), Some("flac")).unwrap();
    let (mut l, mut r) = (Vec::new(), Vec::new());
    dec.read_stereo_chunk(&mut l, &mut r).unwrap();
    l.iter()
        .zip(&r)
        .flat_map(|(&l, &r)| [l, r])
        .map(|x| (x * 32768.0).round() as i16)
        .collect()
}

#[test]
fn tpdf_seed_is_deterministic_and_none_differs() {
    let run = |dither, seed| {
        wav_i16(convert(
            OutputContainer::Wav,
            settings(SampleFormat::S16Le, dither, seed),
        ))
    };
    let a = run(DitherType::Tpdf, Some(42));
    let b = run(DitherType::Tpdf, Some(42));
    let c = run(DitherType::Tpdf, Some(43));
    let none = run(DitherType::None, Some(42));
    assert_eq!(a, b, "same seed must reproduce exactly");
    assert_ne!(a, c, "different seed must change the dither");
    assert_ne!(a, none, "dither must change the output");
    assert_eq!(
        none,
        run(DitherType::None, Some(99)),
        "seed is irrelevant without dither"
    );
}

#[test]
fn wav_flac_raw_agree_sample_for_sample() {
    for dither in [DitherType::None, DitherType::Tpdf] {
        let wav = wav_i16(convert(
            OutputContainer::Wav,
            settings(SampleFormat::S16Le, dither, Some(7)),
        ));
        let flac = flac_i16(convert(
            OutputContainer::Flac,
            settings(SampleFormat::S16Le, dither, Some(7)),
        ));
        let raw = raw_i16(&convert(
            OutputContainer::Raw,
            settings(SampleFormat::S16Le, dither, Some(7)),
        ));
        assert!(!wav.is_empty());
        assert_eq!(wav, raw, "raw != wav for {dither:?}");
        assert_eq!(wav, flac, "flac != wav for {dither:?}");
    }
}

#[test]
fn raw_s24le_uses_24_bit_scale() {
    // Raw S24_LE words carry 24-bit codes (as live does), not 32-bit full scale.
    let bytes = convert(
        OutputContainer::Raw,
        settings(SampleFormat::S24Le, DitherType::None, None),
    );
    let max = bytes
        .chunks_exact(4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]).abs())
        .max()
        .unwrap();
    assert!(max > 0 && max <= 8_388_608, "max code {max}");
}
