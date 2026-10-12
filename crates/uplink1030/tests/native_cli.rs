//! Complete CLI pipeline tests; no RF hardware or Python required.
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(ext: &str) -> Self {
        Self(std::env::temp_dir().join(format!(
            "uplink-native-{}-{}.{ext}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn waveform() -> Vec<u8> {
    let frame = [0x20u8, 0xa9, 0x30, 0x40, 0xbf, 0xa0, 0xad];
    let mut iq = vec![(0f64, 0f64); 2000];
    let p1 = 200;
    let p6 = p1 + 56;
    let start = p6 + 28;
    iq[p1..p1 + 13].fill((0.4, 0.));
    iq[p1 + 32..p1 + 45].fill((0.4, 0.));
    let mut sign = 1.;
    for (i, sample) in iq.iter_mut().enumerate().take(start).skip(p6) {
        if i == p6 + 20 {
            sign = -sign;
        }
        *sample = (sign * 0.4, 0.);
    }
    for bit in 0..56 {
        if frame[bit / 8] & (1 << (7 - bit % 8)) != 0 {
            sign = -sign;
        }
        for n in 0..4 {
            iq[start + bit * 4 + n] = (sign * 0.4, 0.);
        }
    }
    let mut bytes = vec![];
    for (n, (i, q)) in iq.into_iter().enumerate() {
        let phase = -std::f64::consts::TAU * n as f64 / 8.;
        let re = (i * phase.cos() - q * phase.sin() + 0.08) as f32;
        let im = (i * phase.sin() + q * phase.cos() - 0.03) as f32;
        bytes.extend_from_slice(&re.to_le_bytes());
        bytes.extend_from_slice(&im.to_le_bytes());
    }
    bytes
}
fn decoded(bytes: &[u8]) -> Vec<serde_json::Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
fn assert_query(rows: &[serde_json::Value]) {
    assert!(
        rows.iter()
            .any(|r| r["message"]["raw"] == "20a93040bfa0ad" && r["message"]["icao24"] == "39bda4"),
        "{rows:?}"
    );
    for r in rows {
        assert_eq!(
            r["detection"]["num_bits"],
            r["message"]["raw"].as_str().unwrap().len() * 4
        );
    }
}
#[test]
fn known_list_annotates_unknowns_and_known_only_filters() {
    let file = Fixture::new("cf32");
    let list = Fixture::new("txt");
    std::fs::write(&file.0, waveform()).unwrap();
    std::fs::write(&list.0, "deadbe\n").unwrap();
    let run = |known: bool, only: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_uplink1030"));
        command
            .arg(&file.0)
            .args(["--sample-rate", "16M", "--center-freq", "1032M"]);
        if known {
            command.arg("--known-aircraft").arg(&list.0);
        }
        if only {
            command.arg("--known-only");
        }
        command.output().unwrap()
    };
    let blind = run(false, false);
    let annotated = run(true, false);
    assert!(blind.status.success() && annotated.status.success());
    assert_eq!(blind.stdout, annotated.stdout);
    assert_query(&decoded(&annotated.stdout));
    assert!(decoded(&annotated.stdout)
        .iter()
        .all(|r| r["address_validation"] == "unverified"));
    let filtered = run(true, true);
    assert!(filtered.status.success());
    assert!(filtered.stdout.is_empty());
    let invalid = run(false, true);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("--known-aircraft"));
    std::fs::write(&list.0, "39bda4\n").unwrap();
    for only in [false, true] {
        let matching = run(true, only);
        assert!(matching.status.success());
        let rows = decoded(&matching.stdout);
        assert_query(&rows);
        assert!(rows.iter().any(|r| r["message"]["raw"] == "20a93040bfa0ad"
            && r["address_validation"] == "known_address_match"
            && r["validation"] == "candidate"));
    }
}

#[test]
fn fractional_input_rate_resamples_and_preserves_time_coordinates() {
    use num_complex::Complex32;
    use rs1030::dsp::resample::RationalResampler;
    let raw = waveform();
    let dc = Complex32::new(0.08, -0.03);
    let input: Vec<_> = raw
        .chunks_exact(8)
        .enumerate()
        .map(|(n, b)| {
            let x = Complex32::new(
                f32::from_le_bytes(b[..4].try_into().unwrap()),
                f32::from_le_bytes(b[4..].try_into().unwrap()),
            );
            (x - dc) * Complex32::from_polar(1., std::f32::consts::TAU * n as f32 / 16.) + dc
        })
        .collect();
    let mut converter = RationalResampler::new(16_000_000, 10_000_000).unwrap();
    let ten = converter.push(&input, true);
    let bytes: Vec<_> = ten
        .iter()
        .flat_map(|s| s.re.to_le_bytes().into_iter().chain(s.im.to_le_bytes()))
        .collect();
    let file = Fixture::new("cf32");
    std::fs::write(&file.0, bytes).unwrap();
    for decoder_rate in ["8M", "12M"] {
        let result = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
            .arg(&file.0)
            .args([
                "--sample-rate",
                "10M",
                "--center-freq",
                "1031M",
                "--decode-rate",
                decoder_rate,
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let rows = decoded(&result.stdout);
        assert_query(&rows);
        for row in rows {
            let decode = if decoder_rate == "8M" {
                8_000_000.
            } else {
                12_000_000.
            };
            assert_eq!(row["sample_rate_hz"], 10_000_000);
            assert_eq!(row["decode_sample_rate_hz"].as_f64().unwrap(), decode);
            let seconds = row["seconds"].as_f64().unwrap();
            assert!(
                (seconds - row["detection"]["p6_sample"].as_f64().unwrap() / decode).abs() < 1e-12
            );
            assert!(
                (row["input_p6_sample"].as_f64().unwrap() - seconds * 10_000_000.).abs() < 1e-9
            );
        }
    }
    let automatic = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .arg(&file.0)
        .args(["--sample-rate", "10M", "--center-freq", "1031M"])
        .output()
        .unwrap();
    assert!(automatic.status.success());
    assert_query(&decoded(&automatic.stdout));
    assert!(decoded(&automatic.stdout)
        .iter()
        .all(|r| r["decode_sample_rate_hz"] == 8_000_000));
}

#[test]
fn compressed_file_and_stdin_share_native_pipeline() {
    let file = Fixture::new("raw.zst");
    std::fs::write(
        &file.0,
        zstd::stream::encode_all(waveform().as_slice(), 1).unwrap(),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .arg(&file.0)
        .args(["--sample-rate", "16M", "--center-freq", "1032M"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_query(&decoded(&result.stdout));
    let mut child = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .args(["-", "--sample-rate", "16M", "--center-freq", "1032M"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&waveform()).unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_query(&decoded(&result.stdout));
}
#[test]
fn tcp_iq_and_output_file() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let sender = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.write_all(&waveform()).unwrap();
    });
    let file = Fixture::new("jsonl");
    let result = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .arg(format!("tcp://{address}?rate=16M&freq=1032M"))
        .arg("--output")
        .arg(&file.0)
        .output()
        .unwrap();
    sender.join().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_query(&decoded(&std::fs::read(&file.0).unwrap()));
    assert_query(&decoded(&result.stdout));
}
#[test]
fn protects_whitelist_and_existing_output_on_tui_error() {
    let input = Fixture::new("cf32");
    std::fs::write(&input.0, waveform()).unwrap();
    let known = Fixture::new("txt");
    std::fs::write(&known.0, "39bda4\n").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .arg(&input.0)
        .arg("--known-aircraft")
        .arg(&known.0)
        .arg("-o")
        .arg(&known.0)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read_to_string(&known.0).unwrap(), "39bda4\n");
    let output = Fixture::new("jsonl");
    std::fs::write(&output.0, "unchanged").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .arg(&input.0)
        .arg("-i")
        .arg("-o")
        .arg(&output.0)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read_to_string(&output.0).unwrap(), "unchanged");
    #[cfg(unix)]
    {
        let alias = Fixture::new("cf32");
        std::fs::hard_link(&input.0, &alias.0).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
            .arg(&input.0)
            .arg("-o")
            .arg(&alias.0)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert_eq!(std::fs::read(&input.0).unwrap(), waveform());
    }
}

#[test]
fn rejects_output_overwriting_file_uri_and_nonterminal_tui() {
    let file = Fixture::new("cf32");
    let original = waveform();
    std::fs::write(&file.0, &original).unwrap();
    let uri = url::Url::from_file_path(&file.0).unwrap().to_string();
    let result = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .arg(uri)
        .arg("--output")
        .arg(&file.0)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read(&file.0).unwrap(), original);
    let result = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .arg(&file.0)
        .arg("--interactive")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("requires a terminal"));
}
