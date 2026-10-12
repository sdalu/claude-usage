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
//!
//! Two markers can frame the line. An open incident on the status page puts a
//! `⚠` *in front*, coloured by the incident's impact; a refresh failing in tail
//! mode appends one *after* the last good numbers, in the warning colour. They
//! say different things — "Anthropic has a problem" versus "this line is old" —
//! so they sit at opposite ends.
//!
//! The incident marker is also a click target: it carries polybar's
//! `%{A1:…:}` action tag, so clicking the `⚠` — and only the `⚠` — opens that
//! incident's own status-page entry. A `click-right` in the module config can
//! only ever open the front door, because it has no way to know which incident
//! is showing; this does, so the click lands where the reader was looking.

use std::io::Write;
use std::time::Duration;

use crate::incidents::Incidents;
use crate::usage_api::{LimitWindow, UsageStatus};
use crate::views::{ahead_of_pace, level, Level};

/// Put after a window's percentage while it is being used faster than an even
/// rate would last it to the reset (see `views::ahead_of_pace`).
const AHEAD: &str = "\u{2191}";

/// Separator between window segments.
const SEP: &str = " \u{b7} ";

/// Put in front of the line while the status page lists an open incident.
const INCIDENT: &str = "\u{26a0}";

/// Command the incident marker's click runs, the URL appended. `xdg-open`
/// because it is what a desktop already has wired to a browser.
pub const DEFAULT_OPENER: &str = "xdg-open";

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

/// Longest error notice, in characters: past this it is cut with an ellipsis,
/// so an unexpected message (a transport error with its cause chain, the
/// API's own prose) cannot take over the bar.
const MAX_ERROR: usize = 40;

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
    /// Command the incident marker's click runs, with the incident's URL
    /// appended. Empty for a marker that is not clickable.
    pub opener: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            color: true,
            bold_font: DEFAULT_BOLD_FONT,
            palette: Palette::default(),
            opener: DEFAULT_OPENER.to_string(),
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

/// The module line, with a warning marker in front of it while the status page
/// lists an open incident (coloured by the worst one's impact, and clicking
/// through to that incident). Nothing is added when there is none — or when the
/// page could not be checked, which is unknown rather than bad news.
pub fn render_line(
    status: &Result<UsageStatus, String>,
    incidents: Option<&Incidents>,
    opts: &Options,
) -> String {
    let line = render(status, opts);
    match incidents.and_then(Incidents::worst) {
        Some(worst) => {
            let marker = opts.paint(INCIDENT, worst.impact.level());
            let marker = opts.on_left_click(&marker, worst.link());
            format!("{marker} {line}")
        }
        None => line,
    }
}

/// Tail mode: one long-lived process printing a line per refresh for a polybar
/// `tail = true` module. Fetches on `interval`, doubling the wait after each
/// consecutive failure (up to [`MAX_BACKOFF`]) so a rate-limited API is not
/// hammered, and keeps the last good line on the bar — marked stale — instead
/// of blanking it out. Returns when stdout closes, i.e. polybar went away.
///
/// `refresh` yields the usage and the status-page check together; only the
/// usage drives the backoff, the incident marker just reflects the latest check.
pub fn run_tail(
    interval: Duration,
    opts: &Options,
    refresh: impl Fn() -> (Result<UsageStatus, String>, Option<Incidents>),
) -> i32 {
    let mut last_good: Option<UsageStatus> = None;
    let mut failures: u32 = 0;

    loop {
        let (status, incidents) = refresh();
        let line = match &status {
            Ok(s) => {
                last_good = Some(s.clone());
                failures = 0;
                render_line(&status, incidents.as_ref(), opts)
            }
            Err(_) => {
                failures = failures.saturating_add(1);
                match &last_good {
                    // Refresh failed but we have numbers: keep them, flagged.
                    Some(previous) => stale(previous, incidents.as_ref(), opts),
                    // Nothing was ever fetched: the error is all we can say.
                    None => render_line(&status, incidents.as_ref(), opts),
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
fn stale(previous: &UsageStatus, incidents: Option<&Incidents>, opts: &Options) -> String {
    let line = render_line(&Ok(previous.clone()), incidents, opts);
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

/// "5h 11%", coloured by severity and emphasised when it is the binding window;
/// "5h 62%\u{2191}" when it is running ahead of its pace.
fn segment(window: &LimitWindow, opts: &Options) -> String {
    let percent = window.percent.round() as i64;
    let ratio = (window.percent / 100.0).clamp(0.0, 1.0);
    let ahead = if ahead_of_pace(window, chrono::Local::now()) {
        AHEAD
    } else {
        ""
    };
    let text = format!("{} {percent}%{ahead}", short_label(window));
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

/// Gist of a fetch error, so the module stays bar-sized: our own messages are
/// written as "<summary> — <what to do>", and chained ones (ureq's
/// "<url>: <cause>: <cause>") keep their last cause. Only the first line is
/// used, and it is capped at `MAX_ERROR` characters.
fn short_error(error: &str) -> String {
    let head = error.lines().next().unwrap_or("");
    let head = head.split(" \u{2014} ").next().unwrap_or(head);
    // The innermost cause says what went wrong; an HTTP status keeps its code
    // in front, since the API's message alone can be cryptic.
    let head = match (head.split_once(": "), head.rsplit_once(": ")) {
        (Some((first, _)), Some((_, last))) if first.contains("HTTP ") => {
            format!("{first}: {last}")
        }
        (_, Some((_, last))) => last.to_string(),
        _ => head.to_string(),
    };
    let head = head.split_whitespace().collect::<Vec<_>>().join(" ");
    let head = head.trim_end_matches('.');
    let head = match head.char_indices().nth(MAX_ERROR) {
        Some((cut, _)) => format!("{}\u{2026}", head[..cut].trim_end()),
        None => head.to_string(),
    };
    sanitize(&head)
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

    /// Wraps `text` in polybar's left-click action tag, so a click on that
    /// stretch of the line — and only that stretch — runs the opener on `url`.
    /// Dropped along with the colours in plain-text mode, since the tag is
    /// polybar's own syntax and means nothing to waybar, tmux or a shell
    /// prompt; dropped too when the opener has been emptied.
    fn on_left_click(&self, text: &str, url: &str) -> String {
        if !self.color || self.opener.is_empty() {
            return text.to_string();
        }
        // Single-quoted because polybar hands the command to a shell and the
        // URL came off the network; is_safe_url has already refused any URL
        // carrying a quote of its own. The opener is the caller's own string,
        // so it is left as written — `firefox --new-tab` has to keep working.
        let command = escape_action(&format!("{} '{url}'", self.opener));
        format!("%{{A1:{command}:}}{text}%{{A}}")
    }
}

/// Escapes a command for polybar's action tag, which ends the command at the
/// first unescaped `:` — and every URL has one immediately after its scheme.
fn escape_action(command: &str) -> String {
    command.replace(':', "\\:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::incidents::STATUS_PAGE;
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

    /// A usage reply whose windows reset at times relative to now, so the
    /// pace they are judged against does not rot with the calendar.
    fn paced(session_percent: u32, session_left_min: i64) -> String {
        let reset =
            (chrono::Local::now() + chrono::Duration::minutes(session_left_min)).to_rfc3339();
        format!(
            r#"{{"limits":[{{"kind":"session","group":"session",
                "percent":{session_percent},"severity":"normal",
                "resets_at":"{reset}","scope":null,"is_active":false}}]}}"#
        )
    }

    #[test]
    fn a_window_ahead_of_its_pace_carries_an_arrow() {
        let plain = Options {
            color: false,
            bold_font: 0,
            ..Options::default()
        };
        // Half the session gone (150 of 300 minutes left).
        let ahead = render(&usage_api::parse(&paced(62, 150)), &plain);
        assert_eq!(ahead, "5h 62%\u{2191}");
        let behind = render(&usage_api::parse(&paced(30, 150)), &plain);
        assert_eq!(behind, "5h 30%");
        // Five minutes in: too early to judge, however much is used.
        let early = render(&usage_api::parse(&paced(40, 295)), &plain);
        assert_eq!(early, "5h 40%");
        // The arrow is inside the colour, so it takes the window's severity.
        let coloured = render(&usage_api::parse(&paced(62, 150)), &Options::default());
        assert!(coloured.contains("62%\u{2191}%{F-}"), "{coloured}");
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
    fn long_error_is_cut_to_bar_size() {
        let opts = Options {
            color: false,
            ..Options::default()
        };
        let status = Err("https://api.anthropic.com/api/oauth/usage: Dns Failed: \
                          resolve dns name 'api.anthropic.com:443': failed to \
                          lookup address information: Name does not resolve"
            .to_string());
        assert_eq!(render(&status, &opts), "claude: Name does not resolve");
        let status = Err("HTTP 500: Internal\nserver error".to_string());
        assert_eq!(render(&status, &opts), "claude: HTTP 500: Internal");
        let status = Err("token refresh HTTP 400: invalid_grant".to_string());
        assert_eq!(
            render(&status, &opts),
            "claude: token refresh HTTP 400: invalid_grant"
        );
        let status = Err("a very long single clause with no colon in it at all".to_string());
        assert_eq!(
            render(&status, &opts),
            "claude: a very long single clause with no colon\u{2026}"
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
        assert_eq!(
            render(&usage_api::parse(body), &opts),
            "5h 30% \u{b7} $ 12%"
        );
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
        assert_eq!(
            backoff(Duration::from_secs(3600), 9),
            Duration::from_secs(3600)
        );
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
            stale(&previous, None, &opts),
            "5h 11% \u{b7} wk 39% \u{b7} Sonnet 62% \u{26a0}"
        );
    }

    fn incidents(impact: &str) -> Incidents {
        crate::incidents::parse(&format!(
            r#"{{"incidents":[{{"name":"Elevated errors","status":"investigating",
                "impact":"{impact}"}}]}}"#
        ))
        .unwrap()
    }

    /// One open incident carrying the status page's own deep link to it.
    fn incident(url: Option<&str>) -> crate::incidents::Incident {
        crate::incidents::Incident {
            name: "Outage".to_string(),
            impact: crate::incidents::Impact::Major,
            status: "investigating".to_string(),
            url: url.map(String::from),
        }
    }

    fn incident_linked(shortlink: &str) -> Incidents {
        crate::incidents::parse(&format!(
            r#"{{"incidents":[{{"name":"Elevated errors","status":"investigating",
                "impact":"major","shortlink":"{shortlink}"}}]}}"#
        ))
        .unwrap()
    }

    /// Colours and position, with the click tag out of the way.
    fn quiet() -> Options {
        Options {
            bold_font: 0,
            opener: String::new(),
            ..Options::default()
        }
    }

    #[test]
    fn an_open_incident_is_flagged_in_front() {
        let opts = quiet();
        let status = usage_api::parse(SAMPLE);
        // A major incident takes the critical colour ...
        let line = render_line(&status, Some(&incidents("major")), &opts);
        assert!(
            line.starts_with("%{F#ff5555}\u{26a0}%{F-} %{F#50fa7b}5h 11%"),
            "{line}"
        );
        // ... a minor one the warning colour.
        let line = render_line(&status, Some(&incidents("minor")), &opts);
        assert!(line.starts_with("%{F#f1fa8c}\u{26a0}%{F-} "), "{line}");
    }

    #[test]
    fn clicking_the_marker_opens_that_incident() {
        let opts = Options {
            bold_font: 0,
            ..Options::default()
        };
        let status = usage_api::parse(SAMPLE);
        let open = incident_linked("https://stspg.io/9xz4hh");
        let line = render_line(&status, Some(&open), &opts);
        // The action wraps the marker alone: the tag closes before the numbers.
        assert!(
            line.starts_with(
                "%{A1:xdg-open 'https\\://stspg.io/9xz4hh':}\
                 %{F#ff5555}\u{26a0}%{F-}%{A} %{F#50fa7b}5h 11%"
            ),
            "{line}"
        );
    }

    #[test]
    fn the_scheme_colon_is_escaped_for_polybars_tag_parser() {
        // Unescaped, polybar would end the command at "xdg-open 'https".
        let opts = Options::default();
        let line = render_line(
            &usage_api::parse(SAMPLE),
            Some(&incident_linked("https://stspg.io/x")),
            &opts,
        );
        assert!(line.contains("https\\://stspg.io/x"), "{line}");
        assert!(!line.contains("'https://"), "{line}");
    }

    #[test]
    fn an_incident_with_no_link_falls_back_to_the_front_door() {
        assert_eq!(incident(None).link(), STATUS_PAGE);
        let line = render_line(
            &usage_api::parse(SAMPLE),
            Some(&incidents("major")),
            &Options::default(),
        );
        assert!(
            line.starts_with("%{A1:xdg-open 'https\\://status.claude.com/':}"),
            "{line}"
        );
    }

    #[test]
    fn a_shortlink_that_could_break_out_is_not_passed_on() {
        for hostile in [
            // Closes the quote and appends a command.
            "https://stspg.io/x'; rm -rf ~; echo '",
            // Opens a polybar tag inside the action's own command.
            "https://stspg.io/%{F#ff0000}",
            // Not https, so not the status page.
            "http://stspg.io/x",
            "javascript:alert(1)",
            // Whitespace would split the command; control characters, worse.
            "https://stspg.io/x y",
            "https://stspg.io/x\n",
        ] {
            assert_eq!(incident(Some(hostile)).link(), STATUS_PAGE, "{hostile}");
        }
        // The real shape still gets through.
        let real = "https://stspg.io/9xz4hhmd1jzn";
        assert_eq!(incident(Some(real)).link(), real);
    }

    #[test]
    fn plain_text_carries_no_action_tag() {
        let opts = Options {
            color: false,
            bold_font: 0,
            ..Options::default()
        };
        let line = render_line(
            &usage_api::parse(SAMPLE),
            Some(&incident_linked("https://stspg.io/x")),
            &opts,
        );
        assert_eq!(line, "\u{26a0} 5h 11% \u{b7} wk 39% \u{b7} Sonnet 62%");
    }

    #[test]
    fn an_empty_opener_leaves_the_marker_unclickable() {
        let line = render_line(
            &usage_api::parse(SAMPLE),
            Some(&incident_linked("https://stspg.io/x")),
            &quiet(),
        );
        assert!(!line.contains("%{A"), "{line}");
        assert!(line.starts_with("%{F#ff5555}\u{26a0}%{F-} "), "{line}");
    }

    #[test]
    fn the_stale_marker_is_not_a_click_target() {
        // "this line is old" is not something the status page can answer.
        let previous = usage_api::parse(SAMPLE).unwrap();
        let open = incident_linked("https://stspg.io/x");
        let line = stale(&previous, Some(&open), &Options::default());
        assert_eq!(line.matches("%{A1:").count(), 1, "{line}");
        assert!(line.ends_with("%{F#f1fa8c} \u{26a0}%{F-}"), "{line}");
    }

    #[test]
    fn no_incident_means_no_marker() {
        let opts = Options {
            color: false,
            bold_font: 0,
            ..Options::default()
        };
        let status = usage_api::parse(SAMPLE);
        let expected = "5h 11% \u{b7} wk 39% \u{b7} Sonnet 62%";
        let all_clear = crate::incidents::parse(r#"{"incidents":[]}"#).unwrap();
        assert_eq!(render_line(&status, Some(&all_clear), &opts), expected);
        // Unknown (the page could not be checked) is shown as nothing, too.
        assert_eq!(render_line(&status, None, &opts), expected);
    }

    #[test]
    fn incident_and_stale_markers_sit_at_opposite_ends() {
        let opts = Options {
            color: false,
            bold_font: 0,
            ..Options::default()
        };
        let previous = usage_api::parse(SAMPLE).unwrap();
        assert_eq!(
            stale(&previous, Some(&incidents("major")), &opts),
            "\u{26a0} 5h 11% \u{b7} wk 39% \u{b7} Sonnet 62% \u{26a0}"
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
