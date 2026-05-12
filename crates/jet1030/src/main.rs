use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use rs1030::decode_frame;
use rs1030::demodulate_snippet;
use rs1030::dsp::demodulator::demodulate_detection;
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
    let mut paths = Vec::new();

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--stats" => stats = true,
            "--pretty" => pretty = true,
            "--detect" => detect = true,
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }

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
            let detector = Detector::default();
            for detection in detector.detect(&iq, 0) {
                let frame = demodulate_detection(&iq, detection.p6_sample, detection.num_bits)?;
                let decoded = decode_frame(&frame)?;
                *uf_counts.entry(decoded.uf).or_default() += 1;
                decoded_count += 1;

                if !stats {
                    let value = serde_json::json!({
                        "path": file.display().to_string(),
                        "snr_db": parse_snr(&file),
                        "detection": detection,
                        "message": decoded,
                    });
                    print_json(value, pretty)?;
                }
            }
        } else {
            let frame = demodulate_snippet(&iq)?;
            let decoded = decode_frame(&frame)?;
            *uf_counts.entry(decoded.uf).or_default() += 1;
            decoded_count += 1;

            if !stats {
                let value = serde_json::json!({
                    "path": file.display().to_string(),
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
            "uf_counts": uf_counts,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    }

    Ok(())
}

fn print_help() {
    eprintln!("jet1030: decode 1030 MHz Mode S uplink CF32 snippets\n");
    eprintln!("Usage:");
    eprintln!(
        "  cargo run -p jet1030 -- [--stats] [--pretty] [--detect] <file-or-directory> [...]"
    );
    eprintln!("\nExamples:");
    eprintln!("  cargo run -p jet1030 -- samples --stats");
    eprintln!("  cargo run -p jet1030 -- --detect samples --stats");
    eprintln!("  cargo run -p jet1030 -- --pretty samples/example.cf32");
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
