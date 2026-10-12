# claude-usage -- a terminal monitor for Claude plan usage limits
#
# A thin driver around cargo, portable between GNU make and BSD make: no
# conditionals, no pattern rules, no GNU-only functions. Run `make help`
# for the targets and the variables you can override.

NAME       = claude-usage

# The release, from the one file that holds it. tests/check-version.sh
# holds this reading to cargo's own, and to the tags.
VERSION   != sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -n 1
GITVER    != sh scripts/gitversion.sh

CARGO     ?= cargo
# Opt-in rather than default: a newer clippy inventing a lint should not
# fail an ordinary check, but CI and a tag should stay clean.
WERROR    ?= no
W_yes      = -- -D warnings
W_no       =

PREFIX    ?= /usr/local
BINDIR    ?= $(PREFIX)/bin
DESTDIR   ?=
INSTALL   ?= install
YES       ?=

RELEASE    = target/release/$(NAME)

# First target, so a bare `make` runs it.
help:						## show this help (the default)
	@echo '$(NAME) -- a terminal monitor for Claude plan usage limits'
	@echo ''
	@echo 'Targets:'
	@awk -F':.*## ' '/^[a-z][a-z0-9-]*:.*## /{ \
	    pre = sprintf("  %-16s ", $$1); n = split($$2, w, / /); line = ""; \
	    for (i = 1; i <= n; i++) { \
	        cand = (line == "" ? w[i] : line " " w[i]); \
	        if (length(pre) + length(cand) > 80 && line != "") { \
	            print pre line; pre = "                   "; line = w[i]; \
	        } else line = cand; \
	    } \
	    if (line != "") print pre line; \
	}' Makefile
	@echo ''
	@echo 'Variables (current value):'
	@printf '  %-16s %s\n' \
	    CARGO    '$(CARGO)' \
	    WERROR   '$(WERROR)  (yes fails check-lint on any clippy warning)' \
	    PREFIX   '$(PREFIX)' \
	    BINDIR   '$(BINDIR)' \
	    DESTDIR  '$(DESTDIR)  (staging prefix for packaging)' \
	    YES      '$(YES)  (1 answers yes to `make tag`)'
	@echo ''
	@echo 'There is no all target: build is everything this project ships.'
	@echo 'This Makefile works with both GNU make and BSD make.'

# --- preflight --------------------------------------------------------
# check runs none of this project's code. rustfmt only parses, clippy
# compiles but runs nothing; the version check asks cargo and git, not the
# binary.

check: check-version check-fmt check-lint		## preflight: this tree is fit to build

check-version:					## Cargo.toml, cargo and the v* tags agree on the release
	@CARGO='$(CARGO)' sh tests/check-version.sh '$(VERSION)'

check-fmt:					## the code is as rustfmt would write it (cargo fmt fixes it)
	$(CARGO) fmt --check

check-lint:					## clippy is clean on the code and the tests (WERROR=yes to enforce)
	$(CARGO) clippy --all-targets $(W_$(WERROR))

# --- building and the suite -------------------------------------------

build:						## build the release binary (target/release/claude-usage)
	$(CARGO) build --release

# The tests never touch the network: the cache tests drive the fetch
# through a fake, everything else parses fixtures.
tests:						## build and run the suite, and report
	$(CARGO) test

# --- installing -------------------------------------------------------
# A plain copy under PREFIX, staged through DESTDIR, rather than
# `cargo install`, which writes its own bookkeeping beside the binary and
# knows nothing of DESTDIR.

install: build					## install the binary into BINDIR
	$(INSTALL) -d '$(DESTDIR)$(BINDIR)'
	$(INSTALL) -m 755 $(RELEASE) '$(DESTDIR)$(BINDIR)/$(NAME)'

uninstall:					## remove what install put down
	rm -f '$(DESTDIR)$(BINDIR)/$(NAME)'

# --- the release ------------------------------------------------------

# Bare, so a script can use it:  v=`make -s version`
version:					## print the release version
	@echo '$(VERSION)'

version-full:					## print the version this tree builds as
	@echo '$(VERSION)$(GITVER)'

tag:						## tag this release, from Cargo.toml (YES=1 skips the question)
	@if [ -n "`git status --porcelain --untracked-files=no`" ]; then \
	    echo 'make: uncommitted changes; commit them before tagging' >&2; \
	    exit 1; fi
	@if git rev-parse -q --verify 'refs/tags/v$(VERSION)' >/dev/null; then \
	    echo 'make: v$(VERSION) exists already; bump Cargo.toml first' >&2; \
	    exit 1; fi
	@if [ "$(YES)" != 1 ]; then \
	    printf 'tag v%s at %s? (the full check runs first) [y/N] ' \
		'$(VERSION)' "`git rev-parse --short HEAD`"; \
	    read -r ans || ans=; \
	    case "$$ans" in \
		y|Y|yes|YES) ;; \
		*) echo 'make: not tagged'; exit 1 ;; \
	    esac; \
	fi
	@$(MAKE) check WERROR=yes && $(MAKE) tests
	git tag -a -m '$(NAME) $(VERSION)' 'v$(VERSION)'
	@sh scripts/checktag.sh '$(VERSION)'
	@echo 'tagged v$(VERSION) -- push it with: git push origin v$(VERSION)'

# --- housekeeping -----------------------------------------------------

clean:						## remove build products (cargo clean)
	$(CARGO) clean

.PHONY: help check check-version check-fmt check-lint build tests install uninstall \
	version version-full tag clean
