use clap::Parser;
use rs1030::decode_frame;
use rs1030::dsp::demodulator::demodulate_detection_with_timing;
use rs1030::dsp::timing::UplinkTiming;
use rs1030::Detector;
use std::collections::BTreeMap;
use std::str::FromStr;

mod source;
use source::{for_each_sample_chunk, load_samples, Source};

#[derive(Parser, Debug)]
#[command(name = "uplink1030")]
#[command(about = "Mode S 1030 MHz uplink decoder", long_about = None)]
struct Args {
    /// Source specification (file://, hackrf://, soapy://)
    /// Can be:
    ///   - Plain path: samples/file.cf32
    ///   - file:// URI: file:///path/to/file.cf32 or file://~/file.cf32
    ///   - Directory: samples (will process all .cf32 files recursively)
    ///   - hackrf:// device: hackrf://?sample_rate=8000000
    ///   - soapy:// device: soapy://driver=rtlsdr?sample_rate=8000000
    #[arg(value_name = "SOURCE")]
    source: String,

    /// Show statistics only (no decoded messages)
    #[arg(long)]
    stats: bool,

    /// Pretty-print JSON output
    #[arg(long)]
    pretty: bool,

    /// Override sample rate (Hz, supports SI units like 8M, 20M)
    #[arg(long)]
    sample_rate: Option<String>,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn collect_cf32_files(dir: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    collect_cf32_recursive(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_cf32_recursive(
    dir: &str,
    files: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_cf32_recursive(path.to_str().unwrap(), files)?;
        } else if path.extension().is_some_and(|ext| ext == "cf32") {
            files.push(path.display().to_string());
        }
    }
    Ok(())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Check if source is a directory - if so, collect CF32 files
    let source_specs = if std::path::Path::new(&args.source).is_dir() {
        let files = collect_cf32_files(&args.source)?;
        eprintln!("Found {} .cf32 files in {}", files.len(), args.source);
        files
    } else {
        vec![args.source.clone()]
    };

    let mut uf_counts: BTreeMap<u8, usize> = BTreeMap::new();
    let mut decoded_count = 0usize;
    let mut total_files = 0usize;

    for source_spec in source_specs {
        total_files += 1;

        // Parse source
        let source = Source::from_str(&source_spec)
            .map_err(|e| format!("Invalid source '{}': {}", source_spec, e))?;

        // Override sample rate if provided
        let sample_rate = if let Some(ref rate_str) = args.sample_rate {
            desperado::parse_si_value::<u32>(rate_str)
                .map_err(|e| format!("Invalid sample rate: {}", e))?
        } else {
            source.sample_rate()
        };

        let timing = UplinkTiming::from_sample_rate(sample_rate)
            .map_err(|e| format!("Invalid sample rate: {}", e))?;

        if source.is_continuous() {
            process_continuous_source(
                &source,
                &source_spec,
                sample_rate,
                timing,
                args.stats,
                args.pretty,
                &mut uf_counts,
                &mut decoded_count,
            )
            .await?;
        } else {
            // Files are finite, but still use the detector path. It works for both
            // tightly-cut one-frame snippets and longer recordings.
            let iq = match load_samples(&source).await {
                Ok(samples) => samples,
                Err(e) => {
                    eprintln!("Warning: Failed to load {}: {}", source_spec, e);
                    continue;
                }
            };
            process_detections(
                &iq,
                0,
                &source_spec,
                sample_rate,
                timing,
                args.stats,
                args.pretty,
                &mut uf_counts,
                &mut decoded_count,
            )?;
        }
    }

    if args.stats {
        let value = serde_json::json!({
            "files_processed": total_files,
            "decoded_messages": decoded_count,
            "uf_counts": uf_counts,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    }

    Ok(())
}

fn process_detections(
    iq: &[num_complex::Complex32],
    base_idx: usize,
    source_spec: &str,
    sample_rate: u32,
    timing: UplinkTiming,
    stats: bool,
    pretty: bool,
    uf_counts: &mut BTreeMap<u8, usize>,
    decoded_count: &mut usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let detector = Detector::with_timing(3.0, false, timing);
    for detection in detector.detect(iq, base_idx) {
        let p6_local = detection.p6_sample.saturating_sub(base_idx);
        let frame = demodulate_detection_with_timing(iq, p6_local, detection.num_bits, &timing)?;
        let decoded = decode_frame(&frame)?;
        *uf_counts.entry(decoded.uf()).or_default() += 1;
        *decoded_count += 1;

        if !stats {
            let value = serde_json::json!({
                "source": source_spec,
                "sample_rate_hz": sample_rate,
                "detection": detection,
                "message": decoded,
            });
            print_json(value, pretty)?;
        }
    }
    Ok(())
}

async fn process_continuous_source(
    source: &Source,
    source_spec: &str,
    sample_rate: u32,
    timing: UplinkTiming,
    stats: bool,
    pretty: bool,
    uf_counts: &mut BTreeMap<u8, usize>,
    decoded_count: &mut usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let detector = Detector::with_timing(3.0, false, timing);
    let mut buffer = Vec::new();
    let mut base_idx = 0usize;
    let mut last_detection_p1 = None::<usize>;
    let overlap =
        timing.min_samples_long() + timing.p6_offset_samples + timing.p6_guard_samples + 32;

    for_each_sample_chunk(source, |chunk| {
        buffer.extend(chunk);

        for detection in detector.detect(&buffer, base_idx) {
            if last_detection_p1.is_some_and(|last| detection.p1_sample <= last) {
                continue;
            }

            let p6_local = detection.p6_sample.saturating_sub(base_idx);
            let frame =
                demodulate_detection_with_timing(&buffer, p6_local, detection.num_bits, &timing)
                    .map_err(|e| source::SourceError::ParseError(e.to_string()))?;
            let decoded =
                decode_frame(&frame).map_err(|e| source::SourceError::ParseError(e.to_string()))?;

            *uf_counts.entry(decoded.uf()).or_default() += 1;
            *decoded_count += 1;
            last_detection_p1 = Some(detection.p1_sample);

            if !stats {
                let value = serde_json::json!({
                    "source": source_spec,
                    "sample_rate_hz": sample_rate,
                    "detection": detection,
                    "message": decoded,
                });
                print_json(value, pretty)
                    .map_err(|e| source::SourceError::ParseError(e.to_string()))?;
            }
        }

        if buffer.len() > overlap {
            let drain_len = buffer.len() - overlap;
            buffer.drain(..drain_len);
            base_idx += drain_len;
        }

        Ok(())
    })
    .await?;

    Ok(())
}

fn print_json(value: serde_json::Value, pretty: bool) -> Result<(), serde_json::Error> {
    if pretty {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", serde_json::to_string(&value)?);
    }
    Ok(())
}
