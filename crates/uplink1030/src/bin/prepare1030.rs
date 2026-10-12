//! Prepare a short, centered, anti-aliased IQ fixture without Python or a huge
//! uncompressed intermediate. Output is CF32 Zstandard; existing files refused.
#[path = "../source.rs"]
mod source;
use clap::Parser;
use num_complex::{Complex32, Complex64};
use rs1030::dsp::resample::RationalResampler;
use source::{Source, SourceError};
use std::io::Write;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    input: String,
    #[arg(long)]
    output: String,
    #[arg(long)]
    output_rate: String,
    #[arg(long, default_value = "6")]
    seconds: f64,
    #[arg(long)]
    sample_rate: Option<String>,
    #[arg(long)]
    center_freq: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), SourceError> {
    let args = Args::parse();
    if !args.seconds.is_finite() || args.seconds <= 0. {
        return Err("seconds must be positive".into());
    }
    let mut source: Source = args.input.parse()?;
    source.configure(
        args.sample_rate
            .as_deref()
            .map(desperado::parse_si_value::<u32>)
            .transpose()?,
        args.center_freq
            .as_deref()
            .map(desperado::parse_si_value::<u32>)
            .transpose()?,
        None,
    );
    let output_rate = desperado::parse_si_value::<u32>(&args.output_rate)?;
    if source.sample_rate < 8_000_000 || output_rate < 8_000_000 {
        return Err("uplink fixtures require actual rates >=8 MS/s".into());
    }
    let offset = 1_030_000_000. - source.center_frequency as f64;
    if offset.abs() + 3_500_000. > source.sample_rate as f64 / 2. {
        return Err("1030 MHz channel does not fit in original input bandwidth".into());
    }
    let mut converter = if source.sample_rate != output_rate {
        Some(RationalResampler::new(source.sample_rate, output_rate)?)
    } else {
        None
    };
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args.output)?;
    let mut encoder = zstd::stream::write::Encoder::new(std::io::BufWriter::new(file), 3)?;
    let max_samples = (args.seconds * source.sample_rate as f64) as usize;
    let mut input_samples = 0usize;
    let mut output_samples = 0usize;
    let mut oscillator = Complex64::new(1., 0.);
    let step = Complex64::from_polar(
        1.,
        -std::f64::consts::TAU * offset / source.sample_rate as f64,
    );
    let mut count = 0usize;
    let mut bytes = Vec::new();
    source::for_each_sample_chunk(&source, |mut iq| {
        iq.truncate(max_samples.saturating_sub(input_samples));
        if iq.is_empty() {
            return Ok(input_samples < max_samples);
        }
        // Original ADC DC must be removed BEFORE translating: afterwards it
        // would be an off-center tone inside the exported channel.
        let mean = iq.iter().fold(Complex64::default(), |sum, x| {
            sum + Complex64::new(x.re as f64, x.im as f64)
        }) / iq.len() as f64;
        for sample in &mut iq {
            let mut value = Complex64::new(sample.re as f64, sample.im as f64) - mean;
            if offset != 0. {
                value *= oscillator;
                oscillator *= step;
                count += 1;
                if count.is_multiple_of(4096) {
                    oscillator /= oscillator.norm();
                }
            }
            *sample = Complex32::new(value.re as f32, value.im as f32);
        }
        input_samples += iq.len();
        let converted;
        let samples = if let Some(converter) = &mut converter {
            converted = converter.push(&iq, false);
            &converted[..]
        } else {
            &iq[..]
        };
        bytes.clear();
        for sample in samples {
            bytes.extend(sample.re.to_le_bytes());
            bytes.extend(sample.im.to_le_bytes());
        }
        encoder.write_all(&bytes)?;
        output_samples += samples.len();
        Ok(input_samples < max_samples)
    })
    .await?;
    if let Some(converter) = &mut converter {
        let tail = converter.push(&[], true);
        bytes.clear();
        for sample in &tail {
            bytes.extend(sample.re.to_le_bytes());
            bytes.extend(sample.im.to_le_bytes());
        }
        encoder.write_all(&bytes)?;
        output_samples += tail.len();
    }
    encoder.finish()?.flush()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "input":args.input,"input_file":source.file_path(),"input_is_live":source.is_live(),"output":args.output,"input_sample_rate_hz":source.sample_rate,
            "input_center_hz":source.center_frequency,"input_samples":input_samples,
            "output_sample_rate_hz":output_rate,"output_center_hz":1030000000u32,"output_samples":output_samples,
            "duration_seconds":input_samples as f64/source.sample_rate as f64,
            "start_timestamp":source.start_timestamp,"dc_removed_before_translation":true,
            "resampler":"centered 81-tap Blackman windowed sinc; rational phase; endpoint zero padding"
        }))?
    );
    Ok(())
}
