use audigma_format::dsf_reader::DsfReader;
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Comparing DsfReader vs probe_dump.bin ===");

    // 1. DsfReader on synthetic_pattern.dsf
    let dsf_file = File::open("crates/audigma-format/examples/synthetic_pattern.dsf")?;
    let mut reader = DsfReader::try_new(dsf_file)?;
    let mut dsf_l = vec![0u8; 4096];
    let mut dsf_r = vec![0u8; 4096];
    let read_dsf = reader.read_dsd_bytes(&mut [&mut dsf_l, &mut dsf_r])?;
    println!("DsfReader read {read_dsf} bytes per channel");
    println!("DsfReader L[:16]: {:02x?}", &dsf_l[..16]);
    println!("DsfReader R[:16]: {:02x?}", &dsf_r[..16]);

    // 2. probe_dump.bin (as received by pipewire sink)
    let dump_bytes = std::fs::read("crates/audigma-io/examples/probe_dump.bin")?;
    println!("probe_dump.bin has {} bytes total", dump_bytes.len());
    println!("probe_dump.bin[:16]: {:02x?}", &dump_bytes[..16]);

    // Deinterleave probe_dump bytes [L0, R0, L1, R1, ...]
    let mut pw_l = Vec::new();
    let mut pw_r = Vec::new();
    for chunk in dump_bytes.chunks_exact(2).take(4096) {
        pw_l.push(chunk[0]);
        pw_r.push(chunk[1]);
    }

    println!("PW Deinterleaved L[:16]: {:02x?}", &pw_l[..16]);
    println!("PW Deinterleaved R[:16]: {:02x?}", &pw_r[..16]);

    let diff_l = dsf_l[..4096]
        .iter()
        .zip(pw_l.iter())
        .filter(|(a, b)| a != b)
        .count();
    let diff_r = dsf_r[..4096]
        .iter()
        .zip(pw_r.iter())
        .filter(|(a, b)| a != b)
        .count();
    println!("Diff L: {diff_l} mismatches / 4096");
    println!("Diff R: {diff_r} mismatches / 4096");

    // 3. Test decimation of dsf_l vs pw_l
    let mut decimator1 = audigma_dsp::fir::HalfBandDecimationCascade::new(
        audigma_dsp::FilterPreset::Balanced,
        4,
        65536,
    );
    let mut decimator2 = audigma_dsp::fir::HalfBandDecimationCascade::new(
        audigma_dsp::FilterPreset::Balanced,
        4,
        65536,
    );

    let mut out1 = vec![0.0; 2048];
    let mut out2 = vec![0.0; 2048];

    decimator1.process_dsd_bytes(&dsf_l, &mut out1)?;
    decimator2.process_dsd_bytes(&pw_l, &mut out2)?;

    println!("Decimated out1[:8]: {:?}", &out1[..8]);
    println!("Decimated out2[:8]: {:?}", &out2[..8]);

    let max_diff = out1
        .iter()
        .zip(out2.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f64, f64::max);
    println!("Max decimation diff between DsfReader and PW: {max_diff}");

    // 4. Test ASRC on out2
    let r176 = audigma_dsp::PcmRateHz::new(176400).unwrap();
    let mut asrc = audigma_dsp::fir::PolyphaseResampler::new_asrc(r176, r176);
    let mut asrc_out = vec![0.0; 4096];
    let (consumed, produced) = asrc.process(&out2[..256], &mut asrc_out);
    println!("ASRC consumed: {consumed}, produced: {produced}");
    println!("ASRC out[:8]: {:?}", &asrc_out[..8]);

    Ok(())
}
