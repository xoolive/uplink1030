//! Replay integration tests exercise the real executable without an SDR or terminal.
use serde_json::{json, Value};
use std::{
    fs,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(contents: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "uplink-replay-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&p, contents).unwrap();
        Self(p)
    }
    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn event(t: f64, uf: &str, address: &str) -> Value {
    json!({"timestamp":t,"frame":"20970000b1c8a9","uf":uf,"icao24":address,"interrogator":"II3","bds":"20","confidence":0.95,"rssi":-30.})
}
fn recording() -> Fixture {
    Fixture::new(
        &[
            event(100., "11", "ffffff"),
            event(100.005, "4", "aaaaaa"),
            event(100.02, "5", "bbbbbb"),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n"),
    )
}
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn paced_and_unpaced_replays_produce_the_same_capture_time_summary() {
    let f = recording();
    let mut expected = None;
    for speed in ["0", "1", "10"] {
        let out = run(&["--replay", f.path(), "--speed", speed, "--stats"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["decoded_messages"], 3);
        assert_eq!(v["active_radar_codes"], 1);
        assert_eq!(v["active_aircraft_candidates"], 2);
        if let Some(e) = &expected {
            assert_eq!(&v, e);
        } else {
            expected = Some(v);
        }
    }
}
#[test]
fn replay_preserves_all_json_values_and_protects_input_aliases() {
    let f = recording();
    let outpath = Fixture::new("");
    fs::remove_file(outpath.path()).unwrap();
    let out = run(&[
        "--replay",
        f.path(),
        "--speed",
        "0",
        "--stats",
        "-o",
        outpath.path(),
    ]);
    assert!(out.status.success());
    let original = fs::read_to_string(f.path()).unwrap();
    let new = fs::read_to_string(outpath.path()).unwrap();
    assert_eq!(
        original
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect::<Vec<_>>(),
        new.lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect::<Vec<_>>()
    );
    let alias = Fixture::new("");
    fs::remove_file(alias.path()).unwrap();
    fs::hard_link(f.path(), alias.path()).unwrap();
    let out = run(&["--replay", f.path(), "--speed", "0", "-o", alias.path()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("overwrite"));
    assert_eq!(fs::read_to_string(f.path()).unwrap(), original);
}
#[test]
fn output_appends_across_runs_and_separates_unterminated_last_records() {
    let input = recording();
    let empty = Fixture::new("");
    for ending in ["", "\n"] {
        let prefix = format!("{{ \"existing\": true }}{ending}");
        let output = Fixture::new(&prefix);
        let args = |path| {
            vec![
                "--replay",
                path,
                "--speed",
                "0",
                "--stats",
                "-o",
                output.path(),
            ]
        };
        assert!(run(&args(empty.path())).status.success());
        assert_eq!(fs::read_to_string(output.path()).unwrap(), prefix);
        for sessions in 1..=2 {
            let result = run(&args(input.path()));
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let saved = fs::read_to_string(output.path()).unwrap();
            assert!(saved.starts_with("{ \"existing\": true }\n"));
            let values: Vec<Value> = saved
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            assert_eq!(values[0], json!({"existing":true}));
            assert_eq!(values.len(), 1 + sessions * 3);
            for batch in values[1..].chunks(3) {
                assert_eq!(batch[0], event(100., "11", "ffffff"));
                assert_eq!(batch[1], event(100.005, "4", "aaaaaa"));
                assert_eq!(batch[2], event(100.02, "5", "bbbbbb"));
            }
        }
    }
}
#[test]
fn malformed_records_and_clock_discontinuities_report_line_numbers() {
    for tail in ["not json".into(), event(98., "4", "aaaaaa").to_string()] {
        let f = Fixture::new(&format!("{}\n{tail}\n", event(100., "4", "aaaaaa")));
        let out = run(&["--replay", f.path(), "--speed", "0", "--stats"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("line 2"));
    }
    let f = Fixture::new(&format!(
        "{}\n{}\n",
        event(100., "4", "aaaaaa"),
        event(99.9, "4", "aaaaaa")
    ));
    assert!(run(&["--replay", f.path(), "--speed", "0", "--stats"])
        .status
        .success());
}
#[test]
fn replay_rejects_iq_options_bad_windows_and_nonterminal_tui_without_truncating() {
    let f = recording();
    let output = Fixture::new("keep");
    for extra in [
        vec!["--sample-rate", "8M"],
        vec!["--speed", "-1"],
        vec!["--activity-window", "0"],
        vec!["-i", "-o", output.path()],
    ] {
        let mut args = vec!["--replay", f.path()];
        args.extend(extra);
        assert!(!run(&args).status.success());
    }
    assert_eq!(fs::read_to_string(output.path()).unwrap(), "keep");
}
#[test]
fn catalogue_is_optional_validated_and_protected() {
    let f = recording();
    let catalogue =
        Fixture::new(r#"{"sites":[],"location_evidence":[],"code_assignment_evidence":[]}"#);
    let before = fs::read_to_string(catalogue.path()).unwrap();
    let out = run(&[
        "--replay",
        f.path(),
        "--speed",
        "0",
        "--stats",
        "--catalogue",
        catalogue.path(),
        "-o",
        catalogue.path(),
    ]);
    assert!(!out.status.success());
    assert_eq!(fs::read_to_string(catalogue.path()).unwrap(), before);
    let invalid = Fixture::new("{}");
    assert!(!run(&[
        "--replay",
        f.path(),
        "--speed",
        "0",
        "--stats",
        "--catalogue",
        invalid.path()
    ])
    .status
    .success());
}
