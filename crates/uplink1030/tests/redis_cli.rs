//! Real executable/RESP integration: immediate publication, file visibility,
//! acknowledgements, reconnects and bounded SIGINT draining without an external Redis.
#[path = "support/redis.rs"]
mod support;
use serde_json::{json, Value};
use std::{
    fs,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};
use support::{Mode, Server};
use tokio::time::{sleep, timeout, Duration};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "uplink-redis-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, contents).unwrap();
        Self(path)
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
struct Running(Option<Child>);
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Running {
    async fn finish(mut self) -> std::process::Output {
        timeout(Duration::from_secs(8), async {
            while self.0.as_mut().unwrap().try_wait().unwrap().is_none() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("CLI did not exit");
        self.0.take().unwrap().wait_with_output().unwrap()
    }
    #[cfg(unix)]
    fn interrupt(&self) {
        assert!(Command::new("kill")
            .args(["-INT", &self.0.as_ref().unwrap().id().to_string()])
            .status()
            .unwrap()
            .success());
    }
}
fn event(time: f64, index: usize) -> Value {
    json!({"timestamp":time,"uf":"4","icao24":format!("{index:06x}"),"frame":"20970000b1c8a9","interrogator":"II3","bds":"50"})
}
fn recording(events: &[Value]) -> Fixture {
    Fixture::new(
        &events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}
fn spawn(
    input: &Fixture,
    url: &str,
    speed: &str,
    retry: &str,
    output: Option<&Fixture>,
) -> Running {
    let mut command = Command::new(env!("CARGO_BIN_EXE_uplink1030"));
    command.args([
        "--replay",
        input.path(),
        "--speed",
        speed,
        "--redis-url",
        url,
        "--redis-topic",
        "test-only",
        "--redis-retry-interval",
        retry,
    ]);
    if let Some(output) = output {
        command.args(["-o", output.path()]);
    }
    Running(Some(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ))
}
fn values(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publishes_exact_file_stdout_events_in_order_including_final_event() {
    let server = Server::start(Mode::Healthy).await;
    let expected: Vec<_> = (0..35).map(|i| event(100. + i as f64 * 0.01, i)).collect();
    let input = recording(&expected);
    let output = Fixture::new("");
    let result = spawn(&input, &server.url, "0", "0", Some(&output))
        .finish()
        .await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(values(&result.stdout), expected);
    assert_eq!(values(&fs::read(output.path()).unwrap()), expected);
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.len(), 35);
    for (record, expected) in seen.iter().zip(expected) {
        assert_eq!(record.topic, "test-only");
        assert_eq!(
            serde_json::from_str::<Value>(&record.payload).unwrap(),
            expected
        );
    }
}
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quiet_input_publishes_and_flushes_file_without_waiting_for_more_events_or_eof() {
    let server = Server::start(Mode::Healthy).await;
    let input = recording(&[event(100., 1), event(160., 2)]);
    let output = Fixture::new("");
    let mut child = spawn(&input, &server.url, "1", "0", Some(&output));
    server.wait_count(1).await;
    timeout(Duration::from_secs(2), async {
        while fs::read(output.path()).unwrap().is_empty() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(child.0.as_mut().unwrap().try_wait().unwrap().is_none());
    assert_eq!(
        values(&fs::read(output.path()).unwrap()),
        vec![event(100., 1)]
    );
    child.interrupt();
    let result = child.finish().await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(values(&result.stdout), vec![event(100., 1)]);
    assert_eq!(server.seen.lock().unwrap().len(), 1);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uncertain_failure_reconnects_before_retrying_the_same_event() {
    let server = Server::start(Mode::DisconnectFirst).await;
    let input = recording(&[event(100., 1)]);
    let result = spawn(&input, &server.url, "0", "1", None).finish().await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].payload, seen[1].payload);
    assert_ne!(seen[0].connection, seen[1].connection);
    assert_eq!(values(&result.stdout), vec![event(100., 1)]);
    assert!(String::from_utf8_lossy(&result.stderr).contains("retry may duplicate"));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_disabled_warn_skip_and_do_not_reuse_failed_connection() {
    let server = Server::start(Mode::RejectFirst).await;
    let input = recording(&[event(100., 1), event(100.1, 2)]);
    let result = spawn(&input, &server.url, "0", "0", None).finish().await;
    assert!(result.status.success());
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_ne!(seen[0].connection, seen[1].connection);
    assert_eq!(
        values(&result.stdout),
        vec![event(100., 1), event(100.1, 2)]
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("retries disabled"));
}
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledgement_precedes_local_output_and_sigint_drains_on_a_fresh_connection() {
    let server = Server::start(Mode::HoldFirst).await;
    let input = recording(&[event(100., 1)]);
    let output = Fixture::new("");
    let child = spawn(&input, &server.url, "0", "1", Some(&output));
    server.wait_count(1).await;
    sleep(Duration::from_millis(50)).await;
    assert!(
        fs::read(output.path()).unwrap().is_empty(),
        "file written before Redis acceptance"
    );
    child.interrupt();
    let result = child.finish().await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_ne!(seen[0].connection, seen[1].connection);
    assert_eq!(seen[0].payload, seen[1].payload);
    assert_eq!(
        values(&fs::read(output.path()).unwrap()),
        vec![event(100., 1)]
    );
    assert_eq!(values(&result.stdout), vec![event(100., 1)]);
}
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_is_bounded_and_reports_unacknowledged_delivery() {
    let server = Server::start(Mode::HoldAll).await;
    let input = recording(&[event(100., 1)]);
    let child = spawn(&input, &server.url, "0", "1", None);
    server.wait_count(1).await;
    let start = Instant::now();
    child.interrupt();
    let result = child.finish().await;
    assert!(!result.status.success());
    assert!(start.elapsed() < Duration::from_secs(4));
    let errors = String::from_utf8_lossy(&result.stderr);
    assert!(
        errors.contains("Timed out draining Redis after 2s"),
        "{errors}"
    );
    assert!(errors.contains("one event remains unacknowledged"));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_error_still_drains_output_and_empty_input_does_not_publish() {
    let server = Server::start(Mode::Healthy).await;
    let input = Fixture::new(&format!("{}\nnot json\n", event(100., 1)));
    let result = spawn(&input, &server.url, "0", "0", None).finish().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("line 2"));
    assert_eq!(server.seen.lock().unwrap().len(), 1);
    let empty = Fixture::new("");
    let result = spawn(&empty, &server.url, "0", "0", None).finish().await;
    assert!(result.status.success());
    assert_eq!(server.seen.lock().unwrap().len(), 1);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_topic_and_option_requirements_match_other_receivers() {
    let server = Server::start(Mode::Healthy).await;
    let input = recording(&[event(100., 1)]);
    let mut command = Command::new(env!("CARGO_BIN_EXE_uplink1030"));
    command.args([
        "--replay",
        input.path(),
        "--speed",
        "0",
        "--redis-url",
        &server.url,
        "--stats",
    ]);
    let result = Running(Some(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ))
    .finish()
    .await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(server.seen.lock().unwrap()[0].topic, "uplink1030");
    let result = Command::new(env!("CARGO_BIN_EXE_uplink1030"))
        .args(["--replay", input.path(), "--redis-topic", "unconfigured"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--redis-url"));
}
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interruption_during_retry_backoff_drains_without_waiting_for_retry_interval() {
    let server = Server::start(Mode::DisconnectFirst).await;
    let input = recording(&[event(100., 1)]);
    let child = spawn(&input, &server.url, "0", "3600", None);
    server.wait_count(1).await;
    sleep(Duration::from_millis(50)).await;
    child.interrupt();
    let result = child.finish().await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(server.seen.lock().unwrap().len(), 2);
}
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn error_unwinding_still_drains_the_interrupted_event() {
    let server = Server::start(Mode::HoldFirst).await;
    let input = Fixture::new(&format!("{}\ninvalid tail\n", event(100., 1)));
    let child = spawn(&input, &server.url, "0", "1", None);
    server.wait_count(1).await;
    child.interrupt();
    let result = child.finish().await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("line 2"));
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_ne!(seen[0].connection, seen[1].connection);
    assert_eq!(seen[0].payload, seen[1].payload);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_or_unavailable_redis_does_not_change_existing_output() {
    let input = recording(&[event(100., 1)]);
    let output = Fixture::new("keep");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("redis://{}/", listener.local_addr().unwrap());
    drop(listener);
    for url in [&url, "invalid://not-redis"] {
        let result = spawn(&input, url, "0", "0", Some(&output)).finish().await;
        assert!(!result.status.success());
        assert_eq!(fs::read_to_string(output.path()).unwrap(), "keep");
    }
}
