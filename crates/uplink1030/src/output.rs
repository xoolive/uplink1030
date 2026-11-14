//! JSONL recording is independent of TUI filters and replay presentation.
use crate::{
    activity::{Message, Model, Settings},
    redis::Publisher,
    source::SourceError,
    tui::{Action, Ui},
};
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Read, Seek, SeekFrom, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

type Screen = Terminal<CrosstermBackend<io::Stdout>>;

/// Preserve earlier sessions, including a final JSON record without a newline.
struct AppendWriter {
    inner: io::BufWriter<std::fs::File>,
    needs_separator: bool,
}
impl AppendWriter {
    fn open(path: &str) -> io::Result<Self> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        let mut last = *b"\n";
        if file.metadata()?.len() > 0 {
            file.seek(SeekFrom::End(-1))?;
            file.read_exact(&mut last)?;
        }
        Ok(Self {
            inner: io::BufWriter::new(file),
            needs_separator: last[0] != b'\n',
        })
    }
}
impl Write for AppendWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if !bytes.is_empty() && self.needs_separator {
            self.inner.write_all(b"\n")?;
            self.needs_separator = false;
        }
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn candidate_names(candidates: &[String]) -> String {
    let mut names: Vec<_> = candidates
        .iter()
        .map(|candidate| format!("{}?", candidate.split(" | ").next().unwrap_or("unknown")))
        .collect();
    names.sort();
    if names.is_empty() {
        "unknown".into()
    } else {
        names.join("\n")
    }
}
pub struct Output {
    writer: Option<Box<dyn Write>>,
    redis: Option<Publisher>,
    redis_waiting: bool,
    stopped: Arc<AtomicBool>,
    stdout_json: bool,
    pretty: bool,
    screen: Option<Screen>,
    last_draw: Instant,
    pub model: Model,
    ui: Ui,
    track_activity: bool,
    sites: BTreeMap<String, Vec<String>>,
    pub replay: bool,
    pub paused: bool,
    pub speed: f64,
    pub restart: bool,
    pub counts: BTreeMap<u8, usize>,
    pub decoded: usize,
}
impl Output {
    pub fn new(
        interactive: bool,
        path: Option<&str>,
        pretty: bool,
        stats: bool,
    ) -> Result<Self, SourceError> {
        if interactive && (!io::stdout().is_terminal() || !io::stdin().is_terminal()) {
            return Err(
                "interactive output requires a terminal; use JSON output when piping".into(),
            );
        }
        let writer = path
            .map(|p| AppendWriter::open(p).map(|writer| Box::new(writer) as Box<dyn Write>))
            .transpose()?;
        let mut result = Self {
            writer,
            redis: None,
            redis_waiting: false,
            stopped: Arc::new(AtomicBool::new(false)),
            stdout_json: !interactive && !stats,
            pretty,
            screen: None,
            last_draw: Instant::now() - Duration::from_secs(1),
            model: Model::new(Settings::default()),
            ui: Ui::default(),
            track_activity: interactive,
            sites: BTreeMap::new(),
            replay: false,
            paused: false,
            speed: 1.,
            restart: false,
            counts: BTreeMap::new(),
            decoded: 0,
        };
        if interactive {
            terminal::enable_raw_mode()?;
            if let Err(e) = execute!(io::stdout(), EnterAlternateScreen) {
                let _ = terminal::disable_raw_mode();
                return Err(e.into());
            }
            match Terminal::new(CrosstermBackend::new(io::stdout())) {
                Ok(screen) => result.screen = Some(screen),
                Err(e) => {
                    let _ = execute!(io::stdout(), LeaveAlternateScreen);
                    let _ = terminal::disable_raw_mode();
                    return Err(e.into());
                }
            }
        }
        Ok(result)
    }
    pub fn attach_redis(&mut self, publisher: Option<Publisher>, stopped: Arc<AtomicBool>) {
        self.redis = publisher;
        self.stopped = stopped;
    }
    /// Bridge the synchronous DSP/file callbacks to acknowledged asynchronous I/O.
    /// The CLI uses Tokio's multithread runtime; block_in_place keeps its signal
    /// and networking drivers alive. Keyboard polling remains responsive on stalls.
    fn publish(&mut self, line: &str) -> Result<(), SourceError> {
        let Some(mut publisher) = self.redis.take() else {
            return Ok(());
        };
        let stopped = self.stopped.clone();
        self.redis_waiting = true;
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let publication = publisher.publish(line, &stopped);
                tokio::pin!(publication);
                loop {
                    tokio::select! {
                        biased;
                        _ = &mut publication => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {
                            match self.poll("Redis acknowledgement pending", self.model.now-self.model.start.unwrap_or(self.model.now), false) {
                                Ok(false) => stopped.store(true, Ordering::Relaxed),
                                Err(error) => { stopped.store(true, Ordering::Relaxed); return Err(error); },
                                Ok(true) => {},
                            }
                        },
                    }
                }
            })
        });
        self.redis = Some(publisher);
        self.redis_waiting = false;
        result
    }
    /// Explicit final network drain. Never rely on Drop to schedule async work.
    pub fn finish(&mut self) -> Result<(), SourceError> {
        let redis_result: Result<(), SourceError> = if let Some(mut publisher) = self.redis.take() {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(publisher.finish())
            })
            .map_err(Into::into)
        } else {
            Ok(())
        };
        let file_result = self.flush();
        file_result.and(redis_result)
    }
    pub fn configure(
        &mut self,
        settings: Settings,
        sites: BTreeMap<String, Vec<String>>,
        replay: bool,
        speed: f64,
    ) {
        self.model = Model::new(settings);
        self.sites = sites;
        self.replay = replay;
        self.speed = speed;
        self.track_activity |= replay;
    }
    pub fn reset_view(&mut self) {
        self.model = Model::new(self.model.settings.clone());
        self.ui = Ui::default();
    }
    pub fn reset_activity(&mut self) {
        self.reset_view();
        self.counts.clear();
        self.decoded = 0;
        self.restart = false;
    }
    pub fn advance(&mut self, time: f64) {
        if self.track_activity {
            self.model.advance(time);
        }
    }
    pub fn message(&mut self, value: Value) -> Result<(), SourceError> {
        let parsed = if self.track_activity {
            Some(
                Message::parse(value.clone())
                    .map_err(|e| format!("invalid activity event: {e}"))?,
            )
        } else {
            None
        };
        let uf = value["uf"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(255);
        *self.counts.entry(uf).or_default() += 1;
        self.decoded += 1;
        if self.writer.is_some() || self.stdout_json || self.redis.is_some() {
            let line = serde_json::to_string(&value)?;
            self.publish(&line)?;
            if let Some(w) = self.writer.as_mut() {
                writeln!(w, "{line}")?;
                w.flush()?;
            }
            if self.stdout_json {
                let mut stdout = io::stdout().lock();
                if self.pretty {
                    writeln!(stdout, "{}", serde_json::to_string_pretty(&value)?)?;
                } else {
                    writeln!(stdout, "{line}")?;
                }
                stdout.flush()?;
            }
        }
        if let Some(m) = parsed {
            self.model.ingest(m);
        }
        Ok(())
    }
    pub fn poll(&mut self, source: &str, seconds: f64, ended: bool) -> Result<bool, SourceError> {
        if self.screen.is_none() {
            return Ok(true);
        }
        let sites = &self.sites;
        let names = |code: &str| {
            sites
                .get(code)
                .map(|v| candidate_names(v))
                .unwrap_or_else(|| "unknown".into())
        };
        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                match self.ui.key(key, &self.model, &names, self.replay) {
                    Action::Quit => return Ok(false),
                    Action::Pause => self.paused = !self.paused,
                    Action::Faster => self.speed = (self.speed.max(0.125) * 2.).min(1024.),
                    Action::Slower => self.speed = (self.speed / 2.).max(0.125),
                    Action::Restart => self.restart = true,
                    Action::None => (),
                }
            }
        }
        if ended || self.last_draw.elapsed() >= Duration::from_millis(100) {
            self.model.expire();
            let mode = if self.redis_waiting {
                "REDIS WAIT"
            } else if ended {
                "EOF"
            } else if self.replay && self.paused {
                "PAUSED"
            } else if self.replay {
                "REPLAY"
            } else {
                "RECEIVE"
            };
            let filename = std::path::Path::new(source)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(source);
            let status = format!(
                "{mode} {:.1}s{} | {}",
                seconds,
                if self.replay {
                    format!(" {}×", self.speed)
                } else {
                    String::new()
                },
                filename
            );
            let hints = |code: &str| {
                sites
                    .get(code)
                    .map(|v| {
                        v.iter()
                            .map(|s| format!("Catalogue candidate: {s}"))
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        vec![
                            "No catalogue assignment candidate. Code alone cannot locate a radar."
                                .into(),
                        ]
                    })
            };
            self.screen
                .as_mut()
                .unwrap()
                .draw(|frame| self.ui.draw(frame, &self.model, &status, &names, &hints))?;
            self.last_draw = Instant::now();
        }
        Ok(true)
    }
    pub fn flush(&mut self) -> Result<(), SourceError> {
        if let Some(w) = &mut self.writer {
            w.flush()?;
        }
        Ok(())
    }
    pub fn is_interactive(&self) -> bool {
        self.screen.is_some()
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        // Error paths still attempt a bounded drain; successful paths call finish
        // explicitly so delivery failures can propagate to the exit status.
        if !std::thread::panicking() {
            if let Err(error) = self.finish() {
                eprintln!("uplink1030: output shutdown failed: {error}");
            }
        } else {
            let _ = self.flush();
        }
        if self.screen.is_some() {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_list_all_candidates_without_provenance_or_claim_counts() {
        assert_eq!(candidate_names(&[]), "unknown");
        assert_eq!(
            candidate_names(&["Beta | source b".into(), "Alpha | source a".into()]),
            "Alpha?\nBeta?"
        );
        assert_eq!(candidate_names(&["Alpha | historical".into()]), "Alpha?");
    }
}
