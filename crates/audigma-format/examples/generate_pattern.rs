use audigma_dsp::DsdBitRateHz;
use audigma_format::dsf::DsfPadMode;
use audigma_format::dsf_writer::DsfFileWriter;
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create("convertTesting/synthetic_pattern.dsf")?;
    // DSD64: 2,822,400 bits/sec = 352,800 bytes/sec per channel.
    // Let's generate exactly 1 second (352,800 bytes per channel).
    let total_bytes = 352_800;
    let mut writer = DsfFileWriter::new(
        file,
        DsdBitRateHz::DSD64,
        audigma_format::dsf::DsfBitOrder::LsbFirst, // Standard DSF is LsbFirst
        DsfPadMode::SpecZero,
    )?;

    // Left channel pattern: recognizable repeating 4-byte sequence [0xA1, 0xB2, 0xC3, 0xD4]
    // Right channel pattern: recognizable repeating 4-byte sequence [0x10, 0x20, 0x30, 0x40]
    let mut left = Vec::with_capacity(total_bytes);
    let mut right = Vec::with_capacity(total_bytes);

    let l_pat = [0xA1u8, 0xB2, 0xC3, 0xD4];
    let r_pat = [0x10u8, 0x20, 0x30, 0x40];

    for i in 0..total_bytes {
        left.push(l_pat[i % 4]);
        right.push(r_pat[i % 4]);
    }

    writer.write_stereo_dsd_bytes(&left, &right)?;
    writer.finalize()?;
    println!(
        "Successfully generated convertTesting/synthetic_pattern.dsf (1 sec, {} bytes/ch)",
        total_bytes
    );
    Ok(())
}
