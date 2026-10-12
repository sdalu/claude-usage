# Closing a round on claude-usage

What has to be true before a unit of work here is done. Every line is a
command or a question with a yes or a no at the end of it.


## Gates

Run in order. A red gate is answered before anything else.

- [ ] `make check WERROR=yes` -- the release agrees with cargo and the
      tags, the code is rustfmt-clean (`cargo fmt` fixes it), and clippy
      is clean on the code and the tests.
- [ ] `make build` -- the release binary builds.
- [ ] `make tests` -- the suite. A behaviour change ships with the test
      that proves it, and a refusal with the test that feeds it the
      refused input.
- [ ] A changed screen or line is looked at, not only asserted: the
      captures `cargo test render_monitor -- --nocapture`,
      `cargo test open_incident -- --nocapture` and
      `cargo test polybar_line -- --nocapture` still match what the
      README shows -- yes or no.


## Documents

Re-read each against what this round changed: a document that was true
this morning is a claim, not a fact.

- [ ] `README.md` -- does every flag, key, path and command it names still
      exist, and do the flag tables match `src/main.rs`?
- [ ] `docs/design.md` -- did this round decide something, or invalidate a
      reason written there?
- [ ] `docs/usage-api.md` -- did this round learn anything new about the
      endpoints' shape or behaviour?
- [ ] `CLAUDE.md` -- does every pointer resolve, and did this round cost
      time in a way the next session would too?
- [ ] `CLAUDE.local.md` -- did this round change the installed binary or
      the polybar wiring it describes?


## Release

- [ ] Does the number move? It lives in `Cargo.toml` alone; `make version`
      reads it. A new flag or a changed output moves it, a refactor does
      not. Then `make tag`, and push the tag with the branch.


Then capture: whatever this round learned goes to the artifact that owns
that kind of fact, and anything this list failed to ask becomes a line
here.
