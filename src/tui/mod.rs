//! Split-pane dashboard. All slow work (file reads, model calls, patch writes,
//! the ad fetch) runs in tokio tasks and reports back over a channel; the UI
//! thread only drains that channel, handles keys and draws.

mod highlight;
mod markdown;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, List, ListItem, ListState, Paragraph, Wrap},
    DefaultTerminal, Frame,
};
use tokio::sync::{mpsc, Semaphore};

use crate::{
    ads,
    config::Config,
    engine::{self, Engine},
    model::{self, Edit, Finding, Severity},
    scan,
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

enum Msg {
    Files(Result<Vec<PathBuf>, String>),
    Loaded(PathBuf, String),
    Done(PathBuf, u64, Result<Vec<Finding>, String>),
    Applied(PathBuf, Result<(), String>),
    Ad(String),
}

enum State {
    Queued,
    Scanning,
    Done(Vec<Finding>),
    Failed(String),
}

struct Entry {
    path: PathBuf,
    rel: String,
    ext: String,
    source: Option<Arc<String>>,
    lines: Vec<String>,
    state: State,
    /// Bumped on every rescan so late results from an older run are dropped.
    gen: u64,
    log: Vec<Line<'static>>,
}

impl Entry {
    fn new(path: PathBuf, root: &Path) -> Self {
        let rel = if root.is_file() {
            path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            path.strip_prefix(root).unwrap_or(&path).display().to_string()
        };
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        Self { path, rel, ext, source: None, lines: Vec::new(), state: State::Queued, gen: 0, log: Vec::new() }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Files,
    Code,
    Log,
}

struct App {
    cfg: Config,
    engine: Arc<dyn Engine>,
    root: PathBuf,
    tx: mpsc::UnboundedSender<Msg>,
    sem: Arc<Semaphore>,
    entries: Vec<Entry>,
    list: ListState,
    focus: Focus,
    code_scroll: usize,
    log_scroll: usize,
    code_h: usize,
    status: String,
    ad: Option<String>,
    started: Instant,
    quit: bool,
    dirty: bool,
}

pub async fn run(cfg: Config, root: PathBuf, force_local: bool) -> Result<i32> {
    let engine = engine::build(&cfg, force_local)?;
    let (tx, mut rx) = mpsc::unbounded_channel();

    let mut app = App {
        sem: Arc::new(Semaphore::new(cfg.concurrency.max(1))),
        cfg,
        engine,
        root,
        tx,
        entries: Vec::new(),
        list: ListState::default(),
        focus: Focus::Files,
        code_scroll: 0,
        log_scroll: 0,
        code_h: 0,
        status: "indexing...".into(),
        ad: None,
        started: Instant::now(),
        quit: false,
        dirty: true,
    };
    app.boot();

    let mut terminal = ratatui::init();
    let res = event_loop(&mut terminal, &mut app, &mut rx);
    ratatui::restore();
    res.map(|_| 0)
}

fn event_loop(
    term: &mut DefaultTerminal,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
) -> Result<()> {
    let mut last_spin = Instant::now();
    while !app.quit {
        while let Ok(m) = rx.try_recv() {
            app.on_msg(m);
            app.dirty = true;
        }
        if app.busy() && last_spin.elapsed() >= Duration::from_millis(100) {
            app.dirty = true;
            last_spin = Instant::now();
        }
        if app.dirty {
            term.draw(|f| ui(f, app))?;
            app.dirty = false;
        }
        // ~60 polls a second; the draw above only runs when something changed.
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k),
                _ => {}
            }
            app.dirty = true;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- state

impl App {
    fn cur(&self) -> Option<&Entry> {
        self.list.selected().and_then(|i| self.entries.get(i))
    }

    fn busy(&self) -> bool {
        self.entries.iter().any(|e| matches!(e.state, State::Queued | State::Scanning))
    }

    fn spinner(&self) -> &'static str {
        SPINNER[(self.started.elapsed().as_millis() / 100) as usize % SPINNER.len()]
    }

    fn border(&self, f: Focus) -> Style {
        if self.focus == f {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::DarkGray)
        }
    }

    fn boot(&mut self) {
        let tx = self.tx.clone();
        let root = self.root.clone();
        let max = self.cfg.max_file_bytes;
        tokio::spawn(async move {
            let r = tokio::task::spawn_blocking(move || scan::collect(&root, max)).await;
            let r = match r {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(e)) => Err(format!("{e:#}")),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(Msg::Files(r));
        });

        if self.cfg.ads.enabled {
            let tx = self.tx.clone();
            let cfg = self.cfg.clone();
            tokio::spawn(async move {
                if let Some(t) = ads::get(&cfg).await {
                    let _ = tx.send(Msg::Ad(t));
                }
            });
        }
    }

    fn scan_entry(&mut self, i: usize) {
        let e = &mut self.entries[i];
        e.gen += 1;
        e.state = State::Scanning;
        e.log.clear();
        let (path, gen) = (e.path.clone(), e.gen);

        let tx = self.tx.clone();
        let sem = self.sem.clone();
        let engine = self.engine.clone();
        let chunk = self.cfg.chunk_lines;

        tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            let text = match scan::read_text(&path).await {
                Ok(t) => t,
                Err(e) => {
                    let _ = tx.send(Msg::Done(path, gen, Err(format!("{e:#}"))));
                    return;
                }
            };
            let _ = tx.send(Msg::Loaded(path.clone(), text.clone()));
            let r = engine::audit_source(engine.as_ref(), &text, chunk)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Done(path, gen, r));
        });
    }

    fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Files(Err(e)) => self.status = model::sanitize(&e, false),
            Msg::Files(Ok(v)) => {
                self.entries = v.into_iter().map(|p| Entry::new(p, &self.root)).collect();
                if self.entries.is_empty() {
                    self.status = "no source files found".into();
                    return;
                }
                self.status.clear();
                self.list.select(Some(0));
                for i in 0..self.entries.len() {
                    self.scan_entry(i);
                }
            }
            Msg::Loaded(p, text) => {
                if let Some(e) = self.entries.iter_mut().find(|e| e.path == p) {
                    e.lines = text
                        .lines()
                        .map(|l| model::sanitize(&l.replace('\t', "    "), false))
                        .collect();
                    e.source = Some(Arc::new(text));
                }
            }
            Msg::Done(p, gen, res) => {
                let Some(i) = self.entries.iter().position(|e| e.path == p) else { return };
                if self.entries[i].gen != gen {
                    return;
                }
                self.entries[i].state = match res {
                    Ok(f) => State::Done(f),
                    Err(m) => State::Failed(model::sanitize(&m, false)),
                };
                self.rebuild_log(i);
            }
            Msg::Applied(p, res) => match res {
                Ok(()) => {
                    self.status = "patch applied, rescanning".into();
                    if let Some(i) = self.entries.iter().position(|e| e.path == p) {
                        self.scan_entry(i);
                    }
                }
                Err(m) => self.status = format!("patch failed: {}", model::sanitize(&m, false)),
            },
            Msg::Ad(t) => self.ad = Some(t),
        }
    }

    fn rebuild_log(&mut self, i: usize) {
        let e = &mut self.entries[i];
        if let (State::Done(f), Some(src)) = (&e.state, &e.source) {
            let md = markdown::report(&e.rel, f, src);
            e.log = markdown::render(&md);
        }
    }

    fn apply_patch(&mut self) {
        let Some(e) = self.cur() else { return };
        let (State::Done(found), Some(src)) = (&e.state, &e.source) else {
            self.status = "nothing to patch yet".into();
            return;
        };
        let edits: Vec<&Edit> = found.iter().filter_map(|f| f.fix.as_ref()).collect();
        if edits.is_empty() {
            self.status = "no fixes proposed for this file".into();
            return;
        }
        let new = model::apply_edits(src, &edits);
        if new == src.as_str() {
            self.status = "patch changes nothing".into();
            return;
        }

        let path = e.path.clone();
        let tx = self.tx.clone();
        self.status = "writing patch...".into();
        tokio::spawn(async move {
            let r = write_with_backup(&path, &new).await.map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Applied(path, r));
        });
    }

    fn on_key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Files => Focus::Code,
                    Focus::Code => Focus::Log,
                    Focus::Log => Focus::Files,
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::PageUp => self.step(-10),
            KeyCode::PageDown => self.step(10),
            KeyCode::Char('p') => self.apply_patch(),
            KeyCode::Char('r') => {
                if let Some(i) = self.list.selected() {
                    self.status.clear();
                    self.scan_entry(i);
                }
            }
            KeyCode::Char('R') => {
                self.status.clear();
                for i in 0..self.entries.len() {
                    self.scan_entry(i);
                }
            }
            KeyCode::Char('n') => self.jump(1),
            KeyCode::Char('N') => self.jump(-1),
            _ => {}
        }
    }

    fn step(&mut self, d: isize) {
        match self.focus {
            Focus::Files => {
                let n = self.entries.len();
                if n == 0 {
                    return;
                }
                let cur = self.list.selected().unwrap_or(0);
                let next = shift(cur, d, n - 1);
                if next != cur {
                    self.list.select(Some(next));
                    self.code_scroll = 0;
                    self.log_scroll = 0;
                }
            }
            Focus::Code => {
                let max = self.cur().map_or(0, |e| e.lines.len().saturating_sub(1));
                self.code_scroll = shift(self.code_scroll, d, max);
            }
            Focus::Log => {
                let max = self.cur().map_or(0, |e| e.log.len().saturating_sub(1));
                self.log_scroll = shift(self.log_scroll, d, max);
            }
        }
    }

    /// Scroll the code pane to the next (or previous) finding.
    fn jump(&mut self, dir: isize) {
        let Some(e) = self.cur() else { return };
        let State::Done(found) = &e.state else { return };
        let mut t: Vec<usize> = found.iter().map(|f| f.line.saturating_sub(3)).collect();
        t.sort_unstable();
        t.dedup();
        let Some(&last) = t.last() else { return };
        let cur = self.code_scroll;
        self.code_scroll = if dir > 0 {
            t.iter().copied().find(|&x| x > cur).unwrap_or(t[0])
        } else {
            t.iter().rev().copied().find(|&x| x < cur).unwrap_or(last)
        };
    }
}

fn shift(v: usize, d: isize, max: usize) -> usize {
    (v as isize + d).clamp(0, max as isize) as usize
}

async fn write_with_backup(path: &Path, new: &str) -> Result<()> {
    let mut bak = path.as_os_str().to_owned();
    bak.push(".orig");
    let bak = PathBuf::from(bak);
    if tokio::fs::metadata(&bak).await.is_err() {
        tokio::fs::copy(path, &bak).await?;
    }

    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".auditor-tmp");
    let tmp = PathBuf::from(tmp);
    tokio::fs::write(&tmp, new).await?;
    if let Ok(m) = tokio::fs::metadata(path).await {
        let _ = tokio::fs::set_permissions(&tmp, m.permissions()).await;
    }
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

// ---------------------------------------------------------------- drawing

fn ui(f: &mut Frame, app: &mut App) {
    let rows = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(f.area());
    let cols = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(rows[0]);
    let list_h = (app.entries.len() as u16 + 2).clamp(3, 8);
    let left = Layout::vertical([Constraint::Length(list_h), Constraint::Min(3)]).split(cols[0]);

    draw_files(f, app, left[0]);
    draw_code(f, app, left[1]);
    draw_log(f, app, cols[1]);
    draw_footer(f, app, rows[1]);
}

fn draw_files(f: &mut Frame, app: &mut App, area: Rect) {
    let spin = app.spinner();
    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|e| {
            let (tag, color) = match &e.state {
                State::Queued => ("..".to_string(), Color::DarkGray),
                State::Scanning => (spin.to_string(), Color::Cyan),
                State::Failed(_) => ("err".to_string(), Color::Red),
                State::Done(v) if v.is_empty() => ("ok".to_string(), Color::Green),
                State::Done(v) => {
                    let worst = v.iter().map(|x| x.severity).max().unwrap_or_default();
                    (v.len().to_string(), markdown::sev_color(worst))
                }
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{tag:>3} "), Style::new().fg(color)),
                Span::raw(e.rel.clone()),
            ]))
        })
        .collect();

    let block = Block::bordered()
        .title(format!(" Files [{}] ", app.engine.name()))
        .border_style(app.border(Focus::Files));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, &mut app.list);
}

fn marks(findings: &[Finding]) -> HashMap<usize, Severity> {
    let mut m: HashMap<usize, Severity> = HashMap::new();
    for x in findings {
        let end = x.end_line.unwrap_or(x.line).max(x.line).min(x.line + 50);
        for l in x.line..=end {
            let s = m.entry(l).or_insert(x.severity);
            if x.severity > *s {
                *s = x.severity;
            }
        }
    }
    m
}

fn draw_code(f: &mut Frame, app: &mut App, area: Rect) {
    let title = app.cur().map_or(" Source ".to_string(), |e| format!(" {} ", e.rel));
    let block = Block::bordered().title(title).border_style(app.border(Focus::Code));
    let inner = block.inner(area);
    f.render_widget(block, area);

    app.code_h = inner.height as usize;
    let max = app.cur().map_or(0, |e| e.lines.len().saturating_sub(1));
    app.code_scroll = app.code_scroll.min(max);

    let text = match app.cur() {
        None => Text::raw("nothing to show"),
        Some(e) if e.source.is_none() => Text::raw("loading..."),
        Some(e) => {
            let marked = match &e.state {
                State::Done(found) => marks(found),
                _ => HashMap::new(),
            };
            let w = e.lines.len().to_string().len();
            let lines: Vec<Line> = e
                .lines
                .iter()
                .enumerate()
                .skip(app.code_scroll)
                .take(app.code_h)
                .map(|(i, l)| {
                    let n = i + 1;
                    let gutter = match marked.get(&n) {
                        Some(s) => Span::styled(
                            format!("{n:>w$}!"),
                            Style::new().fg(markdown::sev_color(*s)).add_modifier(Modifier::BOLD),
                        ),
                        None => Span::styled(format!("{n:>w$} "), Style::new().fg(Color::DarkGray)),
                    };
                    let mut spans = vec![gutter, Span::raw(" ")];
                    spans.extend(highlight::line(l, &e.ext));
                    Line::from(spans)
                })
                .collect();
            Text::from(lines)
        }
    };
    f.render_widget(Paragraph::new(text), inner);
}

fn draw_log(f: &mut Frame, app: &mut App, area: Rect) {
    let title = if app.status.is_empty() {
        " Audit log ".to_string()
    } else {
        format!(" Audit log - {} ", app.status)
    };
    let block = Block::bordered().title(title).border_style(app.border(Focus::Log));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let max = app.cur().map_or(0, |e| e.log.len().saturating_sub(1));
    app.log_scroll = app.log_scroll.min(max);

    let lines: Vec<Line> = match app.cur() {
        None => vec![Line::raw("no file selected")],
        Some(e) => match &e.state {
            State::Queued | State::Scanning => vec![Line::styled(
                format!("{} scanning {}", app.spinner(), e.rel),
                Style::new().fg(Color::Cyan),
            )],
            State::Failed(msg) => vec![
                Line::styled("scan failed", Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Line::raw(msg.clone()),
            ],
            State::Done(_) => e.log.iter().skip(app.log_scroll).cloned().collect(),
        },
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let ad = app.cfg.ads.enabled.then(|| app.ad.clone().unwrap_or_else(|| "sponsor this space · USDC / ETH / POL".to_string()));
    let ad_w = ad.as_ref().map_or(0, |t| t.chars().count() as u16 + 1).min(area.width / 2);
    let cols = Layout::horizontal([Constraint::Min(1), Constraint::Length(ad_w)]).split(area);

    let mut spans = Vec::new();
    for (k, label) in [("Q", "Quit"), ("P", "Apply Patch"), ("R", "Rescan"), ("Tab", "Focus"), ("N", "Next")] {
        spans.push(Span::styled(format!("[{k}]"), Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw(format!(" {label}  ")));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), cols[0]);

    if let Some(t) = ad {
        f.render_widget(
            Paragraph::new(Span::styled(t, Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM)))
                .alignment(Alignment::Right),
            cols[1],
        );
    }
}
