//! Responsive presentation and keyboard state; contains no receiver/DSP logic.
use crate::activity::Model;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};
use std::collections::BTreeSet;

#[derive(Default)]
pub struct Ui {
    pub aircraft_focus: bool,
    radar_key: Option<String>,
    aircraft_key: Option<String>,
    query: String,
    editing: bool,
    radar_by_count: bool,
    aircraft_by_recent: bool,
    details: bool,
    detail_scroll: u16,
    help: bool,
    radar_filter: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    None,
    Quit,
    Pause,
    Faster,
    Slower,
    Restart,
}
struct DisplayRow {
    key: String,
    cells: Vec<String>,
    last: f64,
    count: usize,
}

impl Ui {
    fn sorts_by_count(&self, aircraft: bool) -> bool {
        if aircraft {
            !self.aircraft_by_recent
        } else {
            self.radar_by_count
        }
    }
    fn rows(
        &self,
        model: &Model,
        aircraft: bool,
        width: u16,
        names: &impl Fn(&str) -> String,
    ) -> Vec<DisplayRow> {
        let entries = if aircraft {
            &model.aircraft
        } else {
            &model.radars
        };
        let query = self.query.to_ascii_lowercase();
        let mut rows: Vec<_> = entries
            .iter()
            .filter_map(|(key, a)| {
                let name = if aircraft { String::new() } else { names(key) };
                let events: Vec<_> = model.recent(a).collect();
                if !key.to_ascii_lowercase().contains(&query)
                    && !name.to_ascii_lowercase().contains(&query)
                    && !events.iter().any(|e| {
                        e.code
                            .as_ref()
                            .is_some_and(|c| c.to_ascii_lowercase().contains(&query))
                    })
                {
                    return None;
                }
                if aircraft
                    && self
                        .radar_filter
                        .as_ref()
                        .is_some_and(|code| !events.iter().any(|e| e.code.as_ref() == Some(code)))
                {
                    return None;
                }
                let count = events.len();
                let age = format!("{:.1}", (model.now - a.last).max(0.));
                let cells = if aircraft {
                    let codes = events
                        .iter()
                        .filter_map(|e| e.code.as_deref())
                        .collect::<BTreeSet<_>>();
                    let last = a.latest.as_ref();
                    let heard = last.is_some_and(|m| m.value["seen_1090"].as_bool() == Some(true));
                    let mut c = vec![
                        format!("{key}{}", if heard { " ✓" } else { "" }),
                        age,
                        count.to_string(),
                        format!("{:.2}", count as f64 / model.denominator()),
                        codes.into_iter().collect::<Vec<_>>().join(","),
                        last.map(|m| m.uf.to_string()).unwrap_or_default(),
                    ];
                    if width >= 90 {
                        c.push(
                            last.and_then(|m| m.value["callsign"].as_str())
                                .unwrap_or_default()
                                .to_owned(),
                        );
                    }
                    if width >= 70 {
                        c.push(last.and_then(|m| m.bds.clone()).unwrap_or_default());
                        c.push(
                            last.and_then(|m| {
                                crate::schema::observation(&m.value, "rssi").as_f64()
                            })
                            .map(|r| format!("{r:.1}"))
                            .unwrap_or_else(|| "—".into()),
                        );
                    }
                    c
                } else {
                    let all = events.iter().filter(|e| e.uf == 11).count();
                    let targets = events
                        .iter()
                        .filter_map(|e| e.address.as_ref())
                        .collect::<BTreeSet<_>>()
                        .len();
                    let addressed = events.iter().filter(|e| e.address.is_some()).count();
                    let scan = a
                        .scan(model.now)
                        .map(|s| {
                            format!(
                                "{}{:.2}{}",
                                if s.known { "" } else { "~" },
                                s.period,
                                if s.ambiguous { "?" } else { "" }
                            )
                        })
                        .unwrap_or_else(|| "—".into());
                    let mut c = vec![key.clone()];
                    if width >= 80 {
                        c.push(name);
                    }
                    c.extend([
                        age,
                        format!("{:.2}", all as f64 / model.denominator()),
                        format!("{:.2}", addressed as f64 / model.denominator()),
                        targets.to_string(),
                        scan,
                    ]);
                    c
                };
                Some(DisplayRow {
                    key: key.clone(),
                    cells,
                    last: a.last,
                    count,
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            if self.sorts_by_count(aircraft) {
                b.count
                    .cmp(&a.count)
                    .then_with(|| b.last.total_cmp(&a.last))
            } else {
                b.last
                    .total_cmp(&a.last)
                    .then_with(|| b.count.cmp(&a.count))
            }
            .then_with(|| a.key.cmp(&b.key))
        });
        rows
    }
    pub fn key(
        &mut self,
        key: KeyEvent,
        model: &Model,
        names: &impl Fn(&str) -> String,
        replay: bool,
    ) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        if self.editing {
            match key.code {
                KeyCode::Esc => {
                    self.query.clear();
                    self.editing = false;
                }
                KeyCode::Enter => self.editing = false,
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(c) => self.query.push(c),
                _ => (),
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Esc if self.help => self.help = false,
            KeyCode::Esc if self.details => self.details = false,
            KeyCode::Esc if self.radar_filter.is_some() || !self.query.is_empty() => {
                self.radar_filter = None;
                self.query.clear();
            }
            KeyCode::Esc => return Action::Quit,
            KeyCode::Tab | KeyCode::BackTab => {
                self.aircraft_focus = !self.aircraft_focus;
                self.detail_scroll = 0;
            }
            KeyCode::Char('/') => self.editing = true,
            KeyCode::Char('s') => {
                if self.aircraft_focus {
                    self.aircraft_by_recent = !self.aircraft_by_recent;
                } else {
                    self.radar_by_count = !self.radar_by_count;
                }
            }
            KeyCode::Char('d') | KeyCode::Enter => {
                self.details = !self.details;
                self.detail_scroll = 0;
            }
            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Char('f') => {
                self.radar_filter = if self.radar_filter.is_some() {
                    None
                } else {
                    self.radar_key.clone()
                };
            }
            KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End => {
                let rows = self.rows(model, self.aircraft_focus, 80, names);
                let selected = if self.aircraft_focus {
                    &mut self.aircraft_key
                } else {
                    &mut self.radar_key
                };
                let i = selected
                    .as_ref()
                    .and_then(|k| rows.iter().position(|r| &r.key == k))
                    .unwrap_or(0);
                let next = match key.code {
                    KeyCode::Up => i.saturating_sub(1),
                    KeyCode::Down => (i + 1).min(rows.len().saturating_sub(1)),
                    KeyCode::Home => 0,
                    _ => rows.len().saturating_sub(1),
                };
                *selected = rows.get(next).map(|r| r.key.clone());
                self.detail_scroll = 0;
            }
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(5),
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(5),
            KeyCode::Char(' ') if replay => return Action::Pause,
            KeyCode::Char('+') | KeyCode::Char('=') if replay => return Action::Faster,
            KeyCode::Char('-') if replay => return Action::Slower,
            KeyCode::Char('r') if replay => return Action::Restart,
            _ => (),
        }
        Action::None
    }
    pub fn draw(
        &mut self,
        frame: &mut Frame,
        model: &Model,
        status: &str,
        names: &impl Fn(&str) -> String,
        hints: &impl Fn(&str) -> Vec<String>,
    ) {
        let area = frame.area();
        let main = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
        let (mode, source) = status.split_once(" | ").unwrap_or((status, ""));
        frame.render_widget(
            Paragraph::new(format!(
                "uplink1030 | {mode} | {} msgs | {} unattributed{}{}",
                model.total,
                model.unattributed,
                if model.now - model.start.unwrap_or(model.now) < model.settings.window {
                    " | warming"
                } else {
                    ""
                },
                if area.width >= 120 && !source.is_empty() {
                    format!(" | {source}")
                } else {
                    String::new()
                }
            ))
            .style(Style::default().fg(Color::Cyan)),
            main[0],
        );
        let (tables, detail) = if self.details && area.width >= 140 && area.height >= 18 {
            let r = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(main[1]);
            (r[0], Some(r[1]))
        } else if self.details && area.height >= 24 {
            let r = Layout::vertical([Constraint::Min(8), Constraint::Length(9)]).split(main[1]);
            (r[0], Some(r[1]))
        } else if self.details {
            (Rect::default(), Some(main[1]))
        } else {
            (main[1], None)
        };
        if tables.height > 0 {
            if tables.height >= 16 && tables.width >= 50 {
                let parts =
                    Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)])
                        .split(tables);
                self.table(frame, parts[0], model, false, names);
                self.table(frame, parts[1], model, true, names);
            } else {
                self.table(frame, tables, model, self.aircraft_focus, names);
            }
        }
        if let Some(rect) = detail {
            let key = if self.aircraft_focus {
                &self.aircraft_key
            } else {
                &self.radar_key
            };
            let activity = key.as_ref().and_then(|k| {
                if self.aircraft_focus {
                    model.aircraft.get(k)
                } else {
                    model.radars.get(k)
                }
            });
            let mut lines = activity
                .map(|a| model.describe(a))
                .unwrap_or_else(|| vec!["Select a row with arrow keys. Tab changes panel.".into()]);
            if !self.aircraft_focus {
                if let Some(k) = key {
                    lines.extend(hints(k));
                }
            }
            let title = format!(
                "{} {} | PgUp/PgDn scroll",
                if self.aircraft_focus {
                    "Aircraft candidate"
                } else {
                    "Radar-code population"
                },
                key.as_deref().unwrap_or("")
            );
            frame.render_widget(
                Paragraph::new(lines.join("\n"))
                    .wrap(Wrap { trim: false })
                    .scroll((self.detail_scroll, 0))
                    .block(Block::bordered().title(title)),
                rect,
            );
        }
        let help = if self.editing {
            format!("Filter: {}▏  Enter accept / Esc clear", self.query)
        } else {
            format!("Tab panel  ↑↓ select  Enter details  / filter  s sort:{}  f link  ? help  q quit{}",if self.sorts_by_count(self.aircraft_focus) {"count"} else {"recent"},if self.query.is_empty(){String::new()}else{format!("  /{}",self.query)})
        };
        frame.render_widget(
            Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
            main[2],
        );
        if self.help {
            let rect = Rect::new(
                area.x + 1,
                area.y + 1,
                area.width.saturating_sub(2),
                area.height.saturating_sub(2),
            );
            frame.render_widget(Clear, rect);
            frame.render_widget(Paragraph::new("Tab: focus radar / aircraft panel\n↑ ↓ Home End: select and scroll; selection follows identity\nEnter / d: toggle details; PgUp/PgDn: scroll details\n/: edit shared address/code/site filter; Enter accepts; Esc clears\ns: toggle focused panel's recent / rolling-count sort\nf: toggle aircraft filter for selected radar; Esc clears\nReplay: Space pause, +/- speed, r restart\nq / Ctrl-C: quit; Esc closes detail/filter before quitting\n\nRates and targets use the trailing capture-time window.\nTargets are candidate addresses, not independently verified aircraft.\n✓ = address heard on 1090 (jet1090 on --redis-url, or --downlink file) before the interrogation.\n~ scan = candidate estimate; ? = mixed / single-target ambiguity.\nNo estimate means insufficient same-target recurrence.\nRadar code reuse and missed rotations can mix scan estimates.\nCatalogue site names are hypotheses, not verified assignments.\nDisplay filters never change JSON recording.").wrap(Wrap{trim:false}).block(Block::bordered().title("Help")),rect);
        }
    }
    fn table(
        &mut self,
        frame: &mut Frame,
        rect: Rect,
        model: &Model,
        aircraft: bool,
        names: &impl Fn(&str) -> String,
    ) {
        if rect.width < 48 || rect.height < 4 {
            frame.render_widget(
                Paragraph::new("Enlarge terminal (48+ columns); Tab switches panel")
                    .wrap(Wrap { trim: false }),
                rect,
            );
            return;
        }
        let rows = self.rows(model, aircraft, rect.width, names);
        let key = if aircraft {
            &mut self.aircraft_key
        } else {
            &mut self.radar_key
        };
        let selected = key
            .as_ref()
            .and_then(|k| rows.iter().position(|r| &r.key == k))
            .or(if rows.is_empty() { None } else { Some(0) });
        *key = selected.map(|i| rows[i].key.clone());
        let mut state = TableState::default().with_selected(selected);
        let (headers, widths) = if aircraft {
            let mut h = vec!["ICAO24", "Age/s", "Count", "Msg/s", "Codes", "UF"];
            let mut w = vec![
                Constraint::Length(8),
                Constraint::Length(6),
                Constraint::Length(5),
                Constraint::Length(7),
                Constraint::Min(6),
                Constraint::Length(3),
            ];
            if rect.width >= 90 {
                h.push("Callsign");
                w.push(Constraint::Length(8));
            }
            if rect.width >= 70 {
                h.extend(["BDS", "dB"]);
                w.extend([Constraint::Length(3), Constraint::Length(6)]);
            }
            (h, w)
        } else {
            let mut h = vec!["Code"];
            let mut w = vec![Constraint::Length(5)];
            if rect.width >= 80 {
                h.push("Site?");
                w.push(Constraint::Min(10));
            }
            h.extend(["Age/s", "UF11/s", "Addr/s", "Tgts?", "Scan/s"]);
            w.extend([
                Constraint::Length(6),
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Length(5),
                Constraint::Length(8),
            ]);
            (h, w)
        };
        let focus = self.aircraft_focus == aircraft;
        let title = if aircraft {
            let heard = model
                .aircraft
                .values()
                .filter(|a| {
                    a.latest
                        .as_ref()
                        .is_some_and(|m| m.value["seen_1090"].as_bool() == Some(true))
                })
                .count();
            format!(
                "Aircraft candidates {}{}{}",
                rows.len(),
                if heard > 0 {
                    format!(" | {heard} ✓ heard on 1090")
                } else {
                    String::new()
                },
                self.radar_filter
                    .as_ref()
                    .map(|s| format!(" | FILTER {s}"))
                    .unwrap_or_default()
            )
        } else {
            format!("Radar codes {} | targets unverified", rows.len())
        };
        let t = Table::new(
            rows.into_iter().map(|r| {
                let height = r
                    .cells
                    .iter()
                    .map(|cell| cell.lines().count())
                    .max()
                    .unwrap_or(1)
                    .min(u16::MAX as usize) as u16;
                Row::new(r.cells).height(height.max(1))
            }),
            widths,
        )
        .column_spacing(1)
        .header(Row::new(headers).style(Style::default().add_modifier(Modifier::BOLD)))
        .row_highlight_style(
            Style::default()
                .bg(if focus { Color::DarkGray } else { Color::Reset })
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(if focus { "› " } else { "  " })
        .block(
            Block::bordered()
                .title(title)
                .border_style(Style::default().fg(if focus {
                    Color::Cyan
                } else {
                    Color::DarkGray
                })),
        );
        frame.render_stateful_widget(t, rect, &mut state);
        if model.total == 0 {
            frame.render_widget(
                Paragraph::new("Waiting for interrogations…"),
                Rect::new(
                    rect.x + 2,
                    rect.y + 3,
                    rect.width.saturating_sub(4),
                    rect.height.saturating_sub(4).min(1),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Message, Settings};
    use ratatui::{backend::TestBackend, Terminal};
    use serde_json::json;
    fn fixture() -> Model {
        let mut m = Model::new(Settings::default());
        for i in 0..100 {
            m.ingest(Message::parse(json!({"timestamp":100.+i as f64*0.1,"uf":"4","icao24":format!("{i:06x}"),"interrogator":"II3","frame":"20970000b1c8a9"})).unwrap());
        }
        m
    }
    #[test]
    fn aircraft_default_to_count_and_panel_sorting_is_independent() {
        let mut model = Model::new(Settings::default());
        for (time, address, code) in [
            (100., "aaaaaa", "II3"),
            (101., "aaaaaa", "II3"),
            (102., "bbbbbb", "II4"),
        ] {
            model.ingest(Message::parse(json!({"timestamp":time,"uf":"4","icao24":address,"interrogator":code,"frame":"20970000b1c8a9"})).unwrap());
        }
        let mut ui = Ui::default();
        let names = |_: &str| String::new();
        let rows = ui.rows(&model, true, 80, &names);
        assert_eq!(rows[0].key, "aaaaaa");
        assert_eq!(rows[0].cells[2], "2");
        assert_eq!(ui.rows(&model, false, 80, &names)[0].key, "II4");
        ui.aircraft_focus = true;
        ui.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            &model,
            &names,
            false,
        );
        assert_eq!(ui.rows(&model, true, 80, &names)[0].key, "bbbbbb");
        assert_eq!(ui.rows(&model, false, 80, &names)[0].key, "II4");
        ui.aircraft_focus = false;
        ui.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            &model,
            &names,
            false,
        );
        assert_eq!(ui.rows(&model, false, 80, &names)[0].key, "II3");
        assert_eq!(ui.rows(&model, true, 80, &names)[0].key, "bbbbbb");
    }
    #[test]
    fn radar_candidates_render_on_separate_lines_and_all_are_searchable() {
        let model = fixture();
        let mut ui = Ui::default();
        let names = |_: &str| "Alpha?\nBeta?".to_owned();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| ui.draw(f, &model, "REPLAY", &names, &|_| vec![]))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let lines: Vec<_> = (0..24)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect();
        let alpha = lines.iter().position(|l| l.contains("Alpha?")).unwrap();
        let beta = lines.iter().position(|l| l.contains("Beta?")).unwrap();
        assert_eq!(beta, alpha + 1);
        ui.query = "beta".into();
        assert_eq!(ui.rows(&model, false, 80, &names).len(), 1);
    }
    #[test]
    fn aircraft_heard_on_1090_are_marked_with_their_callsign() {
        let mut model = Model::new(Settings::default());
        model.ingest(Message::parse(json!({"timestamp":100.,"uf":"4","icao24":"4ca242","interrogator":"II3","frame":"20970000b1c8a9","seen_1090":true,"seen_1090_age":3.2,"callsign":"RYR67RW"})).unwrap());
        model.ingest(Message::parse(json!({"timestamp":100.5,"uf":"4","icao24":"abcdef","interrogator":"II3","frame":"20970000b1c8a9","seen_1090":false})).unwrap());
        assert_eq!(
            model.aircraft["4ca242"].latest.as_ref().unwrap().known,
            Some(true)
        );
        assert_eq!(
            model.aircraft["abcdef"].latest.as_ref().unwrap().known,
            Some(false)
        );
        let mut ui = Ui::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|f| {
                ui.draw(
                    f,
                    &model,
                    "REPLAY · 1090 1 aircraft",
                    &|_| String::new(),
                    &|_| vec![],
                )
            })
            .unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(content.contains("4ca242 ✓"));
        assert!(content.contains("RYR67RW"));
        assert!(content.contains("1 ✓ heard on 1090"));
        assert!(content.contains("1090 1 aircraft"));
    }
    #[test]
    fn code_search_and_explicit_radar_link_keep_aircraft_visible() {
        let m = fixture();
        let mut ui = Ui {
            query: "II3".into(),
            radar_filter: Some("II3".into()),
            ..Ui::default()
        };
        assert_eq!(ui.rows(&m, true, 80, &|_| String::new()).len(), 100);
        ui.radar_filter = Some("II4".into());
        assert!(ui.rows(&m, true, 80, &|_| String::new()).is_empty());
        assert_eq!(m.total, 100);
    }
    #[test]
    fn renders_responsive_sizes_and_scrolls_to_selected_row() {
        let m = fixture();
        let mut ui = Ui::default();
        for (w, h) in [(80, 24), (120, 30), (180, 40), (40, 12), (10, 4)] {
            let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
            t.draw(|f| ui.draw(f, &m, "REPLAY", &|_| "unknown".into(), &|_| vec![]))
                .unwrap();
            ui.details = true;
            t.draw(|f| ui.draw(f, &m, "EOF", &|_| "unknown".into(), &|_| vec![]))
                .unwrap();
            ui.details = false;
        }
        ui.aircraft_focus = true;
        ui.key(
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            &m,
            &|_| String::new(),
            false,
        );
        let selected = ui.aircraft_key.clone().unwrap();
        ui.aircraft_by_recent = true;
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| ui.draw(f, &m, "LIVE", &|_| String::new(), &|_| vec![]))
            .unwrap();
        assert_eq!(ui.aircraft_key.as_ref(), Some(&selected));
        let content = t
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(content.contains(&selected));
    }
}
