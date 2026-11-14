//! Capture-time activity model shared by live reception and JSONL replay.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const MAX_EVENTS: usize = 20_000;
const HISTORY: f64 = 300.;
type BurstKey = (Option<String>, Option<String>, String);

#[derive(Clone, Debug)]
pub struct Settings {
    pub window: f64,
    pub aircraft_ttl: f64,
    pub radar_ttl: f64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            window: 60.,
            aircraft_ttl: 60.,
            radar_ttl: 120.,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if [self.window, self.aircraft_ttl, self.radar_ttl]
            .iter()
            .any(|x| !x.is_finite() || *x <= 0. || *x > 3600.)
        {
            return Err(
                "activity window and expiry must be finite, positive and at most 3600 seconds"
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Message {
    pub time: f64,
    pub uf: u8,
    pub address: String,
    pub code: Option<String>,
    pub bds: Option<String>,
    pub known: Option<bool>,
    pub value: Value,
}
impl Message {
    pub fn parse(value: Value) -> Result<Self, String> {
        let time = value["timestamp"]
            .as_f64()
            .filter(|t| t.is_finite())
            .ok_or("missing finite timestamp")?;
        let uf = value["uf"]
            .as_str()
            .and_then(|s| s.parse::<u8>().ok())
            .filter(|u| *u <= 31)
            .ok_or("expected flat string uf in 0..31")?;
        let address = crate::schema::activity_address(&value)
            .filter(|s| s.len() == 6 && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or("expected six-digit icao24")?
            .to_ascii_lowercase();
        let frame = value["frame"].as_str().ok_or("missing flat frame")?;
        if ![14, 28].contains(&frame.len()) || !frame.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("frame must contain 14 or 28 hexadecimal digits".into());
        }
        let code = value["interrogator"]
            .as_str()
            .filter(|s| valid_code(s))
            .map(str::to_owned);
        let bds = value["bds"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if !value["known"].is_null() && !value["known"].is_boolean() {
            return Err("known must be boolean when present".into());
        }
        // Airborne ACAS has no ground interrogator identity, even if an input is enriched incorrectly.
        Ok(Self {
            time,
            uf,
            address,
            code: if uf == 0 { None } else { code },
            bds,
            known: value["known"].as_bool(),
            value,
        })
    }
    pub fn addressed(&self) -> bool {
        self.uf != 11 && self.address != "ffffff"
    }
}
pub fn valid_code(s: &str) -> bool {
    let (prefix, max) = if s.starts_with("II") {
        ("II", 15)
    } else if s.starts_with("SI") {
        ("SI", 63)
    } else {
        return false;
    };
    s.strip_prefix(prefix)
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| n <= max && s == format!("{prefix}{n}"))
}

#[derive(Clone)]
pub struct Observation {
    pub time: f64,
    pub uf: u8,
    pub address: Option<String>,
    pub code: Option<String>,
    pub bds: Option<String>,
    pub known: bool,
}
#[derive(Default)]
pub struct Activity {
    pub last: f64,
    pub total: usize,
    pub latest: Option<Message>,
    pub events: VecDeque<Observation>,
    pub truncated: bool,
    bursts: BTreeMap<String, Burst>,
    gaps: VecDeque<Gap>,
}
struct Burst {
    last: f64,
    onset: f64,
    known: bool,
}
struct Gap {
    time: f64,
    interval: f64,
    target: String,
    known: bool,
}
#[derive(Debug)]
pub struct Scan {
    pub period: f64,
    pub iqr: f64,
    pub support: usize,
    pub targets: usize,
    pub known: bool,
    pub ambiguous: bool,
}
impl Activity {
    fn observe(&mut self, m: &Message, now: f64, radar: bool) {
        self.last = now;
        self.total += 1;
        self.events.push_back(Observation {
            time: now,
            uf: m.uf,
            address: m.addressed().then(|| m.address.clone()),
            code: m.code.clone(),
            bds: m.bds.clone(),
            known: m.known == Some(true),
        });
        if self.events.len() > MAX_EVENTS {
            self.events.pop_front();
            self.truncated = true;
        }
        if radar && m.addressed() && [4, 5].contains(&m.uf) {
            if let Some(b) = self.bursts.get_mut(&m.address) {
                if now - b.last > 0.5 {
                    let interval = now - b.onset;
                    if interval > 2. && interval < 20. {
                        self.gaps.push_back(Gap {
                            time: now,
                            interval,
                            target: m.address.clone(),
                            known: b.known && m.known == Some(true),
                        });
                    }
                    b.onset = now;
                    b.known = m.known == Some(true);
                }
                b.last = now;
            } else if self.bursts.len() < MAX_EVENTS {
                self.bursts.insert(
                    m.address.clone(),
                    Burst {
                        last: now,
                        onset: now,
                        known: m.known == Some(true),
                    },
                );
            } else {
                self.truncated = true;
            }
            if self.gaps.len() > MAX_EVENTS {
                self.gaps.pop_front();
                self.truncated = true;
            }
        }
        self.latest = Some(m.clone());
    }
    fn prune(&mut self, now: f64, window: f64) {
        while self.events.front().is_some_and(|e| e.time <= now - window) {
            self.events.pop_front();
        }
        while self.gaps.front().is_some_and(|e| e.time <= now - HISTORY) {
            self.gaps.pop_front();
        }
        self.bursts.retain(|_, b| now - b.last < HISTORY);
    }
    pub fn scan(&self, now: f64) -> Option<Scan> {
        let all: Vec<_> = self
            .gaps
            .iter()
            .filter(|g| now - g.time < HISTORY)
            .collect();
        let supported: Vec<_> = all.iter().copied().filter(|g| g.known).collect();
        let known = supported.len() >= 20;
        let gaps = if known { supported } else { all };
        if gaps.len() < 20 {
            return None;
        }
        let mut bins = [0usize; 720];
        for g in &gaps {
            bins[((g.interval - 2.) / 0.025).floor() as usize] += 1;
        }
        // Integrated ±0.15 s support avoids bin-boundary instability.
        let score = |i: usize| {
            bins[i.saturating_sub(6)..=(i + 6).min(719)]
                .iter()
                .sum::<usize>()
        };
        let peak = (0..720).max_by_key(|i| (score(*i), std::cmp::Reverse(*i)))?;
        let mode = 2. + (peak as f64 + 0.5) * 0.025;
        let near: Vec<_> = gaps
            .iter()
            .filter(|g| (g.interval - mode).abs() <= 0.15)
            .collect();
        if near.len() < 10 {
            return None;
        }
        let mut values: Vec<_> = near.iter().map(|g| g.interval).collect();
        values.sort_by(f64::total_cmp);
        let targets = near
            .iter()
            .map(|g| &g.target)
            .collect::<BTreeSet<_>>()
            .len();
        let second = (0usize..720)
            .filter(|i| i.abs_diff(peak) > 12)
            .map(score)
            .max()
            .unwrap_or(0);
        Some(Scan {
            period: values[values.len() / 2],
            iqr: values[3 * values.len() / 4] - values[values.len() / 4],
            support: values.len(),
            targets,
            known,
            ambiguous: second * 2 >= near.len() || targets < 2,
        })
    }
}

pub struct Model {
    pub settings: Settings,
    pub now: f64,
    pub start: Option<f64>,
    pub radars: BTreeMap<String, Activity>,
    pub aircraft: BTreeMap<String, Activity>,
    pub total: usize,
    pub unattributed: usize,
    last_prune: f64,
}
impl Model {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            now: 0.,
            start: None,
            radars: BTreeMap::new(),
            aircraft: BTreeMap::new(),
            total: 0,
            unattributed: 0,
            last_prune: f64::NEG_INFINITY,
        }
    }
    pub fn advance(&mut self, now: f64) {
        if self.start.is_none() {
            self.start = Some(now);
            self.now = now;
        }
        self.now = self.now.max(now);
        if self.now - self.last_prune < 0.25 {
            return;
        }
        self.expire();
    }
    /// Force exact expiry at a presentation/EOF boundary, independently of sweep cadence.
    pub fn expire(&mut self) {
        self.last_prune = self.now;
        self.radars
            .retain(|_, a| self.now - a.last < self.settings.radar_ttl);
        self.aircraft
            .retain(|_, a| self.now - a.last < self.settings.aircraft_ttl);
        for a in self.radars.values_mut().chain(self.aircraft.values_mut()) {
            a.prune(self.now, self.settings.window);
        }
    }
    pub fn ingest(&mut self, m: Message) {
        self.advance(m.time);
        self.total += 1;
        if let Some(code) = &m.code {
            let row = self.radars.entry(code.clone()).or_default();
            if self.now - row.last >= self.settings.radar_ttl {
                *row = Activity::default();
            }
            row.observe(&m, self.now, true);
        } else {
            self.unattributed += 1;
        }
        if m.addressed() {
            let row = self.aircraft.entry(m.address.clone()).or_default();
            if self.now - row.last >= self.settings.aircraft_ttl {
                *row = Activity::default();
            }
            row.observe(&m, self.now, false);
        }
    }
    pub fn denominator(&self) -> f64 {
        (self.now - self.start.unwrap_or(self.now))
            .max(1.)
            .min(self.settings.window)
    }
    pub fn recent<'a>(&'a self, a: &'a Activity) -> impl Iterator<Item = &'a Observation> {
        a.events
            .iter()
            .filter(|e| self.now - e.time < self.settings.window)
    }
    pub fn describe(&self, a: &Activity) -> Vec<String> {
        let mut ufs = BTreeMap::<u8, usize>::new();
        let mut bds = BTreeMap::<String, usize>::new();
        let mut codes = BTreeSet::new();
        let mut targets = BTreeSet::new();
        let mut known = BTreeSet::new();
        let mut onsets: BTreeMap<BurstKey, (f64, f64)> = BTreeMap::new();
        let mut intervals: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut per_code: BTreeMap<String, (usize, BTreeMap<String, usize>)> = BTreeMap::new();
        for e in self.recent(a) {
            *ufs.entry(e.uf).or_default() += 1;
            if let Some(code) = &e.code {
                codes.insert(code);
                let (n, registers) = per_code.entry(code.clone()).or_default();
                *n += 1;
                if let Some(b) = &e.bds {
                    *registers.entry(b.clone()).or_default() += 1;
                }
            }
            if let Some(target) = &e.address {
                targets.insert(target);
                if e.known {
                    known.insert(target);
                }
            }
            if let Some(b) = &e.bds {
                *bds.entry(b.clone()).or_default() += 1;
                let key = (e.code.clone(), e.address.clone(), b.clone());
                if let Some((last, onset)) = onsets.get_mut(&key) {
                    if e.time - *last > 0.5 {
                        intervals
                            .entry(b.clone())
                            .or_default()
                            .push(e.time - *onset);
                        *onset = e.time;
                    }
                    *last = e.time;
                } else {
                    onsets.insert(key, (e.time, e.time));
                }
            }
        }
        let mut lines = vec![
            format!(
                "Window {:.0}s | {:.2} msg/s | {} events since row appeared{}",
                self.settings.window,
                self.recent(a).count() as f64 / self.denominator(),
                a.total,
                if a.truncated {
                    " | capped: rates are lower bounds"
                } else {
                    ""
                }
            ),
            format!(
                "Targets: {} candidate addresses ({} known) | Codes: {}",
                targets.len(),
                known.len(),
                codes.into_iter().cloned().collect::<Vec<_>>().join(" ")
            ),
            format!(
                "UF: {}",
                ufs.iter()
                    .map(|(u, n)| format!("{u}:{n}"))
                    .collect::<Vec<_>>()
                    .join("  ")
            ),
        ];
        for (code, (n, registers)) in per_code {
            lines.push(format!(
                "{code}: {:.2} msg/s | BDS req/s: {}",
                n as f64 / self.denominator(),
                registers
                    .iter()
                    .map(|(b, n)| format!("{b}={:.2}", *n as f64 / self.denominator()))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        for (b, n) in bds {
            let mut gaps = intervals.remove(&b).unwrap_or_default();
            gaps.sort_by(f64::total_cmp);
            let recurrence = if gaps.is_empty() {
                "unknown".into()
            } else {
                format!("{:.2}s (n={})", gaps[gaps.len() / 2], gaps.len())
            };
            let target_count = self
                .recent(a)
                .filter(|e| e.bds.as_ref() == Some(&b))
                .filter_map(|e| e.address.as_ref())
                .collect::<BTreeSet<_>>()
                .len();
            lines.push(format!("BDS {b}: {:.2} req/s | {target_count} targets | per-target burst interval {recurrence}", n as f64 / self.denominator()));
        }
        if let Some(s) = a.scan(self.now) {
            lines.push(format!(
                "Scan {}{:.3}s / {:.1} RPM | n={} targets={} IQR={:.3}s{}",
                if s.known { "" } else { "candidate " },
                s.period,
                60. / s.period,
                s.support,
                s.targets,
                s.iqr,
                if s.ambiguous {
                    " | AMBIGUOUS / possible missed scans"
                } else {
                    ""
                }
            ));
        }
        if let Some(m) = &a.latest {
            lines.push(format!(
                "Latest UF{} | {} | confidence {} | RSSI {} dB",
                m.uf,
                match m.known {
                    _ if m.uf == 11 => "all-call (not an aircraft)",
                    Some(true) => "known address",
                    Some(false) => "unverified address",
                    None => "address support unknown",
                },
                crate::schema::observation(&m.value, "confidence"),
                crate::schema::observation(&m.value, "rssi")
            ));
            lines.push(format!(
                "Frame: {}",
                m.value["frame"].as_str().unwrap_or("")
            ));
            lines.push(format!("Decoded: {}", m.value));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    pub fn msg(t: f64, uf: u8, address: &str, code: &str) -> Message {
        Message::parse(json!({"timestamp":t,"uf":uf.to_string(),"icao24":address,"interrogator":code,"bds":"20","frame":"20970000b1c8a9"})).unwrap()
    }
    #[test]
    fn allcalls_and_acas_are_not_ground_targets() {
        let mut m = Model::new(Settings::default());
        m.ingest(msg(100., 11, "ffffff", "II3"));
        m.ingest(msg(101., 0, "abcdef", "II3"));
        assert_eq!(m.aircraft.len(), 1);
        assert_eq!(m.radars["II3"].events.len(), 1);
        assert!(m.radars["II3"].events[0].address.is_none());
        assert_eq!(m.unattributed, 1);
    }
    #[test]
    fn silence_expires_rows_and_rates_use_capture_time() {
        let mut m = Model::new(Settings::default());
        m.ingest(msg(100., 4, "abcdef", "II3"));
        m.advance(130.);
        assert_eq!(m.denominator(), 30.);
        m.advance(161.);
        assert!(m.aircraft.is_empty());
        assert_eq!(m.recent(&m.radars["II3"]).count(), 0);
        m.advance(221.);
        assert!(m.radars.is_empty());
    }
    #[test]
    fn bursts_not_individual_messages_estimate_rotation() {
        let mut m = Model::new(Settings::default());
        for i in 0..30 {
            for (a, offset) in [("aaaaaa", 0.), ("bbbbbb", 0.3)] {
                for dt in [0., 0.01, 0.02] {
                    m.ingest(msg(100. + i as f64 * 4. + offset + dt, 4, a, "II3"));
                }
            }
        }
        let s = m.radars["II3"].scan(m.now).unwrap();
        assert!((s.period - 4.).abs() < 0.001);
        assert_eq!(s.support, 58);
        assert_eq!(s.targets, 2);
        assert!(!s.known);
        assert!(!s.ambiguous);
    }
    #[test]
    fn mixed_periods_are_ambiguous() {
        let mut m = Model::new(Settings::default());
        let mut events = vec![];
        for (a, period) in [("aaaaaa", 4.), ("bbbbbb", 8.)] {
            for i in 0..30 {
                events.push(msg(100. + i as f64 * period, 4, a, "II3"));
            }
        }
        events.sort_by(|a, b| a.time.total_cmp(&b.time));
        for e in events {
            m.ingest(e);
        }
        assert!(m.radars["II3"].scan(m.now).unwrap().ambiguous);
    }
    #[test]
    fn independently_known_scan_evidence_is_preferred() {
        let mut m = Model::new(Settings::default());
        let mut events = vec![];
        for (a, period, known) in [
            ("aaaaaa", 4., true),
            ("bbbbbb", 4., true),
            ("cccccc", 6., false),
        ] {
            for i in 0..30 {
                let mut e = msg(100. + i as f64 * period, 4, a, "II3");
                e.known = Some(known);
                events.push(e);
            }
        }
        events.sort_by(|a, b| a.time.total_cmp(&b.time));
        for e in events {
            m.ingest(e);
        }
        let s = m.radars["II3"].scan(m.now).unwrap();
        assert!(s.known);
        assert!((s.period - 4.).abs() < 0.001);
        assert_eq!(s.targets, 2);
    }
    #[test]
    fn exact_expiry_does_not_depend_on_background_tick_cadence() {
        let simulate = |step: f64| {
            let mut m = Model::new(Settings::default());
            for time in [100., 100.1, 160.11, 200., 260.2, 280.2] {
                if m.start.is_some() {
                    while m.now + step < time {
                        m.advance(m.now + step);
                    }
                }
                m.ingest(msg(time, 4, "abcdef", "II3"));
            }
            m.expire();
            (
                m.describe(&m.aircraft["abcdef"]),
                m.describe(&m.radars["II3"]),
            )
        };
        assert_eq!(simulate(0.01), simulate(3.));
    }
    #[test]
    fn bounded_event_history_warns_when_rates_are_lower_bounds() {
        let mut m = Model::new(Settings::default());
        for i in 0..MAX_EVENTS + 10 {
            m.ingest(msg(100. + i as f64 * 0.001, 4, "abcdef", "II3"));
        }
        assert_eq!(m.radars["II3"].events.len(), MAX_EVENTS);
        assert!(m.radars["II3"].truncated);
    }
    #[test]
    fn parser_rejects_legacy_and_preserves_unknown_support() {
        assert!(Message::parse(json!({"message":{"raw":"123"}})).is_err());
        assert_eq!(msg(1., 4, "abcdef", "invalid").known, None);
        assert!(msg(1., 4, "abcdef", "invalid").code.is_none());
    }
}
