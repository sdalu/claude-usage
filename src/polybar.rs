//! Single-line output for polybar's `custom/script` module: one compact
//! segment per usage window, coloured by severity with polybar's own format
//! tags. Printed once (polybar's `interval` drives the refresh), or, with
//! `--interval`, printed on a loop by one long-lived process for a `tail = true`
//! module — which is the friendlier mode for a rate-limited API, since the
//! cadence is then ours to back off: a failed fetch doubles the wait and the
//! last good line stays on the bar with a stale marker, instead of polybar
//! re-running the script into the same limit.
//!
//! ```text
//! %{F#50fa7b}5h 11%%{F-} · %{T2}%{F#f1fa8c}wk 39%%{F-}%{T-} · %{F#ff5555}Sonnet 82%%{F-}
//! ```
//!
//! The binding window (the API's `is_active` — the one you hit first) is set in
//! polybar's second font slot, which is where bar configs conventionally put a
//! bold face. `--no-color` drops every tag, leaving plain text that suits
//! waybar, i3blocks, tmux or a shell prompt just as well.

use std::io::Write;
use std::time::Duration;

use crate::usage_api::{LimitWindow, UsageStatus};
use crate::views::{level, Level};

/// Separator between window segments.
const SEP: &str = " \u{b7} ";

/// Polybar font slot used for the binding window; `font-1` in the bar config
/// (slots are 1-based in the tag, 0-based in the config key). 0 disables it.
pub const DEFAULT_BOLD_FONT: u8 = 2;

/// Shortest tail interval accepted, so a typo can't turn the module into a
/// hammer on a rate-limited endpoint.
pub const MIN_INTERVAL: u64 = 30;

/// Shared-cache TTL for a one-shot module, sized for the usual polybar
/// `interval = 300` so the bars on other monitors reuse one fetch.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(240);

/// Ceiling on the doubling backoff after repeated failures: past this, retrying
/// harder will not help, and the bar keeps showing the last good numbers.
const MAX_BACKOFF: Duration = Duration::from_secs(1800);

/// Appended to the last good line while refreshes are failing.
const STALE: &str = " \u{26a0}";

/// Severity colours, as hex for polybar's `%{F...}` tag.
pub struct Palette {
    pub ok: String,
    pub warn: String,
    pub crit: String,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            ok: "#50fa7b".into(),
            warn: "#f1fa8c".into(),
            crit: "#ff5555".into(),
        }
    }
}

impl Palette {
    /// Parses the `--colors ok,warn,crit` triple; each entry may be empty to
    /// keep the default for that level.
    pub fn parse(spec: &str) -> Result<Palette, String> {
        let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
        if parts.len() != 3 {
            return Err("expected three comma-separated colours: ok,warn,crit".into());
        }
        let mut palette = Palette::default();
        for (slot, value) in [&mut palette.ok, &mut palette.warn, &mut palette.crit]
            .into_iter()
            .zip(parts)
        {
            if value.is_empty() {
                continue;
            }
            if !is_hex_color(value) {
                return Err(format!("not a #rrggbb colour: {value}"));
            }
            *slot = value.to_string();
        }
        Ok(palette)
    }
}

/// `#rgb`, `#rrggbb` or `#aarrggbb` — the forms polybar accepts.
fn is_hex_color(value: &str) -> bool {
    let hex = match value.strip_prefix('#') {
        Some(h) => h,
        None => return false,
    };
    matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

pub struct Options {
    /// Emit `%{F...}` colour tags. Off for plain text.
    pub color: bool,
    /// Font slot for the binding window, or 0 for no emphasis.
    pub bold_font: u8,
    pub palette: Palette,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            color: true,
            bold_font: DEFAULT_BOLD_FONT,
            palette: Palette::default(),
        }
    }
}

/// The whole module line — the segments, or a short error notice when the
/// fetch failed (polybar has one line to say it in).
pub fn render(status: &Result<UsageStatus, String>, opts: &Options) -> String {
    match status {
        Ok(s) if s.windows.is_empty() => opts.paint("claude: no limits", Level::Warn),
        Ok(s) => s
            .windows
            .iter()
            .map(|w| segment(w, opts))
            .collect::<Vec<_>>()
            .join(SEP),
        Err(e) => opts.paint(&format!("claude: {}", short_error(e)), Level::Crit),
    }
}

/// Tail mode: one long-lived process printing a line per refresh for a polybar
/// `tail = true` module. Fetches on `interval`, doubling the wait after each
/// consecutive failure (up to [`MAX_BACKOFF`]) so a rate-limited API is not
/// hammered, and keeps the last good line on the bar — marked stale — instead
/// of blanking it out. Returns when stdout closes, i.e. polybar went away.
pub fn run_tail(
    interval: Duration,
    opts: &Options,
    refresh: impl Fn() -> Result<UsageStatus, String>,
) -> i32 {
    let mut last_good: Option<UsageStatus> = None;
    let mut failures: u32 = 0;

    loop {
        let status = refresh();
        let line = match &status {
            Ok(s) => {
                last_good = Some(s.clone());
                failures = 0;
                render(&status, opts)
            }
            Err(_) => {
                failures = failures.saturating_add(1);
                match &last_good {
                    // Refresh failed but we have numbers: keep them, flagged.
                    Some(previous) => stale(previous, opts),
                    // Nothing was ever fetched: the error is all we can say.
                    None => render(&status, opts),
                }
            }
        };
        if emit(&line).is_err() {
            return 0;
        }
        std::thread::sleep(backoff(interval, failures));
    }
}

/// The last good line with a stale marker, shown while refreshes fail.
fn stale(previous: &UsageStatus, opts: &Options) -> String {
    let line = render(&Ok(previous.clone()), opts);
    format!("{line}{}", opts.paint(STALE, Level::Warn))
}

/// How long to wait before the next fetch: the configured interval, doubled per
/// consecutive failure and capped at [`MAX_BACKOFF`].
fn backoff(interval: Duration, failures: u32) -> Duration {
    match failures {
        0 => interval,
        n => interval
            .saturating_mul(2u32.saturating_pow(n.min(16)))
            .min(MAX_BACKOFF)
            .max(interval),
    }
}

/// Writes one line and flushes it: piped stdout is block-buffered, so without
/// the flush polybar would see nothing until the buffer filled.
fn emit(line: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}")?;
    out.flush()
}

/// "5h 11%", coloured by severity and emphasised when it is the binding window.
fn segment(window: &LimitWindow, opts: &Options) -> String {
    let percent = window.percent.round() as i64;
    let ratio = (window.percent / 100.0).clamp(0.0, 1.0);
    let text = format!("{} {percent}%", short_label(window));
    let painted = opts.paint(&text, level(&window.severity, ratio));
    if window.is_active {
        opts.embolden(&painted)
    } else {
        painted
    }
}

/// Bar-sized name for a window: "5h", "wk", the model name for a per-model
/// week, "$" for spend.
fn short_label(window: &LimitWindow) -> String {
    let label = sanitize(&window.label);
    match window.group.as_str() {
        "session" => "5h".to_string(),
        "spend" => "$".to_string(),
        "weekly" => match label.split_once('\u{b7}') {
            // "Week · Sonnet" -> "Sonnet"
            Some((_, model)) => model.trim().to_string(),
            None => "wk".to_string(),
        },
        _ => label,
    }
}

/// First clause of a fetch error, so the module stays bar-sized: the messages
/// are written as "<summary> — <what to do>".
fn short_error(error: &str) -> String {
    let head = error
        .split(" \u{2014} ")
        .next()
        .unwrap_or(error)
        .trim_end_matches('.');
    sanitize(head)
}

/// Neutralises polybar's `%{...}` tag opener in text that came from the API
/// (model display names) or an error message, so it cannot inject formatting.
fn sanitize(text: &str) -> String {
    text.replace("%{", "%\u{2774}")
}

impl Options {
    fn paint(&self, text: &str, level: Level) -> String {
        if !self.color {
            return text.to_string();
        }
        let color = match level {
            Level::Ok => &self.palette.ok,
            Level::Warn => &self.palette.warn,
            Level::Crit => &self.palette.crit,
        };
        format!("%{{F{color}}}{text}%{{F-}}")
    }

    fn embolden(&self, text: &str) -> String {
        if self.bold_font == 0 {
            return text.to_string();
        }
        format!("%{{T{}}}{text}%{{T-}}", self.bold_font)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_api;

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

    /// Also the source of the README example (`cargo test polybar_line -- --nocapture`).
    #[test]
    fn polybar_line() {
        let line = render(&usage_api::parse(SAMPLE), &Options::default());
        assert_eq!(
            line,
            "%{F#50fa7b}5h 11%%{F-} \u{b7} \
             %{T2}%{F#50fa7b}wk 39%%{F-}%{T-} \u{b7} \
             %{F#f1fa8c}Sonnet 62%%{F-}"
        );
        println!("\n{line}");
    }

    #[test]
    fn plain_line_has_no_tags() {
        let opts = Options {
            color: false,
            bold_font: 0,
            ..Options::default()
        };
        let line = render(&usage_api::parse(SAMPLE), &opts);
        assert_eq!(line, "5h 11% \u{b7} wk 39% \u{b7} Sonnet 62%");
    }

    #[test]
    fn binding_window_is_emboldened() {
        let opts = Options {
            color: false,
            ..Options::default()
        };
        let line = render(&usage_api::parse(SAMPLE), &opts);
        assert!(line.contains("%{T2}wk 39%%{T-}"));
        // Only the binding window is.
        assert!(!line.contains("%{T2}5h"));
    }

    #[test]
    fn error_is_one_short_coloured_notice() {
        let status = Err("Access token expired \u{2014} run Claude Code, or pass \
                          --write-back."
            .to_string());
        assert_eq!(
            render(&status, &Options::default()),
            "%{F#ff5555}claude: Access token expired%{F-}"
        );
    }

    #[test]
    fn empty_limits_say_so() {
        let opts = Options {
            color: false,
            ..Options::default()
        };
        assert_eq!(render(&usage_api::parse("{}"), &opts), "claude: no limits");
    }

    #[test]
    fn labels_are_bar_sized() {
        let opts = Options {
            color: false,
            bold_font: 0,
            ..Options::default()
        };
        let body = r#"{"limits":[{"kind":"session","group":"session","percent":30,
            "severity":"normal","scope":null,"is_active":false}],
            "spend":{"enabled":true,"percent":12.0,"severity":"normal"}}"#;
        assert_eq!(render(&usage_api::parse(body), &opts), "5h 30% \u{b7} $ 12%");
    }

    #[test]
    fn api_text_cannot_inject_format_tags() {
        let body = r#"{"limits":[{"kind":"weekly_scoped","group":"weekly","percent":5,
            "severity":"normal","scope":{"model":{"display_name":"%{F#ff0000}X"}},
            "is_active":false}]}"#;
        let opts = Options {
            color: false,
            bold_font: 0,
            ..Options::default()
        };
        assert!(!render(&usage_api::parse(body), &opts).contains("%{F#ff0000}"));
    }

    #[test]
    fn backoff_doubles_then_caps() {
        let interval = Duration::from_secs(300);
        assert_eq!(backoff(interval, 0), interval);
        assert_eq!(backoff(interval, 1), Duration::from_secs(600));
        assert_eq!(backoff(interval, 2), Duration::from_secs(1200));
        // Capped, and never shorter than the interval however far it is pushed.
        assert_eq!(backoff(interval, 9), MAX_BACKOFF);
        assert_eq!(backoff(Duration::from_secs(3600), 9), Duration::from_secs(3600));
    }

    #[test]
    fn stale_line_keeps_the_last_numbers() {
        let opts = Options {
            color: false,
            bold_font: 0,
            ..Options::default()
        };
        let previous = usage_api::parse(SAMPLE).unwrap();
        assert_eq!(
            stale(&previous, &opts),
            "5h 11% \u{b7} wk 39% \u{b7} Sonnet 62% \u{26a0}"
        );
    }

    #[test]
    fn palette_parsing() {
        let palette = Palette::parse("#111111,,#333").unwrap();
        assert_eq!(palette.ok, "#111111");
        // Empty entries keep the default.
        assert_eq!(palette.warn, Palette::default().warn);
        assert_eq!(palette.crit, "#333");

        assert!(Palette::parse("#111111,#222222").is_err());
        assert!(Palette::parse("red,#222222,#333333").is_err());
    }
}
