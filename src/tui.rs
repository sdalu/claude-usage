//! A single-screen live usage monitor: one progress bar per usage window from
//! `/api/oauth/usage`. `q`/`Esc` quits, `r` re-fetches.

use std::io;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    symbols,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, LineGauge, Padding, Paragraph},
    DefaultTerminal, Frame, Terminal,
};

use crate::usage_api::{self, UsageStatus};
use crate::views::{GaugeView, Views};

/// How long the "?" help marker flashes at startup before fading away.
const INTRO_SECS: f64 = 3.0;

pub struct App {
    views: Views,
    plan: Option<String>,
    last_refresh: DateTime<Local>,
    auto_interval: chrono::Duration,
    show_help: bool,
    show_updated: bool,
    show_intro: bool,
    border: bool,
    started: Instant,
    intro_was_active: bool,
}

impl App {
    pub fn new(status: Result<UsageStatus, String>, plan: Option<String>) -> Self {
        App {
            views: crate::views::build(&status),
            plan,
            last_refresh: Local::now(),
            auto_interval: chrono::Duration::minutes(5),
            show_help: false,
            show_updated: false,
            show_intro: true,
            border: true,
            started: Instant::now(),
            intro_was_active: false,
        }
    }

    /// Disables the startup "?" flash (used by single-shot rendering).
    pub fn without_intro(mut self) -> Self {
        self.show_intro = false;
        self
    }

    /// Drops the surrounding box (and the title/banner it carries); only the
    /// gauge rows are rendered. Intended for single-shot output.
    pub fn without_border(mut self) -> Self {
        self.border = false;
        self
    }

    /// Renders one frame inline to stdout (with colour) and returns; used by
    /// the `-1` single-shot mode.
    pub fn render_once(mut self) -> io::Result<()> {
        let width = crossterm::terminal::size()
            .map(|(w, _)| w)
            .unwrap_or(80)
            .max(40);
        let mut terminal = Terminal::new(TestBackend::new(width, self.box_height()))?;
        terminal.draw(|f| self.draw(f))?;
        print!("{}", buffer_to_ansi(terminal.backend().buffer()));
        Ok(())
    }

    fn box_height(&self) -> u16 {
        let rows = self.views.windows.len().max(1) as u16;
        if self.border {
            rows + 2
        } else {
            rows
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut needs_draw = true;
        loop {
            // Keep redrawing while the startup "?" animation plays, and once
            // more on the frame it stops so the marker is cleared.
            let intro = self.intro_active();
            if intro || self.intro_was_active != intro {
                needs_draw = true;
            }
            self.intro_was_active = intro;

            if needs_draw {
                terminal.draw(|f| self.draw(f))?;
                needs_draw = false;
            }
            // Wake up regularly so the auto-refresh fires (and the intro
            // animation advances) without a keypress.
            if event::poll(Duration::from_millis(200))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if self.handle_key(key) {
                            break;
                        }
                        needs_draw = true;
                    }
                    Event::Resize(_, _) => needs_draw = true,
                    _ => {}
                }
            }
            if Local::now() - self.last_refresh >= self.auto_interval {
                self.refresh();
                needs_draw = true;
            }
        }
        Ok(())
    }

    /// Returns true when the app should quit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl_c = key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL);

        // While help is open, any key dismisses it (q / Ctrl-C still quit).
        if self.show_help {
            if matches!(key.code, KeyCode::Char('q')) || ctrl_c {
                return true;
            }
            self.show_help = false;
            return false;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            _ if ctrl_c => return true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('t') => self.toggle_interval(),
            KeyCode::Char('u') => self.show_updated = !self.show_updated,
            _ => {}
        }
        false
    }

    fn refresh(&mut self) {
        self.views = crate::views::build(&usage_api::fetch());
        self.last_refresh = Local::now();
    }

    fn toggle_interval(&mut self) {
        self.auto_interval = if self.auto_interval == chrono::Duration::minutes(1) {
            chrono::Duration::minutes(5)
        } else {
            chrono::Duration::minutes(1)
        };
    }

    fn interval_label(&self) -> &'static str {
        if self.auto_interval == chrono::Duration::minutes(1) {
            "1m"
        } else {
            "5m"
        }
    }

    fn intro_active(&self) -> bool {
        self.show_intro && self.started.elapsed().as_secs_f64() < INTRO_SECS
    }

    /// The startup help marker: a "?" that cycles colour to draw the eye, or
    /// None once the intro has elapsed (so the border closes back up).
    fn intro_hint(&self) -> Option<Line<'static>> {
        let elapsed = self.started.elapsed().as_secs_f64();
        if !self.show_intro || elapsed >= INTRO_SECS {
            return None;
        }
        const PALETTE: [Color; 4] = [
            Color::LightYellow,
            Color::LightCyan,
            Color::LightMagenta,
            Color::LightGreen,
        ];
        let color = PALETTE[((elapsed * 5.0) as usize) % PALETTE.len()];
        Some(Line::from(Span::styled(" ? ", Style::new().fg(color).bold())).right_aligned())
    }

    fn draw(&mut self, f: &mut Frame) {
        let chunks =
            Layout::vertical([Constraint::Length(self.box_height()), Constraint::Min(0)])
                .split(f.area());

        self.draw_windows(f, chunks[0]);
        if self.show_help {
            self.draw_help(f, f.area());
        }
    }

    /// Draws the surrounding box with its title, optional updated banner and
    /// startup "?" hint, and returns the inner content area.
    fn draw_box(&self, f: &mut Frame, area: Rect) -> Rect {
        let title = match &self.plan {
            Some(plan) => format!(" Claude usage limits \u{b7} {plan} "),
            None => " Claude usage limits ".to_string(),
        };
        let mut block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            // Left margin comes from the " name" lead space; match it on the right.
            .padding(Padding::right(1));
        // Notch the last-updated time into the bottom border (toggle with "u").
        // The "/" separator keeps the border colour; the rest is dimmed.
        if self.show_updated {
            let updated = Line::from(vec![
                Span::raw("\u{2524} "),
                Span::styled(
                    format!("updated {} ", self.last_refresh.format("%H:%M")),
                    Style::new().dim(),
                ),
                Span::raw("/"),
                Span::styled(
                    format!(" {}", self.auto_interval.num_minutes()),
                    Style::new().dim(),
                ),
                Span::raw(" \u{251c}\u{2500}\u{2500}"),
            ])
            .right_aligned();
            block = block.title_bottom(updated);
        }
        // A "?" flashes on the top-right border at startup, then fades away.
        if let Some(hint) = self.intro_hint() {
            block = block.title(hint);
        }
        let inner = block.inner(area);
        f.render_widget(block, area);
        inner
    }

    fn draw_windows(&self, f: &mut Frame, area: Rect) {
        // Borderless mode (single-shot): just the gauge rows, no box/title/banner.
        // No left margin (the rows lose their lead space), one space on the right.
        let inner = if self.border {
            self.draw_box(f, area)
        } else {
            Rect {
                width: area.width.saturating_sub(1),
                ..area
            }
        };

        if self.views.windows.is_empty() {
            let msg = self
                .views
                .error
                .clone()
                .unwrap_or_else(|| "no active limits reported".to_string());
            f.render_widget(
                Paragraph::new(Span::styled(msg, Style::new().red())),
                inner,
            );
            return;
        }

        let name_width = self
            .views
            .windows
            .iter()
            .map(name_columns)
            .max()
            .unwrap_or(0)
            .clamp(8, 26) as u16;

        // Pick the most verbose label set that still leaves the bar a minimum
        // width: full, then drop "resets", then also drop "used". Applied to
        // every row so the columns stay aligned.
        let avail = inner
            .width
            .saturating_sub(name_width + 1)
            .saturating_sub(MIN_BAR);
        let (drop_resets, drop_used) = [(false, false), (true, false), (true, true)]
            .into_iter()
            .find(|&(dr, du)| max_text_width(&self.views.windows, dr, du) <= avail)
            .unwrap_or((true, true));
        let text_width = max_text_width(&self.views.windows, drop_resets, drop_used);

        let rows = Layout::vertical(
            self.views
                .windows
                .iter()
                .map(|_| Constraint::Length(1))
                .collect::<Vec<_>>(),
        )
        .split(inner);

        for (gauge, row) in self.views.windows.iter().zip(rows.iter()) {
            render_gauge_row(
                f, *row, gauge, name_width, text_width, drop_resets, drop_used, self.border,
            );
        }
    }

    fn draw_help(&self, f: &mut Frame, area: Rect) {
        let key = |k: &'static str, desc: String| {
            Line::from(vec![
                Span::styled(format!("  {k:<14}"), Style::new().cyan().bold()),
                Span::raw(desc),
            ])
        };
        let lines = vec![
            key("q / Esc", "quit".to_string()),
            key("Ctrl-C", "quit".to_string()),
            key("r", "refresh now".to_string()),
            key(
                "t",
                format!("toggle auto-refresh (now {})", self.interval_label()),
            ),
            key("u", "toggle the updated banner".to_string()),
            key("?", "toggle this help".to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "  any key closes this help",
                Style::new().dim(),
            )),
        ];

        let rect = centered(area, 50, lines.len() as u16 + 2);
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Keys ")),
            rect,
        );
    }
}

/// A centred rectangle of the given size, clamped to `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// Minimum width to keep for the bar when choosing how verbose the labels are.
const MIN_BAR: u16 = 8;

/// "NN% used", or just "NN%" when `drop_used`. Right-justified so the dot after
/// it lines up across rows.
fn percent_text(percent: i64, drop_used: bool) -> String {
    if drop_used {
        format!("{percent:>3}%")
    } else {
        format!("{percent:>3}% used")
    }
}

/// "resets <suffix>", or just "<suffix>" when `drop_resets`; empty if no reset.
fn reset_text(reset: &str, drop_resets: bool) -> String {
    if reset.is_empty() {
        String::new()
    } else if drop_resets {
        reset.to_string()
    } else {
        format!("resets {reset}")
    }
}

/// Visible width of the trailing text column at a given verbosity.
fn text_width(gauge: &GaugeView, drop_resets: bool, drop_used: bool) -> u16 {
    let mut width = 1 + percent_text(gauge.percent, drop_used).chars().count();
    if !gauge.dollars.is_empty() {
        width += 3 + gauge.dollars.chars().count();
    }
    let reset = reset_text(&gauge.reset, drop_resets);
    if !reset.is_empty() {
        width += 3 + reset.chars().count();
    }
    width as u16
}

fn max_text_width(windows: &[GaugeView], drop_resets: bool, drop_used: bool) -> u16 {
    windows
        .iter()
        .map(|g| text_width(g, drop_resets, drop_used))
        .max()
        .unwrap_or(0)
}

/// Width the name column needs, leaving room for the active marker.
fn name_columns(gauge: &GaugeView) -> usize {
    gauge.name.chars().count() + if gauge.active { 2 } else { 0 }
}

#[allow(clippy::too_many_arguments)]
fn render_gauge_row(
    f: &mut Frame,
    area: Rect,
    gauge: &GaugeView,
    name_width: u16,
    text_width: u16,
    drop_resets: bool,
    drop_used: bool,
    lead_space: bool,
) {
    let cols = Layout::horizontal([
        Constraint::Length(name_width + 1),
        Constraint::Min(MIN_BAR),
        Constraint::Length(text_width),
    ])
    .split(area);

    let color = severity_color(&gauge.severity, gauge.ratio);

    // Window name; the binding (active) window is marked and brightened. The
    // lead space is the inner-left margin in bordered mode; dropped otherwise.
    let lead = if lead_space { " " } else { "" };
    let name = if gauge.active {
        format!("{lead}{} \u{25c0}", gauge.name)
    } else {
        format!("{lead}{}", gauge.name)
    };
    let name_style = if gauge.active {
        Style::new().fg(Color::White).bold()
    } else {
        Style::new().cyan()
    };
    f.render_widget(Paragraph::new(Span::styled(name, name_style)), cols[0]);

    // Progress bar.
    let bar = LineGauge::default()
        .ratio(gauge.ratio)
        .line_set(symbols::line::THICK)
        .filled_style(Style::new().fg(color))
        .unfilled_style(Style::new().dark_gray())
        .label("");
    f.render_widget(bar, cols[1]);

    // Aligned trailing text: " NN% used · $… · resets …".
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(percent_text(gauge.percent, drop_used), Style::new().fg(color)),
    ];
    if !gauge.dollars.is_empty() {
        spans.push(Span::raw(" \u{b7} "));
        spans.push(Span::styled(gauge.dollars.clone(), Style::new().green()));
    }
    let reset = reset_text(&gauge.reset, drop_resets);
    if !reset.is_empty() {
        spans.push(Span::raw(" \u{b7} "));
        spans.push(Span::styled(reset, Style::new().dim()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), cols[2]);
}

/// Renders a ratatui buffer to an ANSI-coloured string for `-1` single-shot
/// output (so the frame stays in the terminal after the program exits).
fn buffer_to_ansi(buf: &Buffer) -> String {
    use std::fmt::Write;
    let area = *buf.area();
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            let codes = sgr_codes(&cell.style());
            if codes.is_empty() {
                out.push_str(cell.symbol());
            } else {
                let _ = write!(out, "\u{1b}[{codes}m{}\u{1b}[0m", cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

fn sgr_codes(style: &Style) -> String {
    let mut codes: Vec<String> = Vec::new();
    let m = style.add_modifier;
    if m.contains(Modifier::BOLD) {
        codes.push("1".into());
    }
    if m.contains(Modifier::DIM) {
        codes.push("2".into());
    }
    if m.contains(Modifier::REVERSED) {
        codes.push("7".into());
    }
    if let Some(code) = style.fg.and_then(fg_code) {
        codes.push(code);
    }
    codes.join(";")
}

fn fg_code(color: Color) -> Option<String> {
    let code = match color {
        Color::Reset => return None,
        Color::Black => "30",
        Color::Red => "31",
        Color::Green => "32",
        Color::Yellow => "33",
        Color::Blue => "34",
        Color::Magenta => "35",
        Color::Cyan => "36",
        Color::Gray => "37",
        Color::DarkGray => "90",
        Color::LightRed => "91",
        Color::LightGreen => "92",
        Color::LightYellow => "93",
        Color::LightBlue => "94",
        Color::LightMagenta => "95",
        Color::LightCyan => "96",
        Color::White => "97",
        Color::Indexed(i) => return Some(format!("38;5;{i}")),
        Color::Rgb(r, g, b) => return Some(format!("38;2;{r};{g};{b}")),
    };
    Some(code.to_string())
}

/// Bar colour: honour the API's severity, falling back to thresholds on the
/// fill ratio when severity is the default "normal".
fn severity_color(severity: &str, ratio: f64) -> Color {
    match severity {
        "warning" => Color::Yellow,
        "critical" | "exceeded" | "blocked" => Color::Red,
        _ => {
            if ratio >= 0.8 {
                Color::Red
            } else if ratio >= 0.5 {
                Color::Yellow
            } else {
                Color::Green
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_api;
    use ratatui::{backend::TestBackend, Terminal};

    const SAMPLE: &str = r#"{
        "limits": [
            {"kind":"session","group":"session","percent":11,"severity":"normal",
             "resets_at":"2026-06-19T20:30:00+00:00","scope":null,"is_active":false},
            {"kind":"weekly_all","group":"weekly","percent":39,"severity":"normal",
             "resets_at":"2026-06-23T06:00:00+00:00","scope":null,"is_active":true},
            {"kind":"weekly_scoped","group":"weekly","percent":62,"severity":"warning",
             "resets_at":"2026-06-23T06:00:00+00:00",
             "scope":{"model":{"display_name":"Sonnet"}},"is_active":false}
        ]
    }"#;

    /// Renders one frame of the monitor; also the source of the README
    /// screenshot (`cargo test render_monitor -- --nocapture`).
    #[test]
    fn render_monitor() {
        let status = usage_api::parse(SAMPLE);
        let mut app = App::new(status, Some("Max 5x".to_string()));

        let backend = TestBackend::new(74, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();

        let rendered = format!("{}", terminal.backend());
        assert!(rendered.contains("Claude usage limits"));
        assert!(rendered.contains("Session (5h)"));
        println!("\n{rendered}");
    }

    fn render_at(width: u16) -> String {
        let mut app = App::new(usage_api::parse(SAMPLE), None);
        let mut terminal = Terminal::new(TestBackend::new(width, 6)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        format!("{}", terminal.backend())
    }

    #[test]
    fn borderless_has_no_box_and_no_left_margin() {
        let mut app = App::new(usage_api::parse(SAMPLE), Some("Max 5x".into()))
            .without_intro()
            .without_border();
        let mut terminal = Terminal::new(TestBackend::new(74, app.box_height())).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();

        let buf = terminal.backend().buffer();
        let width = buf.area().width;
        let first: String = (0..width).map(|x| buf[(x, 0)].symbol()).collect();
        // No top border, and the first row starts at column 0 (lead space gone).
        assert!(first.starts_with("Session (5h)"));
    }

    #[test]
    fn help_overlay_lists_keys() {
        let mut app = App::new(usage_api::parse(SAMPLE), None);
        app.show_help = true;
        let mut terminal = Terminal::new(TestBackend::new(74, 14)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let out = format!("{}", terminal.backend());
        assert!(out.contains("Keys"));
        assert!(out.contains("refresh now"));
        assert!(out.contains("toggle this help"));
    }

    #[test]
    fn updated_banner_toggles() {
        let mut app = App::new(usage_api::parse(SAMPLE), None);
        let render = |app: &mut App| {
            let mut t = Terminal::new(TestBackend::new(74, 6)).unwrap();
            t.draw(|f| app.draw(f)).unwrap();
            format!("{}", t.backend())
        };
        // Off by default.
        assert!(!render(&mut app).contains("updated"));
        app.show_updated = true;
        assert!(render(&mut app).contains("updated"));
    }

    #[test]
    fn without_intro_hides_the_marker() {
        let mut app = App::new(usage_api::parse(SAMPLE), Some("Max 5x".into())).without_intro();
        assert!(app.intro_hint().is_none());

        let mut terminal = Terminal::new(TestBackend::new(74, 5)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let out = format!("{}", terminal.backend());
        assert!(out.contains("Claude usage limits"));
        assert!(!out.contains('?'));
    }

    #[test]
    fn intro_marker_fades_after_timeout() {
        let mut app = App::new(usage_api::parse(SAMPLE), None);
        assert!(app.intro_active());
        assert!(app.intro_hint().is_some());

        // Back-date the start so the intro has "elapsed".
        app.started = std::time::Instant::now() - std::time::Duration::from_secs(10);
        assert!(!app.intro_active());
        assert!(app.intro_hint().is_none());
    }

    #[test]
    fn labels_drop_resets_then_used_when_narrow() {
        // Wide: full labels.
        let wide = render_at(74);
        assert!(wide.contains("resets"));
        assert!(wide.contains("used"));

        // Medium: "resets" goes first, "used" stays.
        let medium = render_at(58);
        assert!(!medium.contains("resets"));
        assert!(medium.contains("used"));

        // Narrow: "used" drops too (the percentage itself always stays).
        let narrow = render_at(48);
        assert!(!narrow.contains("used"));
        assert!(narrow.contains('%'));
    }
}
