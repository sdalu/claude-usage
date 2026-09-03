# Design notes

Why claude-usage is shaped the way it is. The README says what it does;
`docs/usage-api.md` documents the endpoints it talks to. This file records the
decisions and the alternatives rejected, so a later change can tell a
constraint from an accident.

## One data source, no local scraping

Everything shown comes from `GET /api/oauth/usage` — the same call behind
`claude /status`. The Claude CLI also leaves session logs and other byproducts
on disk, and a usage tool could add them up itself. It doesn't: those files are
undocumented, they change shape without notice, and a local sum would disagree
with the number Anthropic actually enforces. The only local dependency is the
OAuth token, because the request needs it.

The cost is that there is no history — the endpoint reports current utilization
only, so the tool cannot chart a week.

## Credentials are read-only by default

`~/.claude/.credentials.json` belongs to Claude Code. When the access token has
expired the default is to say so and stop, rather than refresh it: if Anthropic
rotates the refresh token, two processes refreshing at once can log the loser
out, and the tool losing that race would be the user's editor.

`--write-back` opts into refreshing and persisting, for people running the
monitor without the CLI. `--check-rotation` exists to answer whether that is
safe on a given account, without a network call: it fingerprints the refresh
token (never storing it) and reports, after a refresh has happened, whether the
token changed.

## One layout, two renderers

The TUI and the `-1` single-shot output are the same ratatui code; single-shot
draws into a `TestBackend` and converts the buffer to ANSI. A second layout
implementation for inline output would have drifted from the live one, which is
also what the screenshot in the README is generated from.

The polybar line is a genuinely different renderer — a bar has one line, no
alignment columns, and its own markup — so it is separate. What the two must
agree on is the severity colour, so the threshold rule lives once in
`views::level` and both call it.

## Status-bar output is a line, not JSON

`--json` already existed, and a bar module could have been a shell wrapper
piping it through `jq`. `--polybar` emits the finished line instead, because
every user of the JSON would re-implement the same three things — short window
names, the severity-to-colour mapping, and the format tags — and get the colour
rule subtly different from the gauges.

Consequences of that choice:

- **Bold via a font slot.** Polybar has no bold tag; a bar's fonts are numbered
  and `%{T2}` selects `font-1`, which configs conventionally set to a bold face.
  Hence `--bold-font N`, and `0` to opt out, rather than a hardcoded tag.
- **Exit status 0 on failure.** The error is reported *as the line*
  (`claude: Access token expired`, trimmed to its first clause to stay
  bar-sized). A non-zero exit would tell polybar the script is broken; the bar
  showing why is more useful than the bar showing nothing.
- **Text from the API is sanitised.** A model display name is interpolated into
  a line polybar parses for markup, so `%{` is neutralised before it goes out.

## Tail mode owns the cadence

A `custom/script` module can be driven by polybar's own `interval`, and that
form still works. `--interval N` (one long-lived process, `tail = true`) is the
recommended one because the endpoint is rate limited and polybar's scheduler
cannot react to that: it re-runs the script on a fixed period whatever the API
answered. Owning the loop buys the two behaviours that matter — a doubling
backoff after each consecutive failure (capped at 30 minutes), and keeping the
last good line on the bar with a stale marker instead of replacing the numbers
with an error.

The floor of 30 seconds on `--interval` is deliberate: the value is typed by
hand into a config, and a typo there becomes a permanent hammer on a shared
rate limit.

## The shared cache

The natural setup runs several instances: polybar starts a module process per
monitor, and a monitor opened alongside is one more. Each would poll
independently. They share instead, through `~/.cache/claude-usage/usage.json`
plus a lock file.

**Alternatives rejected.** A daemon that others query is a lifecycle to
supervise — start, restart, socket, staleness on crash — for a tool whose whole
job is one HTTP call. Shared memory has no dependency-free portable form here,
and would lose the state on the last instance exiting, which is exactly when
the next polybar restart needs it. Polybar's own IPC could dedupe the bars but
not the TUI, and would tie the design to one bar. A file plus `O_CREAT|O_EXCL`
needs no dependency, survives restarts, and works between unrelated processes
and modes.

**Why the entry holds the raw response.** Parsing before publishing would mean
two parse paths — the fetcher's and the reader's — that can disagree after a
change to the API shape. Storing the body means every instance runs the same
`usage_api::parse`.

**Why the fetch time is stored with it.** A reader must not present a shared
result as its own fresh one; the monitor's `updated` banner shows when the data
was fetched, by whichever instance fetched it.

**Why failures are cached too.** Three processes independently discovering the
same 429 and independently re-trying it is the stampede the cache exists to
prevent. An error is shared like a success.

**Why the lock loser prefers a stale entry.** It could wait for the winner's
result, but a bar showing numbers a few minutes old immediately beats a bar
blocked on the network. It only waits when there is nothing published at all
(a cold start, where all instances launch together), and even then it gives up
after 8 seconds and fetches for itself rather than hanging.

**Staleness thresholds.** A lock is broken after 30 seconds — longer than the
HTTP timeouts (5s connect + 6s read), so a live fetcher is never robbed, short
enough that a `SIGKILL`ed instance does not wedge the others. The default TTL
is just under the caller's own interval, so the first instance to tick fetches
and the rest ride on that entry for the remainder of the window.

The cache is an optimisation, never a dependency: an unusable cache directory
falls through to a direct fetch, and `--no-cache` turns sharing off entirely.

## Testing the refusals

Parsing is a pure function (`parse_args` → a `Config`, a `Refusal`, or one of
the early-exit modes) precisely so the refusals can be tested: the flags it
turns down — `--no-border` without `-1`, the polybar-only options on their own,
an `--interval` under the floor, unparseable values — are guarantees the README
makes, and a guard nobody has watched fail proves nothing. Applying what was
parsed (the write-back and cache toggles, which are process-wide) stays in
`real_main`, so a parse in a test changes nothing outside it.

## Known shortfalls

- The lock path is proven by unit tests driving `cache::shared` with a fake
  fetcher, not by observing concurrent processes; there is no test that N real
  instances cold-starting make exactly one request.
- In tail mode a closed stdout is noticed at the next write, up to one interval
  later. Polybar signals the process, so this shows up only when testing by
  hand (piping into `head` looks like a hang).
- No history: see "One data source" above.
