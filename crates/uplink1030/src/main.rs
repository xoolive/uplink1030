use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use desperado::parse_si_value;
use rs1030::decode_frame;
use rs1030::demodulate_snippet;
use rs1030::dsp::demodulator::{demodulate_detection_with_timing, demodulate_snippet_with_timing};
use rs1030::dsp::timing::{UplinkTiming, DEFAULT_UPLINK_SAMPLE_RATE_HZ};
use rs1030::source::iqread::read_cf32_file;
use rs1030::Detector;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut stats = false;
    let mut pretty = false;
    let mut detect = false;
    let mut sample_rate_hz = DEFAULT_UPLINK_SAMPLE_RATE_HZ;
    let mut paths = Vec::new();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stats" => stats = true,
            "--pretty" => pretty = true,
            "--detect" => detect = true,
            "--sample-rate" => {
                let value = args.next().ok_or("--sample-rate requires a value")?;
                sample_rate_hz = parse_si_value::<u32>(&value)?;
            }
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }

    let timing = UplinkTiming::from_sample_rate(sample_rate_hz)?;

    if paths.is_empty() {
        print_help();
        return Ok(());
    }

    let files = expand_paths(&paths)?;
    let mut uf_counts: BTreeMap<u8, usize> = BTreeMap::new();
    let mut decoded_count = 0usize;

    for file in files {
        let iq = read_cf32_file(&file)?;
        if detect {
            let detector = Detector::with_timing(3.0, false, timing);
            for detection in detector.detect(&iq, 0) {
                let frame = demodulate_detection_with_timing(
                    &iq,
                    detection.p6_sample,
                    detection.num_bits,
                    &timing,
                )?;
                let decoded = decode_frame(&frame)?;
                *uf_counts.entry(decoded.uf()).or_default() += 1;
                decoded_count += 1;

                if !stats {
                    let value = serde_json::json!({
                        "path": file.display().to_string(),
                        "sample_rate_hz": sample_rate_hz,
                        "snr_db": parse_snr(&file),
                        "detection": detection,
                        "message": decoded,
                    });
                    print_json(value, pretty)?;
                }
            }
        } else {
            let frame = if sample_rate_hz == DEFAULT_UPLINK_SAMPLE_RATE_HZ {
                demodulate_snippet(&iq)?
            } else {
                demodulate_snippet_with_timing(&iq, &timing)?
            };
            let decoded = decode_frame(&frame)?;
            *uf_counts.entry(decoded.uf()).or_default() += 1;
            decoded_count += 1;

            if !stats {
                let value = serde_json::json!({
                    "path": file.display().to_string(),
                    "sample_rate_hz": sample_rate_hz,
                    "snr_db": parse_snr(&file),
                    "message": decoded,
                });
                print_json(value, pretty)?;
            }
        }
    }

    if stats {
        let value = serde_json::json!({
            "files": decoded_count,
            "sample_rate_hz": sample_rate_hz,
            "uf_counts": uf_counts,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    }

    Ok(())
}

fn print_help() {
    eprintln!("uplink1030: decode 1030 MHz Mode S uplink CF32 snippets\n");
    eprintln!("Usage:");
    eprintln!(
        "  cargo run -p uplink1030 -- [--stats] [--pretty] [--detect] [--sample-rate 20M] <file-or-directory> [...]"
    );
    eprintln!("\nExamples:");
    eprintln!("  cargo run -p uplink1030 -- samples --stats");
    eprintln!("  cargo run -p uplink1030 -- --detect samples --stats");
    eprintln!("  cargo run -p uplink1030 -- --sample-rate 20M --pretty samples/example.cf32");
    eprintln!("\nSample-rate rules:");
    eprintln!("  Minimum uplink sample rate is 8 MS/s.");
    eprintln!("  For now, the sample rate must be an integer multiple of 4 MS/s.");
}

fn print_json(value: serde_json::Value, pretty: bool) -> Result<(), serde_json::Error> {
    if pretty {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", serde_json::to_string(&value)?);
    }
    Ok(())
}

fn expand_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            collect_cf32(path, &mut files)?;
        } else {
            files.push(path.clone());
        }
    }
    files.sort();
    Ok(files)
}

fn collect_cf32(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_cf32(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "cf32") {
            files.push(path);
        }
    }
    Ok(())
}

fn parse_snr(path: &Path) -> Option<f32> {
    let name = path.file_name()?.to_str()?;
    let start = name.find("_snr-")? + 5;
    let rest = &name[start..];
    let end = rest.find(".cf32").unwrap_or(rest.len());
    rest[..end].parse().ok()
}
