//! claude-usage: a live terminal monitor for Claude plan usage.
//!
//! It shows the percent of each usage window (5-hour session, weekly, and any
//! per-model weekly windows) that has been consumed, fetched live from the same
//! endpoint as `claude /status`. Its only local dependency is the OAuth token
//! in `~/.claude/.credentials.json`; it does not read the Claude CLI session
//! logs or any other CLI byproduct.

mod cache;
mod json;
mod polybar;
mod rotation;
mod tui;
mod usage_api;
mod views;

const USAGE: &str = "\
Usage: claude-usage [-1 [--no-border]] [--polybar [options]] [--json]
                    [--write-back] [--check-rotation] [--help]

A live monitor of your Claude plan usage windows (percent of allowance used),
fetched from https://api.anthropic.com/api/oauth/usage using the OAuth token in
~/.claude/.credentials.json.

Options:
  -1           render the dashboard once and exit (no live updates)
  --no-border  with -1, drop the box border (just the gauge rows)
  --polybar    print one status-bar line and exit: a compact segment per usage
               window, coloured with polybar format tags. For a custom/script
               module; add --interval to keep running instead (tail = true).
  --json       print the usage windows as JSON and exit (no UI)
  --no-cache   do not share fetches with other claude-usage instances
  --cache-ttl N
               reuse another instance's result if it is younger than N
               seconds (default: just under --interval, or 240s for a
               one-shot --polybar; other modes fetch fresh)
  --write-back refresh an expired OAuth token and write the rotated
               credentials back to ~/.claude/.credentials.json. Default
               is read-only: an expired token is reported, never rewritten.
  --check-rotation
               passively check whether the OAuth refresh token rotates
               (read-only; no network). Run once to baseline, then again
               after Claude Code refreshes.
  -h, --help   show this help

Polybar options (only with --polybar):
  --interval N run as a long-lived process, printing a line every N seconds
               (minimum 30) for a `tail = true` module. Preferred over
               polybar's own interval: a failed fetch doubles the wait,
               and the last good line stays on the bar, marked stale.
  --no-color   plain text, no format tags (also suits waybar, i3blocks, tmux)
  --bold-font N
               font slot for the binding window, matching polybar's font-N
               minus one (default 2, i.e. font-1); 0 leaves it unemphasised
  --colors OK,WARN,CRIT
               severity colours as #rrggbb (default #50fa7b,#f1fa8c,#ff5555);
               an empty entry keeps that default

Keys (interactive UI):
  q / Esc / Ctrl-C   quit
  r / Enter          refresh now
  t                  toggle auto-refresh (5m / 1m)
  u                  toggle the updated banner
  ?                  toggle the key help overlay

The monitor auto-refreshes every 5 minutes (every minute after pressing t).";

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut json_mode = false;
    let mut oneshot = false;
    let mut no_border = false;
    let mut polybar_mode = false;
    let mut polybar_opts = polybar::Options::default();
    let mut polybar_tuning = false;
    let mut polybar_interval: Option<u64> = None;
    let mut cache_ttl: Option<u64> = None;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // Value-taking options accept both "--opt V" and "--opt=V".
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, i32> {
            inline.clone().or_else(|| args.next()).ok_or_else(|| {
                eprintln!("claude-usage: {name} needs a value");
                1
            })
        };
        match flag.as_str() {
            "--json" => json_mode = true,
            "-1" => oneshot = true,
            "--no-border" => no_border = true,
            "--polybar" => polybar_mode = true,
            "--no-color" => {
                polybar_opts.color = false;
                polybar_tuning = true;
            }
            "--bold-font" => {
                let raw = match value("--bold-font") {
                    Ok(v) => v,
                    Err(code) => return code,
                };
                polybar_tuning = true;
                match raw.parse::<u8>() {
                    Ok(slot) => polybar_opts.bold_font = slot,
                    Err(_) => {
                        eprintln!("claude-usage: --bold-font wants a font slot number: {raw}");
                        return 1;
                    }
                }
            }
            "--interval" => {
                let raw = match value("--interval") {
                    Ok(v) => v,
                    Err(code) => return code,
                };
                polybar_tuning = true;
                match raw.parse::<u64>() {
                    Ok(secs) if secs >= polybar::MIN_INTERVAL => polybar_interval = Some(secs),
                    Ok(secs) => {
                        eprintln!(
                            "claude-usage: --interval {secs} is below the {}s minimum \
                             (the usage API is rate limited)",
                            polybar::MIN_INTERVAL
                        );
                        return 1;
                    }
                    Err(_) => {
                        eprintln!("claude-usage: --interval wants a number of seconds: {raw}");
                        return 1;
                    }
                }
            }
            "--colors" => {
                let raw = match value("--colors") {
                    Ok(v) => v,
                    Err(code) => return code,
                };
                polybar_tuning = true;
                match polybar::Palette::parse(&raw) {
                    Ok(palette) => polybar_opts.palette = palette,
                    Err(e) => {
                        eprintln!("claude-usage: --colors: {e}");
                        return 1;
                    }
                }
            }
            "--cache-ttl" => {
                let raw = match value("--cache-ttl") {
                    Ok(v) => v,
                    Err(code) => return code,
                };
                match raw.parse::<u64>() {
                    Ok(secs) => cache_ttl = Some(secs),
                    Err(_) => {
                        eprintln!("claude-usage: --cache-ttl wants a number of seconds: {raw}");
                        return 1;
                    }
                }
            }
            "--no-cache" => cache::set_enabled(false),
            "--write-back" => usage_api::set_write_back(true),
            // Standalone diagnostic: runs and exits, ignoring the other flags.
            "--check-rotation" => return rotation::run(),
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("claude-usage: unknown option: {other}");
                eprintln!("{USAGE}");
                return 1;
            }
        }
    }

    if no_border && !oneshot {
        eprintln!("claude-usage: --no-border is only valid with -1");
        eprintln!("{USAGE}");
        return 1;
    }

    if polybar_tuning && !polybar_mode {
        eprintln!(
            "claude-usage: --no-color, --bold-font, --colors and --interval are only valid \
             with --polybar"
        );
        eprintln!("{USAGE}");
        return 1;
    }

    // Tail mode owns its own refresh cadence (and its backoff), so it fetches
    // inside the loop rather than sharing the one-shot fetch below.
    if let Some(secs) = polybar_interval {
        let interval = std::time::Duration::from_secs(secs);
        // Default the shared-cache TTL to just under the interval, so the
        // instances on the other monitors ride on one instance's fetch.
        let ttl = cache_ttl
            .map(std::time::Duration::from_secs)
            .unwrap_or_else(|| interval.mul_f64(0.9));
        return polybar::run_tail(interval, &polybar_opts, || cache::fetch(ttl));
    }

    // One-shot polybar reads through the shared cache (its default TTL covers
    // the usual polybar interval); every other mode fetches fresh — and
    // publishes what it gets, so the bars can ride on it.
    let ttl = match (polybar_mode, cache_ttl) {
        (_, Some(secs)) => std::time::Duration::from_secs(secs),
        (true, None) => polybar::DEFAULT_CACHE_TTL,
        (false, None) => std::time::Duration::ZERO,
    };
    let status = cache::fetch(ttl);

    if json_mode {
        match serde_json::to_string_pretty(&json::build(&status)) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("claude-usage: {e}");
                return 1;
            }
        }
        // A failed fetch is reported in the JSON body; still exit non-zero so
        // scripts can detect it.
        return if status.is_err() { 1 } else { 0 };
    }

    // Status-bar line: the error, if any, is reported in the line itself, and
    // the exit status stays 0 so polybar keeps rendering the module.
    if polybar_mode {
        println!("{}", polybar::render(&status, &polybar_opts));
        return 0;
    }

    // Plan/tier is account info; fetch it once (the refresh loop only re-hits
    // the usage endpoint).
    let plan = usage_api::fetch_plan();

    // Single-shot: render one frame inline and quit (no "?" flash, no input).
    if oneshot {
        let mut app = tui::App::new(status, plan).without_intro();
        if no_border {
            app = app.without_border();
        }
        return match app.render_once() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("claude-usage: {e}");
                1
            }
        };
    }

    let app = tui::App::new(status, plan);
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();

    if let Err(e) = result {
        eprintln!("claude-usage: {e}");
        return 1;
    }
    0
}
