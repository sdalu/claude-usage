#!/bin/sh
# check-version.sh — every reader of the release agrees, and so do the tags
# usage: check-version.sh <release>     (the Makefile's reading of Cargo.toml)
set -eu

# Cargo.toml is the one place the release is written. The Makefile reads it
# with sed; cargo is the reader that matters -- it is what the binary and
# the crate report -- so its answer is the one compared against. Then the
# tags: scripts/checktag.sh holds the release to the nearest v* tag.

want=${1:-}
if [ -z "$want" ]; then
    printf 'usage: check-version.sh <release>\n' >&2
    exit 2
fi

top=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cargo=${CARGO:-cargo}

# path+file:///.../claude-usage#1.0.0, or ...#claude-usage@1.0.0 when the
# package name differs from the directory.
got=$(cd "$top" && "$cargo" pkgid) || exit 1
got=${got##*[#@]}

if [ "$got" != "$want" ]; then
    printf '  version: the Makefile reads %s from Cargo.toml, cargo says %s\n' \
        "$want" "$got"
    exit 1
fi

sh "$top/scripts/checktag.sh" "$want"
