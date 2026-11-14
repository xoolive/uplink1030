//! Streaming JSONL replay with an analysis clock independent of playback speed.
use crate::{activity::Message, output::Output, source::SourceError};
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub struct Reader {
    reader: BufReader<File>,
    line: usize,
    high_water: Option<f64>,
}
impl Reader {
    pub fn open(path: &str) -> Result<Self, SourceError> {
        Ok(Self {
            reader: BufReader::new(File::open(path)?),
            line: 0,
            high_water: None,
        })
    }
    pub fn next(&mut self) -> Result<Option<(f64, Value)>, SourceError> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        self.line += 1;
        let parse = || -> Result<(f64, Value), String> {
            let value: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            let m = Message::parse(value)?;
            if self.high_water.is_some_and(|t| m.time < t - 1.) {
                return Err("timestamp moved backward by more than 1 second; split discontinuous sessions before replay".into());
            }
            Ok((self.high_water.map_or(m.time, |t| t.max(m.time)), m.value))
        };
        let (t, value) = parse().map_err(|e| format!("JSONL line {}: {e}", self.line))?;
        self.high_water = Some(t);
        Ok(Some((t, value)))
    }
}

#[derive(Debug)]
pub struct Clock {
    pub time: f64,
}
impl Clock {
    pub fn advance(&mut self, wall_seconds: f64, speed: f64, paused: bool) {
        if !paused {
            self.time += wall_seconds * speed;
        }
    }
}

pub fn run(path: &str, out: &mut Output, stopped: &AtomicBool) -> Result<(), SourceError> {
    'restart: loop {
        let mut reader = Reader::open(path)?;
        let mut next = reader.next()?;
        let start = next.as_ref().map_or(0., |e| e.0);
        let mut clock = Clock { time: start };
        out.advance(start);
        let mut wall = Instant::now();
        let mut processed = 0;
        loop {
            if stopped.load(Ordering::Relaxed) {
                break 'restart;
            }
            let ended = next.is_none();
            if !out.poll(path, clock.time - start, ended)? {
                break 'restart;
            }
            if out.restart {
                out.reset_activity();
                continue 'restart;
            }
            if ended {
                if !out.is_interactive() {
                    break 'restart;
                }
                std::thread::sleep(Duration::from_millis(50));
                wall = Instant::now();
                continue;
            }
            let elapsed = wall.elapsed().as_secs_f64();
            wall = Instant::now();
            let target = next.as_ref().unwrap().0;
            if out.speed == 0. && !out.paused {
                clock.time = target;
            } else {
                clock.advance(elapsed, out.speed, out.paused);
            }
            if !out.paused && clock.time >= target {
                let (_, value) = next.take().unwrap();
                out.message(value)?;
                next = reader.next()?;
                processed += 1;
                // Poll controls even in unpaced mode, but avoid an OS event call per message.
                while !out.paused
                    && !stopped.load(Ordering::Relaxed)
                    && next.as_ref().is_some_and(|e| e.0 <= clock.time)
                    && processed % 256 != 0
                {
                    let (_, value) = next.take().unwrap();
                    out.message(value)?;
                    next = reader.next()?;
                    processed += 1;
                }
                if next.is_none() {
                    clock.time = out.model.now;
                }
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
            if next.as_ref().is_some_and(|e| e.0 > clock.time) {
                out.advance(clock.time);
            }
        }
    }
    out.model.expire();
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playback_speed_and_pause_do_not_change_capture_time() {
        let mut a = Clock { time: 100. };
        let mut b = Clock { time: 100. };
        a.advance(10., 1., false);
        b.advance(1., 10., false);
        assert_eq!(a.time, b.time);
        a.advance(100., 10., true);
        assert_eq!(a.time, 110.);
        a.advance(1., 10., false);
        assert_eq!(a.time, 120.);
    }
}
