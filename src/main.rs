//! claude-usage: a live terminal monitor for Claude plan usage.
//!
//! It shows the percent of each usage window (5-hour session, weekly, and any
//! per-model weekly windows) that has been consumed, fetched live from the same
//! endpoint as `claude /status`. Its only local dependency is the OAuth token
//! in `~/.claude/.credentials.json`; it does not read the Claude CLI session
//! logs or any other CLI byproduct.

mod cache;
mod incidents;
mod json;
mod polybar;
mod rotation;
mod tui;
mod usage_api;
mod views;

const USAGE: &str = "\
Usage: claude-usage [-1 [--no-border]] [--polybar [options]] [--json]
                    [--no-incidents] [--write-back] [--check-rotation] [--help]

A live monitor of your Claude plan usage windows (percent of allowance used),
fetched from https://api.anthropic.com/api/oauth/usage using the OAuth token in
~/.claude/.credentials.json. An open incident on https://status.claude.com is
called out beside the numbers, so an outage is not mistaken for your quota.

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
  --no-incidents
               do not consult the status page: no incident marker, and no
               request to status.claude.com
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

/// What the command line selected. Parsing records the choices and applies
/// nothing, so it can be tested without touching the network, the credentials
/// file or the process-wide toggles.
#[derive(Default)]
struct Config {
    json_mode: bool,
    oneshot: bool,
    no_border: bool,
    polybar_mode: bool,
    polybar_opts: polybar::Options,
    polybar_interval: Option<u64>,
    cache_ttl: Option<u64>,
    cache: bool,
    incidents: bool,
    write_back: bool,
}

/// A rejected command line: what to say, and whether the usage text follows.
/// Misuse of an option prints the usage; a bad *value* for a valid option
/// speaks for itself.
struct Refusal {
    message: String,
    show_usage: bool,
}

impl Refusal {
    fn new(message: impl Into<String>) -> Refusal {
        Refusal {
            message: message.into(),
            show_usage: false,
        }
    }

    fn with_usage(message: impl Into<String>) -> Refusal {
        Refusal {
            message: message.into(),
            show_usage: true,
        }
    }
}

/// The modes that exit before anything is fetched, kept as parse outcomes so
/// they stop the scan exactly where the flag appeared — `--check-rotation` and
/// `--help` ignore whatever follows them.
enum Parsed {
    Run(Box<Config>),
    Help,
    CheckRotation,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Parsed, Refusal> {
    let mut config = Config {
        cache: true,
        incidents: true,
        ..Config::default()
    };
    // Set by any option that only means something for --polybar, so using one
    // without it is caught rather than silently ignored.
    let mut polybar_tuning = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // Value-taking options accept both "--opt V" and "--opt=V".
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, Refusal> {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| Refusal::new(format!("{name} needs a value")))
        };
        match flag.as_str() {
            "--json" => config.json_mode = true,
            "-1" => config.oneshot = true,
            "--no-border" => config.no_border = true,
            "--polybar" => config.polybar_mode = true,
            "--no-color" => {
                config.polybar_opts.color = false;
                polybar_tuning = true;
            }
            "--bold-font" => {
                let raw = value("--bold-font")?;
                polybar_tuning = true;
                config.polybar_opts.bold_font = raw.parse::<u8>().map_err(|_| {
                    Refusal::new(format!("--bold-font wants a font slot number: {raw}"))
                })?;
            }
            "--interval" => {
                let raw = value("--interval")?;
                polybar_tuning = true;
                let secs = raw.parse::<u64>().map_err(|_| {
                    Refusal::new(format!("--interval wants a number of seconds: {raw}"))
                })?;
                if secs < polybar::MIN_INTERVAL {
                    return Err(Refusal::new(format!(
                        "--interval {secs} is below the {}s minimum \
                         (the usage API is rate limited)",
                        polybar::MIN_INTERVAL
                    )));
                }
                config.polybar_interval = Some(secs);
            }
            "--colors" => {
                let raw = value("--colors")?;
                polybar_tuning = true;
                config.polybar_opts.palette = polybar::Palette::parse(&raw)
                    .map_err(|e| Refusal::new(format!("--colors: {e}")))?;
            }
            "--cache-ttl" => {
                let raw = value("--cache-ttl")?;
                config.cache_ttl = Some(raw.parse::<u64>().map_err(|_| {
                    Refusal::new(format!("--cache-ttl wants a number of seconds: {raw}"))
                })?);
            }
            "--no-cache" => config.cache = false,
            "--no-incidents" => config.incidents = false,
            "--write-back" => config.write_back = true,
            // Standalone diagnostic: runs and exits, ignoring the other flags.
            "--check-rotation" => return Ok(Parsed::CheckRotation),
            "-h" | "--help" => return Ok(Parsed::Help),
            other => {
                return Err(Refusal::with_usage(format!("unknown option: {other}")));
            }
        }
    }

    if config.no_border && !config.oneshot {
        return Err(Refusal::with_usage("--no-border is only valid with -1"));
    }

    if polybar_tuning && !config.polybar_mode {
        return Err(Refusal::with_usage(
            "--no-color, --bold-font, --colors and --interval are only valid with --polybar",
        ));
    }

    Ok(Parsed::Run(Box::new(config)))
}

fn real_main() -> i32 {
    let config = match parse_args(std::env::args().skip(1)) {
        Ok(Parsed::Run(config)) => config,
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            return 0;
        }
        Ok(Parsed::CheckRotation) => return rotation::run(),
        Err(refusal) => {
            eprintln!("claude-usage: {}", refusal.message);
            if refusal.show_usage {
                eprintln!("{USAGE}");
            }
            return 1;
        }
    };

    let Config {
        json_mode,
        oneshot,
        no_border,
        polybar_mode,
        polybar_opts,
        polybar_interval,
        cache_ttl,
        cache: cache_enabled,
        incidents: incidents_enabled,
        write_back,
    } = *config;

    usage_api::set_write_back(write_back);
    cache::set_enabled(cache_enabled);
    incidents::set_enabled(incidents_enabled);

    // Tail mode owns its own refresh cadence (and its backoff), so it fetches
    // inside the loop rather than sharing the one-shot fetch below.
    if let Some(secs) = polybar_interval {
        let interval = std::time::Duration::from_secs(secs);
        // Default the shared-cache TTL to just under the interval, so the
        // instances on the other monitors ride on one instance's fetch.
        let ttl = cache_ttl
            .map(std::time::Duration::from_secs)
            .unwrap_or_else(|| interval.mul_f64(0.9));
        return polybar::run_tail(interval, &polybar_opts, || {
            (cache::fetch(ttl), cache::fetch_incidents(ttl))
        });
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
    // The status-page check rides on the same TTL; unknown (skipped or failed)
    // is None and shows as nothing.
    let incidents = cache::fetch_incidents(ttl);

    if json_mode {
        match serde_json::to_string_pretty(&json::build(&status, incidents.as_ref())) {
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
        println!(
            "{}",
            polybar::render_line(&status, incidents.as_ref(), &polybar_opts)
        );
        return 0;
    }

    // Plan/tier is account info; fetch it once (the refresh loop only re-hits
    // the usage endpoint).
    let plan = usage_api::fetch_plan();

    // Single-shot: render one frame inline and quit (no "?" flash, no input).
    if oneshot {
        let mut app = tui::App::new(status, plan)
            .with_incidents(incidents)
            .without_intro();
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

    let app = tui::App::new(status, plan).with_incidents(incidents);
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal);
    ratatui::restore();

    if let Err(e) = result {
        eprintln!("claude-usage: {e}");
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, Refusal> {
        parse_args(args.iter().map(|a| a.to_string()))
    }

    fn config(args: &[&str]) -> Config {
        match parse(args) {
            Ok(Parsed::Run(config)) => *config,
            _ => panic!("expected a runnable config for {args:?}"),
        }
    }

    fn refusal(args: &[&str]) -> Refusal {
        match parse(args) {
            Err(refusal) => refusal,
            _ => panic!("expected {args:?} to be refused"),
        }
    }

    #[test]
    fn defaults_are_the_live_monitor() {
        let config = config(&[]);
        assert!(!config.json_mode);
        assert!(!config.oneshot);
        assert!(!config.polybar_mode);
        assert!(!config.write_back);
        // Sharing and the status-page check are on unless opted out of.
        assert!(config.cache);
        assert!(config.incidents);
        assert_eq!(config.cache_ttl, None);
    }

    #[test]
    fn options_take_their_value_in_either_form() {
        for args in [
            ["--polybar", "--interval", "300"],
            ["--polybar", "--interval=300", ""],
        ] {
            let config = config(&args.into_iter().filter(|a| !a.is_empty()).collect::<Vec<_>>());
            assert_eq!(config.polybar_interval, Some(300));
        }
        assert_eq!(config(&["--cache-ttl=90"]).cache_ttl, Some(90));
        assert_eq!(
            config(&["--polybar", "--colors=#111111,,#333333"])
                .polybar_opts
                .palette
                .ok,
            "#111111"
        );
    }

    #[test]
    fn polybar_options_are_recorded() {
        let config = config(&["--polybar", "--no-color", "--bold-font", "0"]);
        assert!(config.polybar_mode);
        assert!(!config.polybar_opts.color);
        assert_eq!(config.polybar_opts.bold_font, 0);
    }

    #[test]
    fn toggles_are_recorded_in_the_config() {
        // Recorded here, applied by real_main — the process-wide toggles they
        // drive are not asserted on, since other tests share them.
        assert!(config(&["--write-back"]).write_back);
        assert!(!config(&["--no-cache"]).cache);
        assert!(!config(&["--no-incidents"]).incidents);
    }

    #[test]
    fn no_border_is_refused_without_oneshot() {
        let refusal = refusal(&["--no-border"]);
        assert!(refusal.message.contains("only valid with -1"));
        assert!(refusal.show_usage);
        // ... and accepted with it.
        assert!(config(&["-1", "--no-border"]).no_border);
    }

    #[test]
    fn polybar_only_options_are_refused_on_their_own() {
        for args in [
            vec!["--no-color"],
            vec!["--bold-font", "5"],
            vec!["--colors", "#111111,#222222,#333333"],
            vec!["--interval", "300"],
        ] {
            let refusal = refusal(&args);
            assert!(
                refusal.message.contains("only valid with --polybar"),
                "{args:?} refused with: {}",
                refusal.message
            );
            assert!(refusal.show_usage);
        }
    }

    #[test]
    fn the_interval_floor_is_enforced() {
        let refusal = refusal(&["--polybar", "--interval", "5"]);
        assert!(refusal.message.contains("below the 30s minimum"));
        // The floor itself is fine.
        assert_eq!(
            config(&["--polybar", "--interval", "30"]).polybar_interval,
            Some(30)
        );
    }

    #[test]
    fn bad_values_are_refused_with_the_value_quoted() {
        for (args, expected) in [
            (vec!["--polybar", "--interval", "abc"], "number of seconds"),
            (vec!["--polybar", "--bold-font", "x"], "font slot number"),
            (vec!["--cache-ttl", "soon"], "number of seconds"),
            (vec!["--polybar", "--colors", "red,#222222,#333333"], "#rrggbb"),
        ] {
            let refusal = refusal(&args);
            assert!(
                refusal.message.contains(expected),
                "{args:?} refused with: {}",
                refusal.message
            );
            // A bad value speaks for itself; no usage dump.
            assert!(!refusal.show_usage);
        }
    }

    #[test]
    fn a_missing_value_is_refused() {
        assert!(refusal(&["--polybar", "--interval"])
            .message
            .contains("needs a value"));
    }

    #[test]
    fn unknown_options_are_refused() {
        let refusal = refusal(&["--bogus"]);
        assert!(refusal.message.contains("unknown option: --bogus"));
        assert!(refusal.show_usage);
    }

    #[test]
    fn early_exit_modes_stop_the_scan() {
        // Both are documented as ignoring whatever follows them.
        assert!(matches!(
            parse(&["--check-rotation", "--bogus"]),
            Ok(Parsed::CheckRotation)
        ));
        assert!(matches!(parse(&["--help", "--bogus"]), Ok(Parsed::Help)));
        assert!(matches!(parse(&["-h"]), Ok(Parsed::Help)));
        // But an error before them still wins.
        assert!(parse(&["--bogus", "--help"]).is_err());
    }
}
