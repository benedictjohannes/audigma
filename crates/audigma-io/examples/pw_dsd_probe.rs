//! Standalone diagnostic probe tool for PipeWire Audio/Sink DSD & PCM streams.
//!
//! Creates an Audio/Sink node in PipeWire, negotiates format choices (advertising both
//! PCM and DSD, msb/lsb, interleaved/planar), logs incoming buffer properties, and
//! dumps raw incoming buffer bytes verbatim to a file (default: `probe_dump.bin`).

use audigma_io::pipewire::{build_dsd_format_pod, build_format_pod, parse_format_pod};
use audigma_settings::RateMode;
use libspa::pod::Pod;
use libspa::utils::Direction;
use pipewire::properties::properties;
use pipewire::stream::{StreamBox, StreamFlags};
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let dump_path = args.get(1).map(|s| s.as_str()).unwrap_or("probe_dump.bin");
    let node_name = args.get(2).map(|s| s.as_str()).unwrap_or("audigma_probe");

    println!("=== Audigma PipeWire DSD Probe ===");
    println!("Node name: {node_name}");
    println!("Dump output path: {dump_path}");
    println!("Waiting for client connection (e.g. pw-dsdplay --target {node_name} <file.dsf>)...");

    let mut dump_file = File::create(dump_path)?;

    pipewire::init();
    let mainloop = pipewire::main_loop::MainLoopBox::new(None)?;
    let context = pipewire::context::ContextBox::new(mainloop.loop_(), None)?;
    let core = context.connect(None)?;

    let stream_props = properties! {
        *pipewire::keys::NODE_NAME => node_name,
        *pipewire::keys::NODE_DESCRIPTION => "Audigma DSD Diagnostic Probe Sink",
        *pipewire::keys::MEDIA_CLASS => "Audio/Sink",
        *pipewire::keys::MEDIA_TYPE => "Audio",
        *pipewire::keys::MEDIA_ROLE => "Music",
        *pipewire::keys::AUDIO_CHANNELS => "2",
        "audio.position" => "FL,FR",
        "audio.allowed-rates" => "[ 44100 48000 88200 96000 176400 192000 352800 384000 705600 768000 ]",
    };

    let stream = StreamBox::new(&core, node_name, stream_props)?;

    let running = Arc::new(AtomicBool::new(true));
    let r_clone = running.clone();

    // Spawn a thread to check stdin for newline to cleanly exit
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        r_clone.store(false, Ordering::SeqCst);
    });

    let channels = 2u32;
    let positions = vec!["FL".to_string(), "FR".to_string()];
    let allowed_pcm_rates = vec![
        44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000, 705_600, 768_000,
    ];
    let pcm_pod_bytes = build_format_pod(
        176_400,
        channels,
        &positions,
        RateMode::Dynamic,
        &allowed_pcm_rates,
    )?;

    const ALLOWED_DSD_BYTE_RATES: [u32; 4] = [352_800, 705_600, 1_411_200, 2_822_400];
    let dsd_pod_bytes =
        build_dsd_format_pod(channels, &positions, 352_800, &ALLOWED_DSD_BYTE_RATES)?;

    let mut param_vec = Vec::new();
    if let Some(pcm_pod) = Pod::from_bytes(&pcm_pod_bytes) {
        param_vec.push(pcm_pod);
    }
    if let Some(dsd_pod) = Pod::from_bytes(&dsd_pod_bytes) {
        param_vec.push(dsd_pod);
    }

    let total_bytes_written = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let total_bytes_written_cb = total_bytes_written.clone();
    let mut buffer_count: u64 = 0;

    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |_, _, id, param| {
            let Some(param) = param else { return; };
            if id == libspa::param::ParamType::Format.as_raw() {
                let bytes = param.as_bytes();
                let parsed = parse_format_pod(bytes);
                println!("[probe] >>> Format Negotiated: {:?}", parsed);
                if let Some(p) = parsed {
                    if p.is_dsd {
                        println!(
                            "[probe] Native DSD detected: byte_rate={} (bit_rate={}), bitorder={}, channels={}",
                            p.rate,
                            p.rate * 8,
                            if p.is_lsb { "LSB-first" } else { "MSB-first" },
                            p.channels
                        );
                    } else {
                        println!("[probe] Multi-bit PCM detected: rate={}, channels={}", p.rate, p.channels);
                    }
                }
            }
        })
        .process(move |stream, _| {
            if let Some(mut buffer) = stream.dequeue_buffer() {
                let datas = buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                buffer_count += 1;
                let n_planes = datas.len();

                // For the first 10 buffers and every 100th buffer, print chunk metadata
                if buffer_count <= 10 || buffer_count % 100 == 0 {
                    print!("[probe buf #{buffer_count}] planes={n_planes} ");
                    for (i, d) in datas.iter().enumerate() {
                        let c = d.chunk();
                        print!("[p{i}: off={} sz={}] ", c.offset(), c.size());
                    }
                    println!();
                }

                if n_planes == 1 {
                    let d = &mut datas[0];
                    let chunk = d.chunk();
                    let off = chunk.offset() as usize;
                    let size = chunk.size() as usize;
                    if let Some(bytes) = d.data() {
                        let valid = bytes.len().saturating_sub(off).min(size);
                        if valid > 0 {
                            let _ = dump_file.write_all(&bytes[off..off + valid]);
                            total_bytes_written_cb.fetch_add(valid as u64, Ordering::Relaxed);
                        }
                    }
                } else {
                    for d in datas.iter_mut() {
                        let chunk = d.chunk();
                        let off = chunk.offset() as usize;
                        let size = chunk.size() as usize;
                        if let Some(bytes) = d.data() {
                            let valid = bytes.len().saturating_sub(off).min(size);
                            if valid > 0 {
                                let _ = dump_file.write_all(&bytes[off..off + valid]);
                                total_bytes_written_cb.fetch_add(valid as u64, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
        })
        .register()?;

    stream.connect(
        Direction::Input,
        None,
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
        &mut param_vec,
    )?;

    println!("Probe running. Press ENTER to stop and flush dump.");
    while running.load(Ordering::Relaxed) {
        mainloop.loop_().iterate(pipewire::loop_::Timeout::Finite(
            std::time::Duration::from_millis(100),
        ));
    }

    let total = total_bytes_written.load(Ordering::Relaxed);
    println!("\nExiting. Total bytes dumped: {total} bytes across {buffer_count} buffers.");
    Ok(())
}
