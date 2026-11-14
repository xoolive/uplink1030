use clap::Parser;
use num_complex::Complex32;
use rs1030::dsp::{
    channel::ChannelFilter, receiver::Receiver, resample::RationalResampler, timing::UplinkTiming,
};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
mod activity;
mod catalogue;
mod output;
mod redis;
mod replay;
mod schema;
mod source;
mod timestamps;
mod tui;
use source::{Source, SourceError};
use timestamps::{system_now, ArrivalClock, EventContext};

#[derive(Parser, Debug)]
#[command(
    name = "uplink1030",
    about = "Native streaming Mode S 1030 MHz receiver (JSONL / interactive)"
)]
struct Args {
    /// IQ file/directory, '-' stdin, tcp://host:port or a Desperado SDR URI.
    #[arg(required_unless_present = "replay", conflicts_with = "replay")]
    source: Option<String>,
    /// Replay flat decoded JSONL instead of IQ (one recording per session).
    #[arg(long, value_name = "PATH", conflicts_with_all = ["source", "sample_rate", "decode_rate", "center_freq", "frequency_offset_hz", "format", "known_aircraft", "known_only", "seconds", "start_time", "notch_offset_hz", "debug", "no_p6_first", "no_dc_removal", "lowpass_hz", "min_confidence", "p6_min_amplitude"])]
    replay: Option<String>,
    /// Replay speed; 0 = unpaced, otherwise 0.125..1024 capture seconds per wall second.
    #[arg(long, default_value = "1", requires = "replay")]
    speed: f64,
    /// Trailing capture-time window for displayed rates and distinct targets.
    #[arg(long, default_value = "60")]
    activity_window: f64,
    /// Hide aircraft after this many capture seconds without events.
    #[arg(long, default_value = "60")]
    aircraft_expiry: f64,
    /// Hide radar-code populations after this many capture seconds without events.
    #[arg(long, default_value = "120")]
    radar_expiry: f64,
    /// Structured radar evidence catalogue; names/locations remain tentative.
    #[arg(long, value_name = "PATH")]
    catalogue: Option<String>,
    /// JSON stdout (the default); like jet1090/ship162 -v.
    #[arg(short, long, conflicts_with = "interactive")]
    verbose: bool,
    /// Interactive terminal table; q/Esc/Ctrl-C exits.
    #[arg(short,long,conflicts_with_all=["stats","pretty"])]
    interactive: bool,
    /// Append JSONL events to this file (created if missing), including with the TUI.
    #[arg(short, long, value_name = "PATH")]
    output: Option<String>,
    /// Publish each compact JSON event via Redis pub/sub, awaiting server acceptance.
    #[arg(long, value_name = "URL")]
    redis_url: Option<String>,
    /// Redis pub/sub topic (not a stream or list).
    #[arg(long, default_value = "uplink1030", requires = "redis_url")]
    redis_topic: String,
    /// Seconds between Redis reconnect attempts; 0 warns and skips failed publications.
    #[arg(long, default_value = "5", requires = "redis_url")]
    redis_retry_interval: u64,
    /// Aggregate JSON statistics only.
    #[arg(long)]
    stats: bool,
    /// Pretty JSON on stdout; file output stays JSONL.
    #[arg(long)]
    pretty: bool,
    /// Actual IQ sample rate. GQRX filenames and source URI rate are recognized.
    #[arg(long, alias = "rate")]
    sample_rate: Option<String>,
    /// Decoder sample rate after native resampling. Default: input rate when
    /// divisible by 4M, otherwise 8M (e.g. Airspy 10M -> 8M).
    #[arg(long)]
    decode_rate: Option<String>,
    /// Recording/device center frequency; live hardware is actually tuned here.
    #[arg(long, alias = "freq", alias = "center-frequency")]
    center_freq: Option<String>,
    /// Desired 1030 MHz RF minus recording center (Hz); overrides inferred shift.
    #[arg(long, allow_hyphen_values = true)]
    frequency_offset_hz: Option<f64>,
    /// Raw IQ format: cf32 (default), cu8, cs8, cs16.
    #[arg(long)]
    format: Option<String>,
    /// Independently observed ICAO24 list; annotate matches, retain unknown candidates.
    #[arg(long, value_name = "PATH")]
    known_aircraft: Option<String>,
    /// Exclude unknown addressed hypotheses (all-calls remain visible).
    #[arg(long, requires = "known_aircraft")]
    known_only: bool,
    /// Butterworth channel low-pass cutoff in Hz.
    #[arg(long, default_value = "3500000")]
    lowpass_hz: f64,
    /// Minimum mean DBPSK decision confidence (0..1).
    #[arg(long, default_value = "0.90")]
    min_confidence: f32,
    /// P6 rising-edge amplitude floor for normalized IQ (0 = relative threshold only).
    #[arg(long, default_value = "0.025")]
    p6_min_amplitude: f32,
    /// Use only the original P1/P2 detector, disabling supplementary P6 recovery.
    #[arg(long)]
    no_p6_first: bool,
    /// Disable per-input-chunk complex DC subtraction.
    #[arg(long)]
    no_dc_removal: bool,
    /// Stop after this many seconds of IQ, useful for capture diagnostics.
    #[arg(long)]
    seconds: Option<f64>,
    /// Explicit RFC3339 UTC start for a file with no timestamp in its name.
    #[arg(long)]
    start_time: Option<String>,
    /// Notch an interferer at this offset from the original recording center.
    #[arg(long, allow_hyphen_values = true)]
    notch_offset_hz: Option<f32>,
    /// Add demodulator diagnostics to each JSON message (sample position,
    /// detection method, symbol/sync quality, noise, emission time).
    #[arg(long)]
    debug: bool,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn collect_files(path: &std::path::Path, out: &mut Vec<String>) -> Result<(), SourceError> {
    for entry in std::fs::read_dir(path)? {
        let p = entry?.path();
        if p.is_dir() {
            collect_files(&p, out)?;
        } else if p.file_name().is_some_and(|n| {
            let n = n.to_string_lossy().to_ascii_lowercase();
            [".cf32", ".raw", ".cf32.zst", ".raw.zst"]
                .iter()
                .any(|s| n.ends_with(s))
        }) {
            out.push(p.to_string_lossy().into_owned());
        }
    }
    Ok(())
}
fn load_known(path: &str) -> Result<HashSet<u32>, SourceError> {
    let mut addresses = HashSet::new();
    for line in std::fs::read_to_string(shellexpand::tilde(path).as_ref())?.lines() {
        let token = line.split(',').next().unwrap_or("").trim();
        if token.len() == 6 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
            addresses.insert(u32::from_str_radix(token, 16)?);
        }
    }
    if addresses.is_empty() {
        return Err("ICAO whitelist contains no six-digit hexadecimal addresses".into());
    }
    Ok(addresses)
}

fn same_file(a: &str, b: &str) -> bool {
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        if a == b {
            return true;
        }
        #[cfg(unix)]
        if let (Ok(a), Ok(b)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            use std::os::unix::fs::MetadataExt;
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    false
}

async fn open_output(args: &Args, stopped: Arc<AtomicBool>) -> Result<output::Output, SourceError> {
    let publisher = if let Some(url) = &args.redis_url {
        Some(tokio::select! {
            biased;
            _ = redis::cancelled(&stopped) => return Err("Redis connection interrupted".into()),
            publisher = redis::Publisher::connect(url, args.redis_topic.clone(), std::time::Duration::from_secs(args.redis_retry_interval)) => publisher?,
        })
    } else {
        None
    };
    let mut output = output::Output::new(
        args.interactive,
        args.output.as_deref(),
        args.pretty,
        args.stats,
    )?;
    output.attach_redis(publisher, stopped);
    Ok(output)
}

async fn run() -> Result<(), SourceError> {
    let args = Args::parse();
    let settings = activity::Settings {
        window: args.activity_window,
        aircraft_ttl: args.aircraft_expiry,
        radar_ttl: args.radar_expiry,
    };
    settings.validate()?;
    let sites = args
        .catalogue
        .as_deref()
        .map(catalogue::load)
        .transpose()?
        .unwrap_or_default();
    if let (Some(catalogue), Some(output)) = (&args.catalogue, &args.output) {
        if same_file(shellexpand::tilde(catalogue).as_ref(), output) {
            return Err("output path would overwrite the radar catalogue".into());
        }
    }
    if !args.speed.is_finite() || (args.speed != 0. && !(0.125..=1024.).contains(&args.speed)) {
        return Err("--speed must be 0 (unpaced) or between 0.125 and 1024".into());
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let signal = stopped.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal.store(true, Ordering::Relaxed);
        }
    });
    if let Some(path) = &args.replay {
        let path = shellexpand::tilde(path).into_owned();
        if args
            .output
            .as_ref()
            .is_some_and(|out| same_file(&path, out))
        {
            return Err("output path would overwrite the JSONL replay source".into());
        }
        // Validate input readability and its first event before creating/truncating output.
        replay::Reader::open(&path)?.next()?;
        let mut output = open_output(&args, stopped.clone()).await?;
        output.configure(settings, sites, true, args.speed);
        replay::run(&path, &mut output, &stopped)?;
        let summary = serde_json::json!({"replay":path,"decoded_messages":output.decoded,"uf_counts":output.counts,"active_radar_codes":output.model.radars.len(),"active_aircraft_candidates":output.model.aircraft.len(),"unattributed_messages":output.model.unattributed,"addressed_messages_are_candidates":true});
        output.finish()?;
        drop(output);
        if args.stats {
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        return Ok(());
    }
    if !args.min_confidence.is_finite()
        || !(0.0..=1.0).contains(&args.min_confidence)
        || !args.p6_min_amplitude.is_finite()
        || args.p6_min_amplitude < 0.
    {
        return Err("invalid confidence or P6 amplitude threshold".into());
    }
    if args.seconds.is_some_and(|s| !s.is_finite() || s <= 0.) {
        return Err("--seconds must be positive and finite".into());
    }
    let known = args.known_aircraft.as_deref().map(load_known).transpose()?;
    let rate = args
        .sample_rate
        .as_deref()
        .map(desperado::parse_si_value::<u32>)
        .transpose()?;
    let requested_decode_rate = args
        .decode_rate
        .as_deref()
        .map(desperado::parse_si_value::<u32>)
        .transpose()?;
    let freq = args
        .center_freq
        .as_deref()
        .map(desperado::parse_si_value::<u32>)
        .transpose()?;
    let format = args
        .format
        .as_deref()
        .map(str::parse::<desperado::IqFormat>)
        .transpose()?;
    let explicit_start = args
        .start_time
        .as_deref()
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()?
        .map(|t| t.timestamp() as f64 + t.timestamp_subsec_nanos() as f64 / 1e9);
    let mut specs = vec![];
    let source_arg = args.source.as_deref().ok_or("missing IQ source")?;
    if std::path::Path::new(source_arg).is_dir() {
        collect_files(std::path::Path::new(source_arg), &mut specs)?;
        specs.sort();
    } else {
        specs.push(source_arg.to_owned());
    }
    if specs.is_empty() {
        return Err("no raw IQ files found".into());
    }
    if let Some(path) = &args.output {
        if let Some(known) = &args.known_aircraft {
            if same_file(shellexpand::tilde(known).as_ref(), path) {
                return Err("output path would overwrite the aircraft whitelist".into());
            }
        }
        for spec in &specs {
            let s: Source = spec.parse()?;
            // Reject an obvious accidental overwrite; canonicalize existing paths
            // before opening the output file. Includes symlink aliases.
            if let Some(input) = s.file_path() {
                if same_file(input, path) {
                    return Err("output path would overwrite the IQ source".into());
                }
            }
        }
    }
    let mut output = open_output(&args, stopped.clone()).await?;
    output.configure(settings, sites, false, 1.);
    let mut total_samples = 0usize;
    let mut total_decoder_samples = 0usize;
    let mut resampled_sources = 0usize;
    let mut source_count = 0;
    let mut quit = false;
    for spec in &specs {
        // Separate recordings can use unrelated UTC/relative clocks. Keep JSON totals,
        // but never mix their rolling activity or scan histories.
        if source_count > 0 {
            output.reset_view();
        }
        let mut source: Source = spec.parse()?;
        source.configure(rate, freq, format);
        if source.sample_rate < rs1030::dsp::timing::MIN_UPLINK_SAMPLE_RATE_HZ {
            return Err("actual IQ rate must be at least 8 MS/s; upsampling cannot restore missing bandwidth".into());
        }
        let decode_rate = requested_decode_rate.unwrap_or_else(|| {
            if source.sample_rate.is_multiple_of(4_000_000) {
                source.sample_rate
            } else {
                8_000_000
            }
        });
        let timing = UplinkTiming::from_sample_rate(decode_rate)?;
        if args.lowpass_hz >= decode_rate as f64 / 2. {
            return Err("channel cutoff must fit below decoder-rate Nyquist".into());
        }
        let mut resampler = if decode_rate != source.sample_rate {
            resampled_sources += 1;
            Some(RationalResampler::new(source.sample_rate, decode_rate)?)
        } else {
            None
        };
        let offset = args
            .frequency_offset_hz
            .unwrap_or(1_030_000_000. - source.center_frequency as f64);
        let mut channel = ChannelFilter::new(
            source.sample_rate,
            offset,
            args.lowpass_hz,
            !args.no_dc_removal,
        )?;
        let mut receiver = Receiver::new(
            timing,
            known.clone(),
            args.min_confidence,
            args.p6_min_amplitude,
            !args.no_p6_first,
        )
        .with_known_only(args.known_only);
        let mut notch = args
            .notch_offset_hz
            .map(|f| NotchFilter::new(f, source.sample_rate))
            .transpose()?;
        let start = explicit_start
            .or(source.start_timestamp)
            .or_else(|| source.is_live().then(system_now));
        let sample_rate = source.sample_rate;
        // Retain anchors through frame lookahead, timing searches and centered
        // FIR lookahead. Pruning happens after event emission, never before it.
        let retain_input = ((timing.min_samples_long() + 32 * timing.samples_per_bit) as u128
            * sample_rate as u128)
            .div_ceil(decode_rate as u128) as usize
            + 2 * resampler
                .as_ref()
                .map_or(0, RationalResampler::input_lookahead);
        let mut context = EventContext {
            decode_rate,
            start,
            debug: args.debug,
            known: known.is_some(),
            arrivals: source
                .is_live()
                .then(|| ArrivalClock::new(sample_rate, retain_input)),
        };
        let max_samples = args
            .seconds
            .map(|seconds| (seconds * sample_rate as f64) as usize);
        let mut samples = 0usize;
        source_count += 1;
        if !args.interactive {
            eprintln!(
                "{spec}: {sample_rate} input samples/s, {decode_rate} decoder samples/s, center {} Hz, signal offset {offset} Hz, {}",
                source.center_frequency,
                if args.known_only {
                    "known-only ICAO filtering enabled"
                } else if known.is_some() {
                    "known ICAO annotation enabled; unknown candidates retained"
                } else {
                    "blind candidate addresses (no independent AP checksum)"
                }
            );
        }
        let live = source.is_live();
        source::for_each_sample_chunk(&source, |mut chunk| {
            let arrival_time = system_now();
            let read_len = chunk.len();
            if stopped.load(Ordering::Relaxed)
                || !output.poll(spec, samples as f64 / sample_rate as f64, false)?
            {
                quit = true;
                return Ok(false);
            }
            if let Some(max) = max_samples {
                chunk.truncate(max.saturating_sub(samples));
            }
            if let Some(clock) = &mut context.arrivals {
                clock.record(samples, read_len, chunk.len(), arrival_time);
            }
            if let Some(notch) = &mut notch {
                notch.apply(&mut chunk);
            }
            channel.apply(&mut chunk);
            samples += chunk.len();
            let converted;
            let decoding = if let Some(resampler) = &mut resampler {
                converted = resampler.push(&chunk, false);
                &converted[..]
            } else {
                &chunk[..]
            };
            total_decoder_samples += decoding.len();
            for r in receiver.push(decoding, false) {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                emit(&mut output, r, &context)?;
            }
            quit |= stopped.load(Ordering::Relaxed);
            if let Some(clock) = &mut context.arrivals {
                clock.prune(samples);
            }
            output.advance(if live {
                arrival_time
            } else {
                start.unwrap_or(0.) + samples as f64 / sample_rate as f64
            });
            Ok(!quit && max_samples.is_none_or(|max| samples < max))
        })
        .await?;
        if !quit {
            if let Some(resampler) = &mut resampler {
                let tail = resampler.push(&[], true);
                total_decoder_samples += tail.len();
                for r in receiver.push(&tail, false) {
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    emit(&mut output, r, &context)?;
                }
            }
            for r in receiver.push(&[], true) {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                emit(&mut output, r, &context)?;
            }
        }
        quit |= stopped.load(Ordering::Relaxed);
        total_samples += samples;
        output.flush()?;
        if quit {
            break;
        }
        if output.is_interactive() && source_count == specs.len() {
            while !stopped.load(Ordering::Relaxed)
                && output.poll(spec, samples as f64 / sample_rate as f64, true)?
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    let mut summary = serde_json::json!({"sources_processed":source_count,"samples_processed":total_samples,"decoded_messages":output.decoded,"uf_counts":output.counts,"addressed_messages_are_candidates":true});
    if resampled_sources > 0 {
        summary["decoder_samples_processed"] = total_decoder_samples.into();
        summary["resampled_sources"] = resampled_sources.into();
    }
    output.finish()?;
    drop(output);
    if args.stats {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    }
    Ok(())
}

fn emit(
    out: &mut output::Output,
    r: rs1030::dsp::receiver::Recovered,
    context: &EventContext,
) -> Result<(), SourceError> {
    use serde_json::{Map, Value};
    let round = |x: f64, digits: i32| {
        let scale = 10f64.powi(digits);
        (x * scale).round() / scale
    };
    let p6 = r.detection.p6_sample;
    let seconds = p6 as f64 / context.decode_rate as f64;
    // jet1090 convention: `timestamp` is the reception time. Live sources use
    // host chunk arrival (the sample clock drifts with dropped samples); files
    // use their UTC start plus sample position, else seconds since file start.
    let timestamp = context
        .reception_time(p6)
        .or(context.start.map(|start| start + seconds))
        .unwrap_or(seconds);
    let mut value = Map::new();
    value.insert("timestamp".into(), timestamp.into());
    if let Value::Object(message) = serde_json::to_value(&r.message)? {
        value.extend(message);
    }
    if context.known && r.address_validation != "all_call_address" {
        value.insert(
            "known".into(),
            (r.address_validation == "known_address_match").into(),
        );
    }
    let power = r.detection.signal_power as f64;
    if power > 0. {
        value.insert("rssi".into(), round(10. * power.log10(), 2).into());
    }
    value.insert(
        "confidence".into(),
        round(r.decision_confidence as f64, 3).into(),
    );
    if context.debug {
        let debug = serde_json::json!({
            "seconds": seconds,
            "p6_sample": p6,
            "detection_method": r.detection_method,
            "demodulation_offset_samples": r.demodulation_offset_samples,
            "phase_rotation_rad": r.phase_rotation_rad,
            "symbol_coherence": r.symbol_coherence,
            "sync_correlation": r.sync_correlation,
            "signal_power": r.detection.signal_power,
            "noise_mag": r.detection.noise_mag,
            "threshold_mag": r.detection.threshold_mag,
            "decoded_at": system_now(),
        });
        if let Value::Object(debug) = debug {
            value.extend(debug);
        }
    }
    schema::normalize_output(&mut value);
    out.message(Value::Object(value))
}

struct NotchFilter {
    coefficient: f32,
    gain: f32,
    previous_1: Complex32,
    previous_2: Complex32,
}
impl NotchFilter {
    fn new(offset: f32, rate: u32) -> Result<Self, SourceError> {
        let c = -2. * (std::f32::consts::TAU * offset / rate as f32).cos();
        if !offset.is_finite() || (2. + c).abs() < 1e-4 {
            return Err("notch frequency too close to DC; use the DC remover instead".into());
        }
        Ok(Self {
            coefficient: c,
            gain: 1. / (2. + c),
            previous_1: Complex32::default(),
            previous_2: Complex32::default(),
        })
    }
    fn apply(&mut self, iq: &mut [Complex32]) {
        for s in iq {
            let x = *s;
            *s = (x + self.coefficient * self.previous_1 + self.previous_2) * self.gain;
            self.previous_2 = self.previous_1;
            self.previous_1 = x;
        }
    }
}
