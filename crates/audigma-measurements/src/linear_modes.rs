//! Linear-path measurement modes of `measure_harness`: the real half-band cascade and the
//! polyphase resamplers, driven through the production types (`Design.md §8`).
//!
//! Nothing here re-implements DSP. The constants that shape the analysis (tap counts, phase
//! counts, resampling ratio) are read back from `audigma_dsp` so the analysis cannot drift from
//! the implementation. Output is raw little-endian `f64` samples plus a JSON description; all
//! analysis happens in `tools/measure/spectral/`.
//!
//! Modes:
//! - `cascade-impulse`: unit impulse through `HalfBandCascade`; the response of the implemented
//!   cascade (phase, apodization, stage count).
//! - `cascade-tone`: coherent tones through the same cascade (cross-check of the impulse response).
//! - `src-tone`: coherent tones through `PolyphaseResampler` (rational or ASRC, optional drift).
//! - `src-tables`: the bank a `PolyphaseResampler` builds for `--src-mode/--in-hz/--out-hz`, row-major.

use std::f64::consts::PI;
use std::fs::File;
use std::io::Write;

use audigma_dsp::{Apodization, FilterPhase, HalfBandCascade, PcmRateHz, PolyphaseResampler};

/// Modes handled by this module.
pub const MODES: &[&str] = &["cascade-impulse", "cascade-tone", "src-tone", "src-tables"];

const TILE_BASEBAND: usize = 64;
/// Baseband frames of steady-state signal fed before recording (well beyond any filter delay).
const WARMUP_BASEBAND: usize = 128 * TILE_BASEBAND;
const TONE_AMPLITUDE: f64 = 0.5;
/// Output frames discarded by `src-tone` before recording (far above the resampler's taps).
const SRC_WARMUP_OUTPUT: usize = 4096;
const SRC_CHUNK_INPUT: usize = 256;

struct Args {
    mode: String,
    phase: FilterPhase,
    apodization: Apodization,
    stages: Option<usize>,
    base_hz: f64,
    n_bb: usize,
    tones_hz: Vec<f64>,
    src_asrc: bool,
    in_hz: u32,
    out_hz: u32,
    ppm: f64,
    n_out: usize,
    out_bin: Option<String>,
    out_json: Option<String>,
}

fn parse(args: &[String]) -> Args {
    let mut a = Args {
        mode: String::new(),
        phase: FilterPhase::Linear,
        apodization: Apodization::Off,
        stages: None,
        base_hz: 44_100.0,
        n_bb: 512,
        tones_hz: Vec::new(),
        src_asrc: false,
        in_hz: 44_100,
        out_hz: 48_000,
        ppm: 0.0,
        n_out: 131_072,
        out_bin: None,
        out_json: None,
    };
    let mut i = 1;
    while i < args.len() {
        let flag = args[i].as_str();
        i += 1;
        let val = args.get(i).cloned().unwrap_or_default();
        match flag {
            "--mode" => a.mode = val,
            "--phase" => {
                a.phase = match val.as_str() {
                    "linear" => FilterPhase::Linear,
                    "minimum" => FilterPhase::Minimum,
                    other => panic!("Unknown phase: {other}"),
                }
            }
            "--apodization" => {
                a.apodization = Apodization::from_name(&val)
                    .unwrap_or_else(|| panic!("Unknown apodization: {val}"))
            }
            "--stages" => a.stages = Some(val.parse().expect("Invalid stages")),
            "--base-hz" => a.base_hz = val.parse().expect("Invalid base-hz"),
            "--n-bb" => a.n_bb = val.parse().expect("Invalid n-bb"),
            "--tones-hz" => {
                a.tones_hz = val
                    .split(',')
                    .map(|t| t.parse().expect("Invalid tone"))
                    .collect()
            }
            "--src-mode" => {
                a.src_asrc = match val.as_str() {
                    "rational" => false,
                    "asrc" => true,
                    other => panic!("Unknown src-mode: {other}"),
                }
            }
            "--in-hz" => a.in_hz = val.parse().expect("Invalid in-hz"),
            "--out-hz" => a.out_hz = val.parse().expect("Invalid out-hz"),
            "--ppm" => a.ppm = val.parse().expect("Invalid ppm"),
            "--n-out" => a.n_out = val.parse().expect("Invalid n-out"),
            "--out-bin" => a.out_bin = Some(val),
            "--out-json" => a.out_json = Some(val),
            other => panic!("Unknown argument: {other}"),
        }
        i += 1;
    }
    a
}

/// Returns `true` and runs the mode when `--mode` names one of [`MODES`].
pub fn dispatch(args: &[String]) -> bool {
    let mode = args
        .windows(2)
        .find(|w| w[0] == "--mode")
        .map(|w| w[1].as_str());
    if !mode.is_some_and(|m| MODES.contains(&m)) {
        return false;
    }
    let a = parse(args);
    match a.mode.as_str() {
        "cascade-impulse" => cascade_impulse(&a),
        "cascade-tone" => cascade_tone(&a),
        "src-tone" => src_tone(&a),
        "src-tables" => src_tables(&a),
        _ => unreachable!(),
    }
    true
}

fn write_f64(path: &Option<String>, samples: &[f64]) {
    if let Some(path) = path {
        let mut bytes = Vec::with_capacity(samples.len() * 8);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        File::create(path).unwrap().write_all(&bytes).unwrap();
    }
}

fn write_json(path: &Option<String>, body: &str) {
    if let Some(path) = path {
        File::create(path)
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
    }
}

fn phase_name(p: FilterPhase) -> &'static str {
    match p {
        FilterPhase::Linear => "linear",
        FilterPhase::Minimum => "minimum",
    }
}

fn require_stages(a: &Args) -> usize {
    a.stages.expect("--stages is required for cascade modes")
}

fn new_cascade(a: &Args, stages: usize) -> HalfBandCascade {
    HalfBandCascade::new(a.phase, a.apodization, stages, TILE_BASEBAND)
}

fn cascade_impulse(a: &Args) {
    let stages = require_stages(a);
    assert!(
        a.n_bb % TILE_BASEBAND == 0,
        "--n-bb must be a multiple of 64"
    );
    let mult = 1usize << stages;
    let mut cascade = new_cascade(a, stages);
    let mut out = vec![0.0f64; a.n_bb * mult];
    let mut input = [0.0f64; TILE_BASEBAND];
    for t in 0..a.n_bb / TILE_BASEBAND {
        input.fill(0.0);
        if t == 0 {
            input[0] = 1.0;
        }
        let o = t * TILE_BASEBAND * mult;
        cascade
            .process_tile(&input, &mut out[o..o + TILE_BASEBAND * mult])
            .unwrap();
    }
    write_f64(&a.out_bin, &out);
    write_json(
        &a.out_json,
        &format!(
            r#"{{"mode":"cascade-impulse","phase":"{}","apodization":"{}","stages":{},"mult":{},"n_bb":{},"base_hz":{},"group_delay_output_samples":{}}}"#,
            phase_name(a.phase),
            a.apodization.name(),
            stages,
            mult,
            a.n_bb,
            a.base_hz,
            cascade.total_group_delay_output_samples()
        ),
    );
}

fn cascade_tone(a: &Args) {
    let stages = require_stages(a);
    assert!(
        a.n_bb % TILE_BASEBAND == 0,
        "--n-bb must be a multiple of 64"
    );
    let mult = 1usize << stages;
    let mut all = Vec::with_capacity(a.tones_hz.len() * a.n_bb * mult);
    let mut tones_json = Vec::new();
    for &f in &a.tones_hz {
        let k = ((f * a.n_bb as f64 / a.base_hz).round() as usize).max(1);
        let f_eff = k as f64 * a.base_hz / a.n_bb as f64;
        let mut cascade = new_cascade(a, stages);
        let mut input = [0.0f64; TILE_BASEBAND];
        let mut out = vec![0.0f64; TILE_BASEBAND * mult];
        let total = WARMUP_BASEBAND + a.n_bb;
        for t in 0..total / TILE_BASEBAND {
            for (j, s) in input.iter_mut().enumerate() {
                let idx = (t * TILE_BASEBAND + j) as i64 - WARMUP_BASEBAND as i64;
                *s = TONE_AMPLITUDE
                    * (2.0 * PI * (k as f64) * (idx.rem_euclid(a.n_bb as i64) as f64)
                        / a.n_bb as f64)
                        .sin();
            }
            cascade.process_tile(&input, &mut out).unwrap();
            if t * TILE_BASEBAND >= WARMUP_BASEBAND {
                all.extend_from_slice(&out);
            }
        }
        tones_json.push(format!(r#"{{"f_hz":{f_eff:.6},"k":{k}}}"#));
    }
    write_f64(&a.out_bin, &all);
    write_json(
        &a.out_json,
        &format!(
            r#"{{"mode":"cascade-tone","phase":"{}","apodization":"{}","stages":{},"mult":{},"n_bb":{},"base_hz":{},"amplitude":{},"tones":[{}]}}"#,
            phase_name(a.phase),
            a.apodization.name(),
            stages,
            mult,
            a.n_bb,
            a.base_hz,
            TONE_AMPLITUDE,
            tones_json.join(",")
        ),
    );
}

fn src_tone(a: &Args) {
    let mut all = Vec::with_capacity(a.tones_hz.len() * a.n_out);
    let mut tones_json = Vec::new();
    let mut ratio = 0.0;
    let mut table_phases = 0;
    let mut taps = 0;
    for &f in &a.tones_hz {
        let mut src = make_src(a);
        src.set_drift_ppm(a.ppm);
        ratio = src.input_per_output_ratio();
        table_phases = src.table_phases();
        taps = src.taps_per_phase();
        // Coherent over the recorded output: k cycles in n_out output frames.
        let cycles_per_out = f / a.out_hz as f64 * (a.out_hz as f64 / a.in_hz as f64) * ratio;
        let k = ((cycles_per_out * a.n_out as f64).round() as usize).max(1);
        let f_in = k as f64 * a.in_hz as f64 / (a.n_out as f64 * ratio);

        let want = SRC_WARMUP_OUTPUT + a.n_out;
        let mut produced_total = 0usize;
        let mut n_in: u64 = 0;
        let mut pending: Vec<f64> = Vec::new();
        let mut out = vec![0.0f64; SRC_CHUNK_INPUT * 4 + 64];
        while produced_total < want {
            for _ in 0..SRC_CHUNK_INPUT {
                let phase = 2.0 * PI * f_in * (n_in as f64) / a.in_hz as f64;
                pending.push(TONE_AMPLITUDE * phase.sin());
                n_in += 1;
            }
            let (consumed, produced) = src.process(&pending, &mut out);
            pending.drain(..consumed);
            for &s in &out[..produced] {
                if produced_total >= SRC_WARMUP_OUTPUT && produced_total < want {
                    all.push(s);
                }
                produced_total += 1;
            }
        }
        tones_json.push(format!(r#"{{"f_in_hz":{f_in:.6},"k":{k}}}"#));
    }
    write_f64(&a.out_bin, &all);
    write_json(
        &a.out_json,
        &format!(
            r#"{{"mode":"src-tone","src_mode":"{}","in_hz":{},"out_hz":{},"ppm":{},"ratio":{:.12},"table_phases":{},"taps":{},"n_out":{},"amplitude":{},"tones":[{}]}}"#,
            if a.src_asrc { "asrc" } else { "rational" },
            a.in_hz,
            a.out_hz,
            a.ppm,
            ratio,
            table_phases,
            taps,
            a.n_out,
            TONE_AMPLITUDE,
            tones_json.join(",")
        ),
    );
}

fn src_tables(a: &Args) {
    let src = make_src(a);
    write_f64(&a.out_bin, src.bank());
    write_json(
        &a.out_json,
        &format!(
            r#"{{"mode":"src-tables","src_mode":"{}","in_hz":{},"out_hz":{},"phases":{},"taps":{},"rows":{},"first_phase":{},"scale":{:.17e}}}"#,
            if a.src_asrc { "asrc" } else { "rational" },
            a.in_hz,
            a.out_hz,
            src.table_phases(),
            src.taps_per_phase(),
            src.bank().len() / src.taps_per_phase(),
            if a.src_asrc { -1 } else { 0 },
            src.band_limit_scale(),
        ),
    );
}

fn make_src(a: &Args) -> PolyphaseResampler {
    let in_rate = PcmRateHz::new(a.in_hz).expect("unsupported --in-hz");
    let out_rate = PcmRateHz::new(a.out_hz).expect("unsupported --out-hz");
    if a.src_asrc {
        PolyphaseResampler::new_asrc(in_rate, out_rate)
    } else {
        PolyphaseResampler::new_rational(in_rate, out_rate).expect("unsupported ratio")
    }
}
