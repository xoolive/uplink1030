//! JSONL and interactive output share the same recovered messages.
use crate::source::SourceError;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    widgets::{Block, Paragraph, Row, Table},
    Terminal,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};

type Screen = Terminal<CrosstermBackend<io::Stdout>>;

#[derive(Default)]
struct AircraftRow {
    count: usize,
    ufs: BTreeMap<u8, usize>,
    bds: BTreeMap<String, usize>,
    latest: Vec<String>,
}

#[derive(Default)]
struct AircraftTable(BTreeMap<String, AircraftRow>);

fn text(v: &Value) -> String {
    if v.is_null() {
        String::new()
    } else {
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
    }
}

impl AircraftTable {
    fn observe(&mut self, uf: u8, value: &Value) {
        let m = &value["message"];
        let address = text(&m["icao24"]).to_ascii_lowercase();
        let key = if uf == 11 || address == "ffffff" {
            "*".to_owned()
        } else {
            address
        };
        let row = self.0.entry(key).or_default();
        row.count += 1;
        *row.ufs.entry(uf).or_default() += 1;
        let bds = text(&m["bds"]).to_ascii_lowercase();
        if !bds.is_empty() {
            *row.bds.entry(bds.clone()).or_default() += 1;
        }
        let interrogator = if uf == 11 {
            let ic = m["ic"].as_u64().unwrap_or(0);
            let cl = m["cl"].as_u64().unwrap_or(0);
            if cl == 0 {
                format!("II{ic}")
            } else {
                format!("SI{}", (cl - 1) * 16 + ic)
            }
        } else if !m["sd"]["sis"].is_null() {
            format!("SI{}", text(&m["sd"]["sis"]))
        } else {
            text(&m["sd"]["iis"])
        };
        // Replace, rather than merge, last-message fields: absent RR/BDS/SD
        // must clear values left by an earlier format for this aircraft.
        row.latest = vec![
            format!("{:.6}", value["seconds"].as_f64().unwrap_or_default()),
            format!("UF{uf}"),
            text(&m["rr"]),
            bds,
            interrogator,
            format!(
                "{:.3}",
                value["decision_confidence"].as_f64().unwrap_or_default()
            ),
            text(&value["address_validation"]),
            text(&m["raw"]),
        ];
    }

    fn rows(&self) -> Vec<Vec<String>> {
        let mut entries: Vec<_> = self.0.iter().collect();
        // Keep the single all-call summary first; rank aircraft by activity.
        // Equal counts use ICAO24 order, avoiding redraw-dependent ties.
        entries.sort_by(|(a, ra), (b, rb)| {
            (b.as_str() == "*")
                .cmp(&(a.as_str() == "*"))
                .then_with(|| rb.count.cmp(&ra.count))
                .then_with(|| a.cmp(b))
        });
        entries
            .into_iter()
            .map(|(address, row)| {
                let mut cells = vec![
                    address.clone(),
                    row.count.to_string(),
                    row.ufs
                        .iter()
                        .map(|(uf, n)| format!("{uf}:{n}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                    row.bds
                        .iter()
                        .map(|(bds, n)| format!("{bds}:{n}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                ];
                cells.extend(row.latest.clone());
                cells
            })
            .collect()
    }
}

pub struct Output {
    writer: Option<Box<dyn Write>>,
    stdout_json: bool,
    pretty: bool,
    screen: Option<Screen>,
    last_draw: Instant,
    aircraft: AircraftTable,
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
            .map(|p| {
                std::fs::File::create(p).map(|f| Box::new(io::BufWriter::new(f)) as Box<dyn Write>)
            })
            .transpose()?;
        let mut result = Self {
            writer,
            stdout_json: !interactive && !stats,
            pretty,
            screen: None,
            last_draw: Instant::now() - Duration::from_secs(1),
            aircraft: AircraftTable::default(),
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
    pub fn message(&mut self, value: Value) -> Result<(), SourceError> {
        let uf = value["message"]["type"]
            .as_str()
            .unwrap_or("uf?")
            .trim_start_matches("uf")
            .parse()
            .unwrap_or(255);
        *self.counts.entry(uf).or_default() += 1;
        self.decoded += 1;
        let line = serde_json::to_string(&value)?;
        if let Some(writer) = self.writer.as_mut() {
            writeln!(writer, "{line}")?;
        }
        if self.stdout_json {
            let mut stdout = io::stdout().lock();
            if self.pretty {
                writeln!(stdout, "{}", serde_json::to_string_pretty(&value)?)?;
            } else {
                writeln!(stdout, "{line}")?;
            }
        }
        if self.screen.is_some() {
            self.aircraft.observe(uf, &value);
        }
        Ok(())
    }
    pub fn poll(&mut self, source: &str, seconds: f64, force: bool) -> Result<bool, SourceError> {
        if self.screen.is_none() {
            return Ok(true);
        }
        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q')
                    || key.code == KeyCode::Esc
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL))
                {
                    return Ok(false);
                }
            }
        }
        if force || self.last_draw.elapsed() >= Duration::from_millis(100) {
            let counts = self
                .counts
                .iter()
                .map(|(u, n)| format!("UF{u}:{n}"))
                .collect::<Vec<_>>()
                .join("  ");
            let title = format!(
                "uplink1030 | {:.3}s | {} messages | {}",
                seconds, self.decoded, counts
            );
            let rows = self.aircraft.rows();
            let uf_width = rows
                .iter()
                .map(|r| r[2].len())
                .max()
                .unwrap_or(0)
                .clamp(9, 40) as u16;
            let bds_width = rows
                .iter()
                .map(|r| r[3].len())
                .max()
                .unwrap_or(0)
                .clamp(10, 64) as u16;
            self.screen.as_mut().unwrap().draw(|frame| {
                let areas=Layout::vertical([Constraint::Length(3),Constraint::Min(3),Constraint::Length(3)]).split(frame.area());
                frame.render_widget(Paragraph::new(source).block(Block::bordered().title(title)),areas[0]);
                let available=areas[1].height.saturating_sub(3) as usize;
                let table=Table::new(rows.iter().take(available).map(|r|Row::new(r.clone())),[Constraint::Length(8),Constraint::Length(6),Constraint::Length(uf_width),Constraint::Length(bds_width),Constraint::Length(12),Constraint::Length(5),Constraint::Length(3),Constraint::Length(5),Constraint::Length(6),Constraint::Length(6),Constraint::Length(20),Constraint::Min(14)])
                    .header(Row::new(["ICAO24","Count","UF counts","BDS counts","Last (s)","UF","RR","BDS","IIS/IC","Conf.","Address evidence","Raw"]))
                    .block(Block::bordered().title("Aircraft activity — * all-call; counts cumulative, other fields latest; addressed messages are candidates"));
                frame.render_widget(table,areas[1]);
                frame.render_widget(Paragraph::new(if force {"Source ended. q / Esc to close. JSON file contains the same events."}else{"q / Esc / Ctrl-C: stop | timestamps are nominal sample-clock times, not synchronized RF reception"}).block(Block::bordered()),areas[2]);
            })?;
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
        let _ = self.flush();
        if self.screen.is_some() {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(address: &str, uf: u8, bds: Option<&str>, seconds: f64) -> Value {
        json!({"message":{"icao24":address,"type":format!("uf{uf}"),"bds":bds,"rr":21,"sd":{"iis":3},"raw":"previous"},"seconds":seconds,"decision_confidence":0.97,"address_validation":"unverified"})
    }

    #[test]
    fn accumulates_counts_but_replaces_all_latest_fields() {
        let mut table = AircraftTable::default();
        table.observe(4, &event("39BDA4", 4, Some("0x50"), 1.));
        table.observe(4, &event("39bda4", 4, Some("0x50"), 2.));
        table.observe(5, &event("39bda4", 5, Some("0x60"), 3.));
        table.observe(
            0,
            &json!({"message":{"icao24":"39bda4","type":"uf0","raw":"latest"},"seconds":4.}),
        );
        let rows = table.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            &rows[0][..5],
            ["39bda4", "4", "0:1 4:2 5:1", "0x50:2 0x60:1", "4.000000"]
        );
        assert_eq!(&rows[0][5..9], ["UF0", "", "", ""]);
        assert_eq!(rows[0][9], "0.000");
        assert_eq!(rows[0][10], "");
        assert_eq!(rows[0][11], "latest");
    }

    #[test]
    fn combines_all_calls_and_ranks_aircraft_by_count() {
        let mut table = AircraftTable::default();
        for _ in 0..3 {
            table.observe(4, &event("bbbbbb", 4, None, 0.));
        }
        table.observe(4, &event("aaaaaa", 4, None, 0.));
        table.observe(4, &event("cccccc", 4, None, 0.));
        table.observe(11, &json!({"message":{"icao24":"ffffff","ic":3,"cl":0}}));
        table.observe(11, &json!({"message":{"icao24":"ffffff","ic":5,"cl":4}}));
        let rows = table.rows();
        assert_eq!(
            rows.iter().map(|r| r[0].as_str()).collect::<Vec<_>>(),
            ["*", "bbbbbb", "aaaaaa", "cccccc"]
        );
        assert_eq!(&rows[0][1..4], ["2", "11:2", ""]);
        assert_eq!(rows[0][8], "SI53");
        table.observe(4, &event("aaaaaa", 4, None, 0.));
        table.observe(4, &event("aaaaaa", 4, None, 0.));
        table.observe(4, &event("aaaaaa", 4, None, 0.));
        assert_eq!(table.rows()[1][0], "aaaaaa");
    }

    #[test]
    fn counters_do_not_roll_off_after_two_hundred_messages() {
        let mut table = AircraftTable::default();
        for n in 0..250 {
            table.observe(4, &event("39bda4", 4, Some("0x50"), n as f64));
        }
        assert_eq!(&table.rows()[0][1..4], ["250", "4:250", "0x50:250"]);
    }
}
