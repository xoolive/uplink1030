//! What jet1090 (1090 MHz downlink) says about the addresses we hear
//! interrogated, from its Redis feed or from a recorded JSONL file.
//!
//! The state is causal: an interrogation at time t is annotated with what
//! was heard on 1090 before t. Annotation never removes or filters events.
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

/// What we last heard from one aircraft on 1090.
#[derive(Clone, Debug, Default)]
pub struct Heard {
    pub last: f64,
    pub callsign: Option<String>,
    pub altitude: Option<f64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

#[derive(Default)]
pub struct State {
    pub aircraft: HashMap<String, Heard>,
    /// Last DF11 all-call reply naming each interrogator code.
    pub codes: HashMap<String, f64>,
    pub messages: u64,
    /// Capture time of the latest downlink message.
    pub latest: Option<f64>,
}

impl State {
    /// Ingests one jet1090 JSON message.
    pub fn ingest(&mut self, message: &Value) {
        let Some(t) = message["timestamp"].as_f64().filter(|t| t.is_finite()) else {
            return;
        };
        let Some(address) = message["icao24"]
            .as_str()
            .filter(|a| a.len() == 6 && a.bytes().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase)
        else {
            return;
        };
        self.messages += 1;
        self.latest = Some(self.latest.map_or(t, |l| l.max(t)));
        if message["df"].as_str() == Some("11") {
            if let Some(code) = interrogator(&message["interrogator"]) {
                let last = self.codes.entry(code).or_insert(t);
                *last = last.max(t);
            }
        }
        let heard = self.aircraft.entry(address).or_default();
        heard.last = heard.last.max(t);
        let callsign = message["callsign"]
            .as_str()
            .or_else(|| message["bds20"]["callsign"].as_str())
            .map(str::trim)
            .filter(|c| !c.is_empty() && !c.contains('#'));
        if let Some(callsign) = callsign {
            heard.callsign = Some(callsign.to_owned());
        }
        if let Some(altitude) = message["altitude"].as_f64() {
            heard.altitude = Some(altitude);
        }
        if let (Some(lat), Some(lon)) =
            (message["latitude"].as_f64(), message["longitude"].as_f64())
        {
            heard.latitude = Some(lat);
            heard.longitude = Some(lon);
        }
    }

    /// Forgets aircraft and codes not heard for `ttl` seconds before `now`.
    pub fn prune(&mut self, now: f64, ttl: f64) {
        self.aircraft.retain(|_, h| now - h.last <= ttl);
        self.codes.retain(|_, t| now - *t <= ttl);
    }

    /// Adds what 1090 says to one output event (time `t`), without removing
    /// anything: `seen_1090`, `seen_1090_age` and `callsign` for addressed
    /// interrogations, `interrogator_seen_1090` for all-calls.
    pub fn annotate(&self, event: &mut Map<String, Value>, ttl: f64) {
        let Some(t) = event.get("timestamp").and_then(Value::as_f64) else {
            return;
        };
        let uf = event.get("uf").and_then(Value::as_str).unwrap_or("");
        if uf == "11" {
            if let Some(code) = event.get("interrogator").and_then(Value::as_str) {
                let seen = self.codes.get(code).is_some_and(|last| t - last <= ttl);
                event.insert("interrogator_seen_1090".into(), seen.into());
            }
            return;
        }
        let Some(address) = event
            .get("icao24")
            .and_then(Value::as_str)
            .filter(|a| !a.eq_ignore_ascii_case("ffffff"))
            .map(str::to_ascii_lowercase)
        else {
            return;
        };
        // Causal: heard before the interrogation, within the time to live;
        // one second of tolerance absorbs the offset between the host clock
        // and jet1090's (e.g. GNSS time from a Beast source).
        let heard = self
            .aircraft
            .get(&address)
            .filter(|h| h.last <= t + 1. && t - h.last <= ttl);
        event.insert("seen_1090".into(), heard.is_some().into());
        if let Some(h) = heard {
            let age = ((t - h.last).max(0.) * 10.).round() / 10.;
            event.insert("seen_1090_age".into(), age.into());
            if let Some(callsign) = &h.callsign {
                event.insert("callsign".into(), callsign.clone().into());
            }
        }
    }

    /// One line for the TUI header.
    pub fn summary(&self, now: f64, ttl: f64) -> String {
        match self.latest {
            None => "1090 waiting".into(),
            Some(latest) if now - latest > 30. => format!("1090 stale {:.0}s", now - latest),
            Some(_) => {
                let live = self
                    .aircraft
                    .values()
                    .filter(|h| now - h.last <= ttl)
                    .count();
                format!("1090 {live} aircraft")
            }
        }
    }
}

/// II/SI codes, written `"SI12"` (current jet1090) or `{"type": "SI", "value": 12}`
/// (October 2026 recordings); `SI0` and `invalid` are not allocated codes.
pub fn interrogator(value: &Value) -> Option<String> {
    let code = match value {
        Value::String(s) => s.clone(),
        Value::Object(o) => format!("{}{}", o.get("type")?.as_str()?, o.get("value")?.as_u64()?),
        _ => return None,
    };
    crate::activity::valid_code(&code)
        .then_some(code)
        .filter(|c| c != "SI0")
}

/// A recorded jet1090 JSONL (optionally .zst) file, read in step with the
/// uplink capture clock.
pub struct FileFeed {
    lines: Box<dyn BufRead + Send>,
    pending: Option<Value>,
    ended: bool,
}
impl FileFeed {
    pub fn open(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let lines: Box<dyn BufRead + Send> = if path.ends_with(".zst") {
            Box::new(BufReader::new(
                zstd::stream::read::Decoder::new(file).map_err(|e| format!("{path}: {e}"))?,
            ))
        } else {
            Box::new(BufReader::new(file))
        };
        Ok(Self {
            lines,
            pending: None,
            ended: false,
        })
    }
    /// Ingests every message up to time `t`.
    pub fn advance(&mut self, t: f64, state: &mut State) {
        loop {
            if let Some(value) = &self.pending {
                if value["timestamp"].as_f64().is_some_and(|m| m > t) {
                    return;
                }
                state.ingest(value);
                self.pending = None;
            }
            if self.ended {
                return;
            }
            let mut line = String::new();
            match self.lines.read_line(&mut line) {
                Ok(0) | Err(_) => self.ended = true,
                Ok(_) => self.pending = serde_json::from_str(&line).ok(),
            }
        }
    }
}

/// Where downlink comes from, and the shared state it feeds.
pub struct Downlink {
    pub state: Arc<Mutex<State>>,
    file: Option<FileFeed>,
    pub ttl: f64,
    last_prune: f64,
}
impl Downlink {
    pub fn from_file(path: &str, ttl: f64) -> Result<Self, String> {
        Ok(Self {
            state: Arc::default(),
            file: Some(FileFeed::open(path)?),
            ttl,
            last_prune: f64::NEG_INFINITY,
        })
    }

    /// Subscribes to jet1090 on Redis (the --redis-url server) in the
    /// background, reconnecting after failures. Decoding never waits on it.
    pub fn from_redis(
        url: &str,
        topic: String,
        ttl: f64,
        stopped: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let client = ::redis::Client::open(url).map_err(|e| format!("Invalid Redis URL: {e}"))?;
        let state: Arc<Mutex<State>> = Arc::default();
        let shared = state.clone();
        tokio::spawn(async move {
            use futures::StreamExt;
            while !stopped.load(Ordering::Relaxed) {
                let connection =
                    tokio::time::timeout(crate::redis::CONNECT_TIMEOUT, client.get_async_pubsub())
                        .await;
                if let Ok(Ok(mut pubsub)) = connection {
                    if pubsub.subscribe(&topic).await.is_ok() {
                        let mut messages = pubsub.on_message();
                        while let Some(message) = messages.next().await {
                            if stopped.load(Ordering::Relaxed) {
                                return;
                            }
                            let Ok(payload) = message.get_payload::<String>() else {
                                continue;
                            };
                            if let Ok(value) = serde_json::from_str::<Value>(&payload) {
                                shared.lock().unwrap().ingest(&value);
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
        Ok(Self {
            state,
            file: None,
            ttl,
            last_prune: f64::NEG_INFINITY,
        })
    }

    /// Annotates one output event, after reading the recorded feed up to it.
    pub fn annotate(&mut self, event: &mut Map<String, Value>) {
        let Some(t) = event.get("timestamp").and_then(Value::as_f64) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if let Some(file) = &mut self.file {
            file.advance(t, &mut state);
        }
        if t - self.last_prune > 10. {
            state.prune(t, self.ttl);
            self.last_prune = t;
        }
        state.annotate(event, self.ttl);
    }

    pub fn summary(&self, now: f64) -> String {
        self.state.lock().unwrap().summary(now, self.ttl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(t: f64, uf: &str, address: &str) -> Map<String, Value> {
        json!({"timestamp": t, "uf": uf, "icao24": address, "frame": "20970000b1c8a9"})
            .as_object()
            .unwrap()
            .clone()
    }

    #[test]
    fn annotation_is_causal_and_expires() {
        let mut state = State::default();
        state.ingest(
            &json!({"timestamp": 100., "df": "17", "icao24": "39D30E", "callsign": "TVF55VU "}),
        );
        // A second of tolerance absorbs the offset between the two clocks.
        let mut before = event(98., "4", "39d30e");
        state.annotate(&mut before, 120.);
        assert_eq!(before["seen_1090"], false);
        let mut after = event(130., "4", "39d30e");
        state.annotate(&mut after, 120.);
        assert_eq!(after["seen_1090"], true);
        assert_eq!(after["seen_1090_age"], 30.);
        assert_eq!(after["callsign"], "TVF55VU");
        let mut late = event(300., "4", "39d30e");
        state.annotate(&mut late, 120.);
        assert_eq!(late["seen_1090"], false);
        assert!(late.get("callsign").is_none());
    }

    #[test]
    fn all_calls_check_the_interrogator_not_the_broadcast_address() {
        let mut state = State::default();
        state.ingest(&json!({"timestamp": 10., "df": "11", "icao24": "39d30e", "interrogator": {"type": "SI", "value": 26}}));
        let mut call = event(12., "11", "ffffff");
        call.insert("interrogator".into(), "SI26".into());
        state.annotate(&mut call, 120.);
        assert_eq!(call["interrogator_seen_1090"], true);
        assert!(call.get("seen_1090").is_none());
    }

    #[test]
    fn interrogator_forms() {
        assert_eq!(interrogator(&json!("II3")).as_deref(), Some("II3"));
        assert_eq!(
            interrogator(&json!({"type": "SI", "value": 12})).as_deref(),
            Some("SI12")
        );
        assert_eq!(interrogator(&json!("SI0")), None);
        assert_eq!(interrogator(&json!("invalid")), None);
    }
}
