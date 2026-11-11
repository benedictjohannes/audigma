use rusty_flac::{decode, Encoder};

fn lcg(s: &mut u64) -> i64 {
    *s = s
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*s >> 33) as i64
}

fn gen(kind: &str, n: usize, ch: usize) -> Vec<Vec<i32>> {
    let mut seed = 7u64;
    (0..ch)
        .map(|c| {
            (0..n)
                .map(|i| {
                    let t = i as f64 / 44100.0;
                    let ph = c as f64 * 0.3;
                    match kind {
                        "sine_noise" => {
                            let v = ((t * 997.0 * std::f64::consts::TAU + ph).sin()
                                * 0.5
                                * 2147483647.0) as i64;
                            (v + (lcg(&mut seed) & 0xFFFF) - 0x8000) as i32
                        }
                        "loud_square" => {
                            if (i / 50) % 2 == 0 {
                                i32::MAX
                            } else {
                                i32::MIN
                            }
                        }
                        "extremes" => match i % 4 {
                            0 => i32::MAX,
                            1 => i32::MIN,
                            2 => 0,
                            _ => -1,
                        },
                        "silence" => 0,
                        "const" => -123456789,
                        "wasted24" => {
                            (((t * 440.0 * std::f64::consts::TAU).sin() * 8_000_000.0) as i32) << 8
                        }
                        "white" => {
                            (lcg(&mut seed) as i32).wrapping_mul(4) ^ (lcg(&mut seed) as i32)
                        }
                        _ => unreachable!(),
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn roundtrip_and_files() {
    let dir = std::env::var("WIDE_DIR").ok();
    let kinds = [
        "sine_noise",
        "loud_square",
        "extremes",
        "silence",
        "const",
        "wasted24",
        "white",
    ];
    for kind in kinds {
        for (rate, n, ch) in [
            (44100u32, 20000usize, 2usize),
            (192000, 9000, 2),
            (384000, 4097, 2),
            (44100, 1, 2),
            (44100, 5, 2),
            (96000, 8192, 1),
            (48000, 5000, 3),
        ] {
            let planes = gen(kind, n, ch);
            let mut enc = Encoder::new(rate, ch as u32, 32).unwrap();
            let refs: Vec<&[i32]> = planes.iter().map(|v| v.as_slice()).collect();
            enc.push_planar(&refs).unwrap();
            let flac = enc.finish();
            let (info, out) = decode(&flac).unwrap();
            assert_eq!(info.sample_rate, rate);
            assert_eq!(out, planes, "{kind} {rate} {n} {ch}");
            if let Some(d) = &dir {
                std::fs::write(format!("{d}/{kind}_{rate}_{n}_{ch}.flac"), &flac).unwrap();
                let mut raw = Vec::new();
                for i in 0..n {
                    for plane in &planes {
                        raw.extend_from_slice(&plane[i].to_le_bytes());
                    }
                }
                std::fs::write(format!("{d}/{kind}_{rate}_{n}_{ch}.s32"), raw).unwrap();
            }
            if kind == "sine_noise" && n == 20000 && ch == 2 && rate == 44100 {
                eprintln!("sine_noise 20000x2: {} bytes of {}", flac.len(), n * 8);
            }
        }
    }
}
