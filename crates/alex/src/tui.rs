use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Gauge, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use ratatui::{DefaultTerminal, Frame};
use serde_json::Value;
use tokio::sync::Notify;

const GOLD: Color = Color::Indexed(220);
const AMBER: Color = Color::Indexed(178);
const PURPLE: Color = Color::Indexed(99);
const LAPIS: Color = Color::Indexed(69);
const TURQUOISE: Color = Color::Indexed(73);

#[derive(Clone, Default)]
enum DarioView {
    #[default]
    Unknown,
    Disabled,
    Enabled(Value),
}

#[derive(Clone, Default)]
enum TranscriptView {
    #[default]
    Empty,
    Unsupported,
    Ready {
        id: String,
        turns: Vec<Value>,
    },
}

#[derive(Clone, Default)]
struct Snapshot {
    up: bool,
    ever: bool,
    version: String,
    traces: Vec<Value>,
    sessions: Vec<Value>,
    sessions_supported: Option<bool>,
    transcript: TranscriptView,
    limits: Vec<Value>,
    accounts: Vec<Value>,
    dario: DarioView,
    analytics: Value,
    last_ok_at: Option<Instant>,
}

struct Ui {
    tab: usize,
    follow: bool,
    table: TableState,
    stable: TableState,
    raw_mode: bool,
    show_pings: bool,
    transcript: bool,
    tsc_scroll: usize,
    tsc_follow: bool,
    tsc_view_h: usize,
    watching: Arc<Mutex<Option<String>>>,
}

fn raw_active(sessions_supported: Option<bool>, raw_mode: bool) -> bool {
    raw_mode || sessions_supported == Some(false)
}

fn session_kind(s: &Value) -> Option<&'static str> {
    let harness = s.get("harness").and_then(|v| v.as_str()).unwrap_or("");
    if harness.contains("alexandria-ping") {
        return Some("ping");
    }
    match s["tags"]["kind"].as_str() {
        Some("ping") | Some("health") | Some("preflight") | Some("heartbeat") => {
            return Some("ping")
        }
        Some("test") | Some("smoke") => return Some("test"),
        _ => {}
    }
    if matches!(
        s["tags"]["phase"].as_str(),
        Some("preflight") | Some("health") | Some("ping")
    ) {
        return Some("ping");
    }
    let sid = s.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    if sid.starts_with("tsh-")
        || sid.starts_with("alexandria-e2e-")
        || sid.starts_with("smoke-")
        || sid.starts_with("PING-")
    {
        return Some("test");
    }
    None
}

fn visible_sessions<'a>(sessions: &'a [Value], show_pings: bool) -> Vec<&'a Value> {
    sessions
        .iter()
        .filter(|s| show_pings || session_kind(s).is_none())
        .collect()
}

pub async fn run(base_url: &str, local_key: &str) -> Result<()> {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(v) = reqwest::header::HeaderValue::from_str(local_key) {
        headers.insert("x-api-key", v);
    }
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(5))
        .build()?;
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let notify = Arc::new(Notify::new());
    let watching = Arc::new(Mutex::new(None::<String>));
    let tnotify = Arc::new(Notify::new());
    let poller = tokio::spawn(poll_loop(
        client.clone(),
        base_url.to_string(),
        shared.clone(),
        notify.clone(),
    ));
    let tpoller = tokio::spawn(transcript_poll_loop(
        client,
        base_url.to_string(),
        shared.clone(),
        watching.clone(),
        tnotify.clone(),
    ));
    let terminal = ratatui::init();
    let res = ui_loop(terminal, shared, notify, tnotify, watching, base_url).await;
    ratatui::restore();
    poller.abort();
    tpoller.abort();
    res
}

async fn transcript_poll_loop(
    client: reqwest::Client,
    base: String,
    shared: Arc<Mutex<Snapshot>>,
    watching: Arc<Mutex<Option<String>>>,
    notify: Arc<Notify>,
) {
    loop {
        let id = watching.lock().unwrap().clone();
        if let Some(id) = id {
            let url = format!("{base}/traces/sessions/{id}/transcript?limit=500");
            if let Some((code, v)) = get_json(&client, &url).await {
                let still = watching.lock().unwrap().as_deref() == Some(id.as_str());
                if still {
                    let mut s = shared.lock().unwrap();
                    match code {
                        200 => {
                            let turns = v
                                .get("turns")
                                .and_then(|t| t.as_array())
                                .cloned()
                                .unwrap_or_default();
                            s.transcript = TranscriptView::Ready { id, turns };
                        }
                        404 => s.transcript = TranscriptView::Unsupported,
                        _ => {}
                    }
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = notify.notified() => {}
        }
    }
}

async fn poll_loop(
    client: reqwest::Client,
    base: String,
    shared: Arc<Mutex<Snapshot>>,
    notify: Arc<Notify>,
) {
    loop {
        poll_once(&client, &base, &shared).await;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = notify.notified() => {}
        }
    }
}

async fn get_json(client: &reqwest::Client, url: &str) -> Option<(u16, Value)> {
    let resp = client.get(url).send().await.ok()?;
    let status = resp.status().as_u16();
    let val = resp.json::<Value>().await.unwrap_or(Value::Null);
    Some((status, val))
}

async fn poll_once(client: &reqwest::Client, base: &str, shared: &Arc<Mutex<Snapshot>>) {
    let health = get_json(client, &format!("{base}/health")).await;
    let ok = matches!(&health, Some((code, _)) if *code == 200);
    if !ok {
        let mut s = shared.lock().unwrap();
        s.up = false;
        return;
    }
    let version = health
        .as_ref()
        .and_then(|(_, v)| v.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let u_traces = format!("{base}/admin/traces?limit=100");
    let u_sessions = format!("{base}/traces/sessions?since=24h&limit=200");
    let u_limits = format!("{base}/admin/limits");
    let u_accounts = format!("{base}/admin/health");
    let u_dario = format!("{base}/admin/dario");
    let u_analytics = format!("{base}/admin/analytics?since_minutes=60");
    let (traces, sessions, limits, accounts, dario, analytics) = tokio::join!(
        get_json(client, &u_traces),
        get_json(client, &u_sessions),
        get_json(client, &u_limits),
        get_json(client, &u_accounts),
        get_json(client, &u_dario),
        get_json(client, &u_analytics),
    );
    let mut s = shared.lock().unwrap();
    s.up = true;
    s.ever = true;
    s.version = version;
    s.last_ok_at = Some(Instant::now());
    if let Some((200, v)) = traces {
        let mut list = v
            .get("traces")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        list.sort_by_key(|t| -t.get("ts_request_ms").and_then(|v| v.as_i64()).unwrap_or(0));
        s.traces = list;
    }
    match sessions {
        Some((200, v)) => {
            let mut list = v
                .get("sessions")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            list.sort_by_key(|x| -x.get("last_ts_ms").and_then(|v| v.as_i64()).unwrap_or(0));
            s.sessions = list;
            s.sessions_supported = Some(true);
        }
        Some((404, _)) => s.sessions_supported = Some(false),
        _ => {}
    }
    if let Some((200, v)) = limits {
        s.limits = v
            .get("providers")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();
    }
    if let Some((200, v)) = accounts {
        s.accounts = v
            .get("accounts")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
    }
    match dario {
        Some((200, v)) => s.dario = DarioView::Enabled(v),
        Some((404, _)) => s.dario = DarioView::Disabled,
        _ => {}
    }
    if let Some((200, v)) = analytics {
        s.analytics = v;
    }
}

async fn ui_loop(
    mut terminal: DefaultTerminal,
    shared: Arc<Mutex<Snapshot>>,
    notify: Arc<Notify>,
    tnotify: Arc<Notify>,
    watching: Arc<Mutex<Option<String>>>,
    base: &str,
) -> Result<()> {
    let mut ui = Ui {
        tab: 0,
        follow: true,
        table: TableState::default(),
        stable: TableState::default(),
        raw_mode: false,
        show_pings: false,
        transcript: false,
        tsc_scroll: 0,
        tsc_follow: true,
        tsc_view_h: 10,
        watching,
    };
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            ev = events.next() => {
                if let Some(Ok(Event::Key(k))) = ev {
                    if k.kind == KeyEventKind::Press {
                        if k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            return Ok(());
                        }
                        if ui.transcript {
                            let page = ui.tsc_view_h.saturating_sub(1).max(1);
                            match k.code {
                                KeyCode::Esc | KeyCode::Char('q') => {
                                    ui.transcript = false;
                                    *ui.watching.lock().unwrap() = None;
                                    shared.lock().unwrap().transcript = TranscriptView::Empty;
                                }
                                KeyCode::Up => {
                                    ui.tsc_follow = false;
                                    ui.tsc_scroll = ui.tsc_scroll.saturating_sub(1);
                                }
                                KeyCode::Down => {
                                    ui.tsc_follow = false;
                                    ui.tsc_scroll = ui.tsc_scroll.saturating_add(1);
                                }
                                KeyCode::PageUp => {
                                    ui.tsc_follow = false;
                                    ui.tsc_scroll = ui.tsc_scroll.saturating_sub(page);
                                }
                                KeyCode::PageDown => {
                                    ui.tsc_follow = false;
                                    ui.tsc_scroll = ui.tsc_scroll.saturating_add(page);
                                }
                                KeyCode::End => ui.tsc_scroll = usize::MAX,
                                KeyCode::Char('f') => ui.tsc_follow = true,
                                _ => {}
                            }
                        } else {
                            match k.code {
                                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                                KeyCode::Char('1') => ui.tab = 0,
                                KeyCode::Char('2') => ui.tab = 1,
                                KeyCode::Char('3') => ui.tab = 2,
                                KeyCode::Char('4') => ui.tab = 3,
                                KeyCode::Left => ui.tab = (ui.tab + 3) % 4,
                                KeyCode::Right => ui.tab = (ui.tab + 1) % 4,
                                KeyCode::Up | KeyCode::Down => {
                                    ui.follow = false;
                                    let supported = shared.lock().unwrap().sessions_supported;
                                    let sess =
                                        ui.tab == 0 && !raw_active(supported, ui.raw_mode);
                                    let st = if sess { &mut ui.stable } else { &mut ui.table };
                                    if k.code == KeyCode::Up {
                                        let i = st.selected().unwrap_or(0);
                                        st.select(Some(i.saturating_sub(1)));
                                    } else {
                                        let i = st.selected().map(|i| i + 1).unwrap_or(0);
                                        st.select(Some(i));
                                    }
                                }
                                KeyCode::Char('f') => ui.follow = true,
                                KeyCode::Char('r') => {
                                    if ui.tab == 0 {
                                        ui.raw_mode = !ui.raw_mode;
                                    } else {
                                        notify.notify_one();
                                    }
                                }
                                KeyCode::Char('p') if ui.tab == 0 => {
                                    ui.show_pings = !ui.show_pings;
                                }
                                KeyCode::Enter if ui.tab == 0 => {
                                    let sid = {
                                        let s = shared.lock().unwrap();
                                        if raw_active(s.sessions_supported, ui.raw_mode) {
                                            None
                                        } else {
                                            let vis = visible_sessions(
                                                &s.sessions,
                                                ui.show_pings,
                                            );
                                            ui.stable
                                                .selected()
                                                .and_then(|i| vis.get(i).copied())
                                                .and_then(|v| v.get("session_id"))
                                                .and_then(|v| v.as_str())
                                                .map(|v| v.to_string())
                                        }
                                    };
                                    if let Some(sid) = sid {
                                        *ui.watching.lock().unwrap() = Some(sid);
                                        ui.transcript = true;
                                        ui.tsc_follow = true;
                                        ui.tsc_scroll = 0;
                                        tnotify.notify_one();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        let snap = shared.lock().unwrap().clone();
        if !ui.transcript {
            let sess = ui.tab == 0 && !raw_active(snap.sessions_supported, ui.raw_mode);
            let (st, len) = if sess {
                (
                    &mut ui.stable,
                    visible_sessions(&snap.sessions, ui.show_pings).len(),
                )
            } else {
                (&mut ui.table, snap.traces.len())
            };
            if ui.follow {
                st.select(if len == 0 { None } else { Some(0) });
            } else {
                match st.selected() {
                    Some(_) if len == 0 => st.select(None),
                    Some(i) if i >= len => st.select(Some(len - 1)),
                    None if len > 0 => st.select(Some(0)),
                    _ => {}
                }
            }
        }
        terminal.draw(|f| draw(f, &snap, &mut ui, base))?;
    }
}

fn draw(f: &mut Frame, snap: &Snapshot, ui: &mut Ui, base: &str) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(f.area());
    draw_status(f, chunks[0], snap, base);
    draw_tabs(f, chunks[1], ui.tab);
    if ui.transcript {
        draw_transcript(f, chunks[2], snap, ui);
    } else {
        match ui.tab {
            0 => {
                if raw_active(snap.sessions_supported, ui.raw_mode) {
                    draw_traces(f, chunks[2], snap, ui);
                } else {
                    draw_sessions(f, chunks[2], snap, ui);
                }
            }
            1 => draw_limits(f, chunks[2], snap),
            2 => draw_accounts(f, chunks[2], snap),
            _ => draw_dario(f, chunks[2], snap),
        }
    }
    draw_bottom(f, chunks[3], snap, ui);
}

fn themed_block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(AMBER))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
        ))
}

fn draw_status(f: &mut Frame, area: Rect, snap: &Snapshot, base: &str) {
    let version = if snap.version.is_empty() {
        "?".to_string()
    } else {
        snap.version.clone()
    };
    let mut spans = vec![
        Span::styled(
            " ◆ ALEX ",
            Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("v{version} "), Style::default().fg(PURPLE)),
        Span::styled(format!("@ {base} "), Style::default().fg(LAPIS)),
        if snap.up {
            Span::styled(
                " UP ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(
                " DOWN ",
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            )
        },
        Span::styled(
            format!("  {}", chrono::Local::now().format("%H:%M:%S")),
            Style::default().fg(PURPLE),
        ),
    ];
    if let Some(t) = snap.last_ok_at {
        spans.push(Span::styled(
            format!(
                "  refreshed {} ago",
                humanize_s(t.elapsed().as_secs() as i64)
            ),
            Style::default().fg(PURPLE).add_modifier(Modifier::DIM),
        ));
    }
    if !snap.up && snap.ever {
        spans.push(Span::styled(
            "  ⚠ stale data",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_tabs(f: &mut Frame, area: Rect, tab: usize) {
    let titles = ["Sessions", "Limits", "Accounts", "Dario"]
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == tab {
                Line::from(Span::styled(
                    format!("◆ {t}"),
                    Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(
                    format!("{} {t}", i + 1),
                    Style::default().fg(PURPLE),
                ))
            }
        });
    let tabs = Tabs::new(titles)
        .select(tab)
        .divider(Span::styled("·", Style::default().fg(AMBER)));
    f.render_widget(tabs, area);
}

struct TraceCells {
    time: String,
    model: String,
    provider: String,
    fmt: String,
    cross: bool,
    status: String,
    status_class: u8,
    tokens_in: String,
    tokens_out: String,
    cost: String,
    session: String,
    error: String,
}

fn jstr(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("-").to_string()
}

fn jint(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| x.as_i64())
}

fn jf64(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

fn fmt_time_ms(ts: Option<i64>) -> String {
    ts.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "--:--:--".into())
}

fn fmt_count(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn trace_cells(t: &Value) -> TraceCells {
    let time = fmt_time_ms(jint(t, "ts_request_ms").filter(|ts| *ts > 0));
    let model = t
        .get("routed_model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| t.get("requested_model").and_then(|v| v.as_str()))
        .unwrap_or("-")
        .to_string();
    let cf = t
        .get("client_format")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let uf = t
        .get("upstream_format")
        .and_then(|v| v.as_str())
        .unwrap_or("-");
    let cross = cf != uf && cf != "-" && uf != "-";
    let code = jint(t, "status").unwrap_or(0);
    TraceCells {
        time,
        model,
        provider: jstr(t, "upstream_provider"),
        fmt: format!("{cf}→{uf}"),
        cross,
        status: if code > 0 {
            code.to_string()
        } else {
            "-".into()
        },
        status_class: (code / 100).clamp(0, 9) as u8,
        tokens_in: jint(t, "input_tokens")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".into()),
        tokens_out: jint(t, "output_tokens")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".into()),
        cost: jf64(t, "cost_usd")
            .map(|c| format!("${c:.4}"))
            .unwrap_or_else(|| "-".into()),
        session: truncate(&jstr(t, "session_id"), 12),
        error: t
            .get("error")
            .and_then(|v| v.as_str())
            .map(|e| truncate(e, 48))
            .unwrap_or_default(),
    }
}

fn status_color(class: u8) -> Color {
    match class {
        2 => Color::Green,
        4 => Color::Yellow,
        5 => Color::Red,
        _ => Color::DarkGray,
    }
}

fn draw_traces(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &mut Ui) {
    let area = if snap.sessions_supported == Some(false) && !ui.raw_mode {
        let c = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
        f.render_widget(
            Paragraph::new(Span::styled(
                " sessions endpoint unavailable — daemon update required · showing raw traces",
                Style::default().fg(PURPLE).add_modifier(Modifier::DIM),
            )),
            c[0],
        );
        c[1]
    } else {
        area
    };
    let sel = ui.table.selected().filter(|i| *i < snap.traces.len());
    let (tarea, darea) = if sel.is_some() && area.height > 16 {
        let c = Layout::vertical([Constraint::Min(6), Constraint::Length(11)]).split(area);
        (c[0], Some(c[1]))
    } else {
        (area, None)
    };
    let header = Row::new(
        [
            "time", "model", "provider", "fmt", "st", "in", "out", "cost", "session", "error",
        ]
        .iter()
        .map(|h| Cell::from(*h)),
    )
    .style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD));
    let rows = snap.traces.iter().map(|t| {
        let c = trace_cells(t);
        let fmt_style = if c.cross {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(PURPLE).add_modifier(Modifier::DIM)
        };
        Row::new(vec![
            Cell::from(c.time).style(Style::default().fg(PURPLE)),
            Cell::from(c.model).style(Style::default().fg(TURQUOISE)),
            Cell::from(c.provider).style(Style::default().fg(LAPIS)),
            Cell::from(c.fmt).style(fmt_style),
            Cell::from(c.status).style(Style::default().fg(status_color(c.status_class))),
            Cell::from(c.tokens_in),
            Cell::from(c.tokens_out),
            Cell::from(c.cost).style(Style::default().fg(PURPLE)),
            Cell::from(c.session).style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM)),
            Cell::from(c.error).style(Style::default().fg(Color::Red)),
        ])
    });
    let title = if ui.follow {
        "Raw traces · following".to_string()
    } else {
        "Raw traces · paused (f to follow)".to_string()
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Min(18),
            Constraint::Length(10),
            Constraint::Length(16),
            Constraint::Length(4),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(9),
            Constraint::Length(13),
            Constraint::Min(10),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(Color::Indexed(236)).fg(GOLD))
    .block(themed_block(&title));
    f.render_stateful_widget(table, tarea, &mut ui.table);
    if let (Some(i), Some(darea)) = (sel, darea) {
        if let Some(t) = snap.traces.get(i) {
            let p = Paragraph::new(detail_lines(t))
                .wrap(Wrap { trim: false })
                .block(themed_block("Detail"));
            f.render_widget(p, darea);
        }
    }
}

fn detail_lines(t: &Value) -> Vec<Line<'static>> {
    let key = |k: &str| Span::styled(format!("{k}: "), Style::default().fg(GOLD));
    let val = |v: String| Span::styled(v, Style::default().fg(PURPLE));
    let latency = match (jint(t, "ts_request_ms"), jint(t, "ts_response_ms")) {
        (Some(a), Some(b)) if b >= a => format!("{}ms", b - a),
        _ => "-".into(),
    };
    let mut lines = vec![
        Line::from(vec![
            key("harness"),
            val(jstr(t, "harness")),
            Span::raw("  "),
            key("format"),
            val(format!(
                "{} → {}",
                jstr(t, "client_format"),
                jstr(t, "upstream_format")
            )),
            Span::raw("  "),
            key("provider"),
            val(jstr(t, "upstream_provider")),
        ]),
        Line::from(vec![
            key("requested"),
            val(jstr(t, "requested_model")),
            Span::raw("  "),
            key("routed"),
            Span::styled(jstr(t, "routed_model"), Style::default().fg(TURQUOISE)),
        ]),
        Line::from(vec![
            key("status"),
            val(jint(t, "status")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".into())),
            Span::raw("  "),
            key("streamed"),
            val(t
                .get("streamed")
                .and_then(|v| v.as_bool())
                .map(|b| b.to_string())
                .unwrap_or_else(|| "-".into())),
            Span::raw("  "),
            key("latency"),
            val(latency),
        ]),
        Line::from(vec![
            key("tokens in/out/cached"),
            val(format!(
                "{}/{}/{}",
                jint(t, "input_tokens")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                jint(t, "output_tokens")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                jint(t, "cached_input_tokens")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
            )),
            Span::raw("  "),
            key("cost"),
            val(jf64(t, "cost_usd")
                .map(|c| format!("${c:.6}"))
                .unwrap_or_else(|| "-".into())),
            Span::raw("  "),
            key("bucket"),
            val(jstr(t, "billing_bucket")),
        ]),
        Line::from(vec![key("session"), val(jstr(t, "session_id"))]),
        Line::from(vec![key("account"), val(jstr(t, "account_id"))]),
    ];
    if let Some(err) = t.get("error").and_then(|v| v.as_str()) {
        if !err.is_empty() {
            lines.push(Line::from(vec![
                key("error"),
                Span::styled(err.to_string(), Style::default().fg(Color::Red)),
            ]));
        }
    }
    lines
}

struct SessionCells {
    time: String,
    id: String,
    run: String,
    models: String,
    turns: String,
    tokens: String,
    cost: String,
    errors: String,
}

fn models_short(v: Option<&Value>) -> String {
    let joined = match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    if joined.is_empty() {
        "-".into()
    } else {
        truncate(&joined, 24)
    }
}

fn session_cells(s: &Value) -> SessionCells {
    let tin = jint(s, "total_input_tokens");
    let tout = jint(s, "total_output_tokens");
    let tokens = if tin.is_none() && tout.is_none() {
        "-".into()
    } else {
        format!(
            "{}→{}",
            tin.map(fmt_count).unwrap_or_else(|| "-".into()),
            tout.map(fmt_count).unwrap_or_else(|| "-".into()),
        )
    };
    SessionCells {
        time: fmt_time_ms(jint(s, "last_ts_ms")),
        id: truncate(&jstr(s, "session_id"), 24),
        run: s
            .get("run_id")
            .and_then(|v| v.as_str())
            .filter(|r| !r.is_empty())
            .map(|r| truncate(r, 12))
            .unwrap_or_default(),
        models: models_short(s.get("models")),
        turns: jint(s, "trace_count")
            .map(|n| format!("{n} turns"))
            .unwrap_or_else(|| "-".into()),
        tokens,
        cost: jf64(s, "total_cost_usd")
            .map(|c| format!("${c:.4}"))
            .unwrap_or_else(|| "-".into()),
        errors: match jint(s, "errors") {
            Some(e) if e > 0 => format!("{e} err"),
            _ => String::new(),
        },
    }
}

fn draw_sessions(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &mut Ui) {
    let vis = visible_sessions(&snap.sessions, ui.show_pings);
    let hidden = snap.sessions.len() - vis.len();
    let title = match (ui.follow, ui.show_pings, hidden) {
        (true, false, 0) => "Sessions · following".to_string(),
        (true, false, n) => format!("Sessions · following · {n} pings hidden (p)"),
        (true, true, _) => "Sessions · following · pings shown (p)".to_string(),
        (false, false, 0) => "Sessions · paused (f to follow)".to_string(),
        (false, false, n) => format!("Sessions · paused · {n} pings hidden (p)"),
        (false, true, _) => "Sessions · paused · pings shown (p)".to_string(),
    };
    let title = title.as_str();
    if vis.is_empty() {
        let block = themed_block(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let msg = if snap.sessions_supported.is_none() {
            "no data yet"
        } else if hidden > 0 {
            "only pings/health checks in the last 24h — press p to show them"
        } else {
            "no sessions in the last 24h"
        };
        let p = Paragraph::new(msg)
            .style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM))
            .alignment(Alignment::Center);
        f.render_widget(p, inner);
        return;
    }
    let header = Row::new(
        [
            "time",
            "session",
            "run",
            "models",
            "turns",
            "tokens in→out",
            "cost",
            "err",
        ]
        .iter()
        .map(|h| Cell::from(*h)),
    )
    .style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD));
    let rows = vis.iter().map(|s| {
        let c = session_cells(s);
        let kind = session_kind(s);
        let dim = kind.is_some();
        let id_cell = match kind {
            Some(k) => Cell::from(format!("{} [{k}]", c.id))
                .style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM)),
            None => Cell::from(c.id).style(Style::default().fg(TURQUOISE)),
        };
        let time_style = if dim {
            Style::default().fg(PURPLE).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(PURPLE)
        };
        Row::new(vec![
            Cell::from(c.time).style(time_style),
            id_cell,
            Cell::from(c.run).style(Style::default().fg(AMBER)),
            Cell::from(c.models).style(Style::default().fg(LAPIS)),
            Cell::from(c.turns),
            Cell::from(c.tokens),
            Cell::from(c.cost).style(Style::default().fg(PURPLE)),
            Cell::from(c.errors)
                .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(26),
            Constraint::Length(12),
            Constraint::Min(14),
            Constraint::Length(9),
            Constraint::Length(15),
            Constraint::Length(10),
            Constraint::Min(6),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(Color::Indexed(236)).fg(GOLD))
    .block(themed_block(title));
    f.render_stateful_widget(table, area, &mut ui.stable);
}

const TURN_TEXT_CAP: usize = 40;

fn turn_header_text(t: &Value) -> String {
    let tin = jint(t, "input_tokens")
        .map(fmt_count)
        .unwrap_or_else(|| "-".into());
    let tout = jint(t, "output_tokens")
        .map(fmt_count)
        .unwrap_or_else(|| "-".into());
    let cost = jf64(t, "cost_usd")
        .map(|c| format!("${c:.4}"))
        .unwrap_or_else(|| "-".into());
    let status = jint(t, "status")
        .filter(|s| *s > 0)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "-".into());
    format!(
        "── {} · {} · {tin}→{tout} tok · {cost} · {status} ──",
        fmt_time_ms(jint(t, "ts_request_ms")),
        jstr(t, "model"),
    )
}

fn wrap_text(s: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out = Vec::new();
    for raw in s.split('\n') {
        let mut cur = String::new();
        let mut len = 0usize;
        for word in raw.split_whitespace() {
            let wlen = word.chars().count();
            if len > 0 && len + 1 + wlen <= width {
                cur.push(' ');
                cur.push_str(word);
                len += 1 + wlen;
                continue;
            }
            if len > 0 {
                out.push(std::mem::take(&mut cur));
                len = 0;
            }
            if wlen <= width {
                cur.push_str(word);
                len = wlen;
            } else {
                let chars: Vec<char> = word.chars().collect();
                for chunk in chars.chunks(width) {
                    if len > 0 {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur = chunk.iter().collect();
                    len = chunk.len();
                }
            }
        }
        out.push(cur);
    }
    out
}

fn cap_lines(lines: Vec<String>, max: usize) -> (Vec<String>, usize) {
    if lines.len() <= max {
        (lines, 0)
    } else {
        let hidden = lines.len() - max;
        (lines.into_iter().take(max).collect(), hidden)
    }
}

fn transcript_lines(turns: &[Value], width: usize) -> Vec<Line<'static>> {
    let w = width.max(8);
    let dim = Style::default().fg(PURPLE).add_modifier(Modifier::DIM);
    let mut out = Vec::new();
    for t in turns {
        out.push(Line::from(Span::styled(turn_header_text(t), dim)));
        if let Some(u) = t
            .get("user")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            let (lines, hidden) = cap_lines(wrap_text(u, w.saturating_sub(2)), TURN_TEXT_CAP);
            for (i, l) in lines.into_iter().enumerate() {
                let prefix = if i == 0 { "❯ " } else { "  " };
                out.push(Line::from(Span::styled(
                    format!("{prefix}{l}"),
                    Style::default().fg(PURPLE),
                )));
            }
            if hidden > 0 {
                out.push(Line::from(Span::styled(
                    format!("  … (+{hidden} lines)"),
                    dim,
                )));
            }
        }
        if let Some(a) = t
            .get("assistant")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            let (lines, hidden) = cap_lines(wrap_text(a, w), TURN_TEXT_CAP);
            for l in lines {
                out.push(Line::from(Span::raw(l)));
            }
            if hidden > 0 {
                out.push(Line::from(Span::styled(
                    format!("… (+{hidden} lines)"),
                    dim,
                )));
            }
        }
        if let Some(e) = t
            .get("error")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            for l in wrap_text(&format!("✗ {e}"), w) {
                out.push(Line::from(Span::styled(l, Style::default().fg(Color::Red))));
            }
        }
        out.push(Line::default());
    }
    out
}

fn centered_msg(f: &mut Frame, area: Rect, block: Block, msg: &str) {
    let inner = block.inner(area);
    f.render_widget(block, area);
    let p = Paragraph::new(msg.to_string())
        .style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    f.render_widget(p, inner);
}

fn draw_transcript(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &mut Ui) {
    match &snap.transcript {
        TranscriptView::Ready { id, turns } => {
            let cost: f64 = turns.iter().filter_map(|t| jf64(t, "cost_usd")).sum();
            let follow = if ui.tsc_follow {
                "following"
            } else {
                "paused (f to follow)"
            };
            let title = format!(
                "◆ session {} — {} turns · ${cost:.4} · {follow}",
                truncate(id, 24),
                turns.len(),
            );
            let block = themed_block(&title);
            let inner = block.inner(area);
            f.render_widget(block, area);
            if turns.is_empty() {
                let p = Paragraph::new("no turns yet")
                    .style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM))
                    .alignment(Alignment::Center);
                f.render_widget(p, inner);
                return;
            }
            let lines = transcript_lines(turns, inner.width.saturating_sub(1) as usize);
            let view_h = inner.height as usize;
            ui.tsc_view_h = view_h;
            let max_scroll = lines.len().saturating_sub(view_h);
            if ui.tsc_follow || ui.tsc_scroll > max_scroll {
                ui.tsc_scroll = max_scroll;
            }
            let visible: Vec<Line> = lines.into_iter().skip(ui.tsc_scroll).take(view_h).collect();
            f.render_widget(Paragraph::new(visible), inner);
        }
        TranscriptView::Unsupported => {
            centered_msg(
                f,
                area,
                themed_block("◆ transcript"),
                "daemon update required — transcript endpoint unavailable",
            );
        }
        TranscriptView::Empty => {
            let id = ui.watching.lock().unwrap().clone().unwrap_or_default();
            let title = format!("◆ session {}", truncate(&id, 24));
            centered_msg(f, area, themed_block(&title), "loading transcript…");
        }
    }
}

fn gauge_color(pct: f64) -> Color {
    if pct >= 80.0 {
        Color::Red
    } else if pct >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn humanize_s(mut s: i64) -> String {
    if s < 0 {
        return "expired".into();
    }
    let d = s / 86400;
    s %= 86400;
    let h = s / 3600;
    s %= 3600;
    let m = s / 60;
    let sec = s % 60;
    if d > 0 {
        format!("{d}d{h}h")
    } else if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{sec:02}s")
    } else {
        format!("{sec}s")
    }
}

fn reset_secs(w: &Value, now_s: i64) -> Option<i64> {
    if let Some(s) = w.get("resets_at").and_then(|v| v.as_str()) {
        if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
            return Some(d.timestamp() - now_s);
        }
    }
    w.get("resets_at_s")
        .and_then(|v| v.as_i64())
        .map(|e| e - now_s)
}

enum LimitItem {
    Text(Line<'static>),
    Meter { label: String, pct: f64 },
}

fn limit_items(providers: &[Value]) -> Vec<LimitItem> {
    let now_s = chrono::Utc::now().timestamp();
    let mut items = vec![];
    for p in providers {
        let provider = jstr(p, "provider");
        let mut head = vec![Span::styled(
            provider.clone(),
            Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
        )];
        if let Some(plan) = p.get("plan").and_then(|v| v.as_str()) {
            head.push(Span::styled(
                format!(" · plan: {plan}"),
                Style::default().fg(PURPLE),
            ));
        }
        if let Some(src) = p.get("source").and_then(|v| v.as_str()) {
            head.push(Span::styled(
                format!(" · source: {src}"),
                Style::default().fg(PURPLE).add_modifier(Modifier::DIM),
            ));
        }
        items.push(LimitItem::Text(Line::from(head)));
        if let Some(err) = p.get("error").and_then(|v| v.as_str()) {
            items.push(LimitItem::Text(Line::from(Span::styled(
                format!("  ✗ {err}"),
                Style::default().fg(Color::Red),
            ))));
        }
        for w in p
            .get("windows")
            .and_then(|v| v.as_array())
            .map(|v| v.as_slice())
            .unwrap_or_default()
        {
            let window = jstr(w, "window");
            let pct = jf64(w, "used_pct").unwrap_or(0.0);
            let reset = reset_secs(w, now_s)
                .map(|s| format!("resets in {}", humanize_s(s)))
                .unwrap_or_else(|| "no reset info".into());
            items.push(LimitItem::Meter {
                label: format!("{provider} {window} — {reset} · {pct:.0}% used"),
                pct,
            });
        }
        for (k, name) in [("requests", "requests"), ("tokens", "tokens")] {
            if let Some(q) = p.get(k).filter(|q| q.is_object()) {
                let limit = jint(q, "limit").unwrap_or(0);
                let remaining = jint(q, "remaining").unwrap_or(0);
                if limit > 0 {
                    let pct = ((limit - remaining).max(0) as f64 / limit as f64) * 100.0;
                    items.push(LimitItem::Meter {
                        label: format!(
                            "{provider} {name} — {remaining}/{limit} remaining · {pct:.0}% used"
                        ),
                        pct,
                    });
                }
            }
        }
        items.push(LimitItem::Text(Line::default()));
    }
    items
}

fn draw_limits(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = themed_block("Limits");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if snap.limits.is_empty() {
        let p = Paragraph::new("no limit data")
            .style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM))
            .alignment(Alignment::Center);
        f.render_widget(p, inner);
        return;
    }
    let items = limit_items(&snap.limits);
    let mut y = inner.y;
    for item in items {
        if y >= inner.y + inner.height {
            break;
        }
        let row = Rect::new(inner.x, y, inner.width, 1);
        match item {
            LimitItem::Text(line) => f.render_widget(Paragraph::new(line), row),
            LimitItem::Meter { label, pct } => {
                let g = Gauge::default()
                    .ratio((pct / 100.0).clamp(0.0, 1.0))
                    .label(Span::styled(label, Style::default().fg(Color::White)))
                    .gauge_style(
                        Style::default()
                            .fg(gauge_color(pct))
                            .bg(Color::Indexed(236)),
                    );
                f.render_widget(g, row);
            }
        }
        y += 1;
    }
}

fn draw_accounts(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let header = Row::new(
        [
            "provider",
            "id",
            "kind",
            "status",
            "token expires",
            "heartbeat",
        ]
        .iter()
        .map(|h| Cell::from(*h)),
    )
    .style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD));
    let now_ms = chrono::Utc::now().timestamp_millis();
    let rows = snap.accounts.iter().map(|a| {
        let expiry = match a.get("token_expires_in_s").and_then(|v| v.as_i64()) {
            Some(s) if s <= 0 => Cell::from("expired")
                .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Some(s) => Cell::from(humanize_s(s)).style(Style::default().fg(PURPLE)),
            None => Cell::from("-").style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM)),
        };
        let hb = match a.get("last_heartbeat").filter(|h| h.is_object()) {
            Some(h) => {
                let ok = h.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                let age = jint(h, "ts_ms")
                    .map(|t| format!("{} ago", humanize_s(((now_ms - t) / 1000).max(0))))
                    .unwrap_or_else(|| "-".into());
                let latency = jint(h, "latency_ms")
                    .map(|l| format!("{l}ms"))
                    .unwrap_or_default();
                let msg = h
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|m| truncate(m, 30))
                    .unwrap_or_default();
                if ok {
                    Cell::from(format!("✓ {latency} · {age}"))
                        .style(Style::default().fg(Color::Green))
                } else {
                    Cell::from(format!("✗ {msg} · {age}")).style(Style::default().fg(Color::Red))
                }
            }
            None => Cell::from("-").style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM)),
        };
        let status = jstr(a, "status");
        let status_style = match status.as_str() {
            "ok" | "active" | "ready" => Style::default().fg(Color::Green),
            "expired" | "error" | "invalid" => Style::default().fg(Color::Red),
            _ => Style::default().fg(Color::Yellow),
        };
        Row::new(vec![
            Cell::from(jstr(a, "provider")).style(Style::default().fg(LAPIS)),
            Cell::from(jstr(a, "id")).style(Style::default().fg(PURPLE)),
            Cell::from(jstr(a, "kind"))
                .style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM)),
            Cell::from(status).style(status_style),
            expiry,
            hb,
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(themed_block("Accounts"));
    f.render_widget(table, area);
}

fn phase_style(phase: &str) -> Style {
    match phase {
        "ready" => Style::default().fg(Color::Green),
        "starting" | "draining" => Style::default().fg(Color::Yellow),
        "unhealthy" => Style::default().fg(Color::Red),
        "dead" => Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
        _ => Style::default().fg(PURPLE),
    }
}

fn draw_dario(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let block = themed_block("Dario");
    let inner = block.inner(area);
    f.render_widget(block, area);
    match &snap.dario {
        DarioView::Unknown => {
            let p = Paragraph::new("no data yet")
                .style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM))
                .alignment(Alignment::Center);
            f.render_widget(p, inner);
        }
        DarioView::Disabled => {
            let msg = "dario mode disabled (anthropic_upstream = \"direct\")";
            let v = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .split(inner);
            let p = Paragraph::new(msg)
                .style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM))
                .alignment(Alignment::Center);
            f.render_widget(p, v[1]);
        }
        DarioView::Enabled(v) => {
            let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);
            let active = v
                .get("active_generation_id")
                .and_then(|x| x.as_str())
                .unwrap_or("-");
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("active generation: ", Style::default().fg(GOLD)),
                    Span::styled(
                        active.to_string(),
                        Style::default().fg(TURQUOISE).add_modifier(Modifier::BOLD),
                    ),
                ])),
                chunks[0],
            );
            let header = Row::new(
                [
                    "id",
                    "version",
                    "phase",
                    "pid",
                    "port",
                    "in-flight",
                    "fails",
                    "probe",
                ]
                .iter()
                .map(|h| Cell::from(*h)),
            )
            .style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD));
            let rows = v
                .get("generations")
                .and_then(|g| g.as_array())
                .map(|g| g.as_slice())
                .unwrap_or_default()
                .iter()
                .map(|g| {
                    let phase = jstr(g, "phase");
                    let probe = match g.get("last_probe").filter(|p| p.is_object()) {
                        Some(p) => {
                            let ok = p.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                            if ok {
                                Cell::from(format!(
                                    "✓ {}",
                                    jint(p, "latency_ms")
                                        .map(|l| format!("{l}ms"))
                                        .unwrap_or_else(|| "-".into())
                                ))
                                .style(Style::default().fg(Color::Green))
                            } else {
                                Cell::from(format!(
                                    "✗ {}",
                                    p.get("error")
                                        .and_then(|v| v.as_str())
                                        .map(|e| truncate(e, 30))
                                        .unwrap_or_else(|| "-".into())
                                ))
                                .style(Style::default().fg(Color::Red))
                            }
                        }
                        None => Cell::from("-")
                            .style(Style::default().fg(PURPLE).add_modifier(Modifier::DIM)),
                    };
                    Row::new(vec![
                        Cell::from(jstr(g, "id")).style(Style::default().fg(PURPLE)),
                        Cell::from(jstr(g, "version")).style(Style::default().fg(TURQUOISE)),
                        Cell::from(phase.clone()).style(phase_style(&phase)),
                        Cell::from(
                            jint(g, "pid")
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "-".into()),
                        ),
                        Cell::from(
                            jint(g, "port")
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "-".into()),
                        ),
                        Cell::from(
                            jint(g, "in_flight")
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "-".into()),
                        ),
                        Cell::from(
                            jint(g, "consecutive_failures")
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "-".into()),
                        ),
                        probe,
                    ])
                });
            let table = Table::new(
                rows,
                [
                    Constraint::Min(12),
                    Constraint::Length(10),
                    Constraint::Length(10),
                    Constraint::Length(8),
                    Constraint::Length(6),
                    Constraint::Length(9),
                    Constraint::Length(6),
                    Constraint::Min(14),
                ],
            )
            .header(header);
            f.render_widget(table, chunks[1]);
        }
    }
}

fn key_hints(snap: &Snapshot, ui: &Ui) -> &'static str {
    if ui.transcript {
        " Esc back · ↑↓ PgUp/PgDn scroll · f follow · End bottom "
    } else if ui.tab == 0 {
        if raw_active(snap.sessions_supported, ui.raw_mode) {
            " q quit · 1-4/←→ tabs · ↑↓ select · f follow · r sessions "
        } else {
            " q quit · 1-4/←→ tabs · ↑↓ select · Enter transcript · f follow · p pings · r raw "
        }
    } else {
        " q quit · 1-4/←→ tabs · ↑↓ scroll · f follow · r refresh "
    }
}

fn draw_bottom(f: &mut Frame, area: Rect, snap: &Snapshot, ui: &Ui) {
    let totals = snap.analytics.get("totals").cloned().unwrap_or(Value::Null);
    let req = jint(&totals, "requests").unwrap_or(0);
    let cost = jf64(&totals, "cost_usd").unwrap_or(0.0);
    let errors = jint(&totals, "errors").unwrap_or(0);
    let line = Line::from(vec![
        Span::styled(
            key_hints(snap, ui),
            Style::default().fg(PURPLE).add_modifier(Modifier::DIM),
        ),
        Span::styled("│ ", Style::default().fg(AMBER)),
        Span::styled(
            format!("60m: {req} req · ${cost:.4} · "),
            Style::default().fg(PURPLE),
        ),
        Span::styled(
            format!("{errors} err"),
            if errors > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(PURPLE)
            },
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gauge_color_thresholds() {
        assert_eq!(gauge_color(0.0), Color::Green);
        assert_eq!(gauge_color(49.9), Color::Green);
        assert_eq!(gauge_color(50.0), Color::Yellow);
        assert_eq!(gauge_color(79.9), Color::Yellow);
        assert_eq!(gauge_color(80.0), Color::Red);
        assert_eq!(gauge_color(100.0), Color::Red);
    }

    #[test]
    fn humanize_durations() {
        assert_eq!(humanize_s(-5), "expired");
        assert_eq!(humanize_s(0), "0s");
        assert_eq!(humanize_s(45), "45s");
        assert_eq!(humanize_s(125), "2m05s");
        assert_eq!(humanize_s(8160), "2h16m");
        assert_eq!(humanize_s(90000), "1d1h");
    }

    #[test]
    fn trace_cells_full() {
        let t = json!({
            "ts_request_ms": 1_700_000_000_000i64,
            "ts_response_ms": 1_700_000_000_500i64,
            "harness": "claude",
            "client_format": "anthropic",
            "upstream_format": "openai",
            "upstream_provider": "xai",
            "requested_model": "claude-x",
            "routed_model": "grok-4",
            "status": 200,
            "streamed": true,
            "input_tokens": 120,
            "output_tokens": 45,
            "cost_usd": 0.01234,
            "session_id": "sess-abcdef123456789",
            "error": null
        });
        let c = trace_cells(&t);
        assert_eq!(c.model, "grok-4");
        assert_eq!(c.provider, "xai");
        assert_eq!(c.fmt, "anthropic→openai");
        assert!(c.cross);
        assert_eq!(c.status, "200");
        assert_eq!(c.status_class, 2);
        assert_eq!(c.tokens_in, "120");
        assert_eq!(c.tokens_out, "45");
        assert_eq!(c.cost, "$0.0123");
        assert!(c.session.ends_with('…'));
        assert!(c.error.is_empty());
    }

    #[test]
    fn trace_cells_missing_fields() {
        let c = trace_cells(&json!({}));
        assert_eq!(c.model, "-");
        assert_eq!(c.provider, "-");
        assert_eq!(c.fmt, "-→-");
        assert!(!c.cross);
        assert_eq!(c.status, "-");
        assert_eq!(c.status_class, 0);
        assert_eq!(c.tokens_in, "-");
        assert_eq!(c.cost, "-");
    }

    #[test]
    fn trace_cells_error_and_fallback_model() {
        let t = json!({
            "requested_model": "gpt-5",
            "status": 502,
            "error": "upstream exploded in a very long and detailed way that should be truncated somewhere"
        });
        let c = trace_cells(&t);
        assert_eq!(c.model, "gpt-5");
        assert_eq!(c.status_class, 5);
        assert!(c.error.chars().count() <= 48);
        assert!(c.error.ends_with('…'));
    }

    #[test]
    fn truncate_char_boundaries() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("ábcdéfghij", 5), "ábcd…");
    }

    #[test]
    fn reset_secs_variants() {
        let now = 1_700_000_000i64;
        let w = json!({"resets_at_s": now + 3600});
        assert_eq!(reset_secs(&w, now), Some(3600));
        let w = json!({"resets_at": "2023-11-14T22:13:20Z"});
        assert_eq!(reset_secs(&w, now - 100), Some(100));
        assert_eq!(reset_secs(&json!({}), now), None);
    }

    #[test]
    fn session_kind_classifies() {
        let ping = serde_json::json!({"harness": "alexandria-ping", "session_id": "x"});
        let test = serde_json::json!({"harness": "curl/8", "session_id": "tsh-W1-99"});
        let e2e = serde_json::json!({"session_id": "alexandria-e2e-claude-1"});
        let real = serde_json::json!({"harness": "claude-cli/2.1", "session_id": "abc"});
        assert_eq!(session_kind(&ping), Some("ping"));
        assert_eq!(session_kind(&test), Some("test"));
        assert_eq!(session_kind(&e2e), Some("test"));
        assert_eq!(session_kind(&real), None);
        let all = vec![ping, test, real.clone()];
        assert_eq!(visible_sessions(&all, false).len(), 1);
        assert_eq!(visible_sessions(&all, true).len(), 3);
    }

    #[test]
    fn session_cells_full() {
        let s = json!({
            "session_id": "sess-0123456789abcdef0123456789",
            "run_id": "run-42",
            "first_ts_ms": 1_699_999_000_000i64,
            "last_ts_ms": 1_700_000_000_000i64,
            "trace_count": 7,
            "models": ["grok-4", "claude-x"],
            "total_input_tokens": 45_000,
            "total_output_tokens": 320,
            "total_cost_usd": 0.5,
            "errors": 2
        });
        let c = session_cells(&s);
        assert_eq!(c.id.chars().count(), 24);
        assert!(c.id.ends_with('…'));
        assert_eq!(c.run, "run-42");
        assert_eq!(c.models, "grok-4,claude-x");
        assert_eq!(c.turns, "7 turns");
        assert_eq!(c.tokens, "45.0k→320");
        assert_eq!(c.cost, "$0.5000");
        assert_eq!(c.errors, "2 err");
        assert_ne!(c.time, "--:--:--");
    }

    #[test]
    fn session_cells_missing_fields() {
        let c = session_cells(&json!({}));
        assert_eq!(c.time, "--:--:--");
        assert_eq!(c.id, "-");
        assert!(c.run.is_empty());
        assert_eq!(c.models, "-");
        assert_eq!(c.turns, "-");
        assert_eq!(c.tokens, "-");
        assert_eq!(c.cost, "-");
        assert!(c.errors.is_empty());
    }

    #[test]
    fn models_short_variants() {
        assert_eq!(models_short(Some(&json!(["a", "b"]))), "a,b");
        assert_eq!(models_short(Some(&json!("a, b"))), "a, b");
        assert_eq!(models_short(None), "-");
        assert_eq!(models_short(Some(&json!(42))), "-");
        let long = models_short(Some(&json!("very-long-model-name-that-overflows-here")));
        assert!(long.ends_with('…'));
        assert!(long.chars().count() <= 24);
    }

    #[test]
    fn turn_header_full() {
        let t = json!({
            "ts_request_ms": 1_700_000_000_000i64,
            "model": "grok-4",
            "input_tokens": 120,
            "output_tokens": 45,
            "cost_usd": 0.01234,
            "status": 200
        });
        let h = turn_header_text(&t);
        assert!(h.starts_with("── "));
        assert!(h.ends_with(" ──"));
        assert!(h.contains("grok-4"));
        assert!(h.contains("120→45 tok"));
        assert!(h.contains("$0.0123"));
        assert!(h.contains("· 200 ──"));
    }

    #[test]
    fn turn_header_missing_fields() {
        let h = turn_header_text(&json!({}));
        assert!(h.contains("--:--:--"));
        assert!(h.contains("-→- tok"));
        assert!(h.contains("· - · - ──"));
    }

    #[test]
    fn wrap_text_basics() {
        assert_eq!(wrap_text("hello world", 20), vec!["hello world"]);
        assert_eq!(
            wrap_text("hello world again", 11),
            vec!["hello world", "again"]
        );
        assert_eq!(wrap_text("a\n\nb", 10), vec!["a", "", "b"]);
        let long = "x".repeat(25);
        assert_eq!(
            wrap_text(&long, 10),
            vec!["x".repeat(10), "x".repeat(10), "x".repeat(5)]
        );
        for l in wrap_text("many words that should wrap nicely at the boundary", 12) {
            assert!(l.chars().count() <= 12);
        }
    }

    #[test]
    fn cap_lines_tail() {
        let lines: Vec<String> = (0..50).map(|i| i.to_string()).collect();
        let (kept, hidden) = cap_lines(lines, 40);
        assert_eq!(kept.len(), 40);
        assert_eq!(hidden, 10);
        let (kept, hidden) = cap_lines(vec!["a".into()], 40);
        assert_eq!(kept.len(), 1);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn fmt_count_scales() {
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(9_999), "9999");
        assert_eq!(fmt_count(45_000), "45.0k");
        assert_eq!(fmt_count(2_500_000), "2.5M");
    }

    #[test]
    fn raw_active_fallback() {
        assert!(!raw_active(None, false));
        assert!(!raw_active(Some(true), false));
        assert!(raw_active(Some(false), false));
        assert!(raw_active(Some(true), true));
    }

    #[test]
    fn limit_items_build() {
        let providers = vec![json!({
            "provider": "anthropic",
            "plan": "max",
            "source": "oauth",
            "windows": [{"window": "5h", "used_pct": 63.0, "resets_at_s": chrono::Utc::now().timestamp() + 8160}],
        })];
        let items = limit_items(&providers);
        let meters: Vec<_> = items
            .iter()
            .filter_map(|i| match i {
                LimitItem::Meter { label, pct } => Some((label.clone(), *pct)),
                _ => None,
            })
            .collect();
        assert_eq!(meters.len(), 1);
        assert!(meters[0].0.contains("anthropic 5h"));
        assert!(meters[0].0.contains("resets in"));
        assert_eq!(meters[0].1, 63.0);
    }
}
