#!/usr/bin/env bash
# licenses.sh -- `chore lint`: every crate in the dependency graph carries a
# permissive licence (deny.toml names them). An unknown or copyleft licence
# anywhere in the graph fails.
#
# Needs cargo-deny. It is not installed here: `cargo install cargo-deny
# --locked`; CI installs a prebuilt binary (see ci.yml's lint job).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
command -v cargo-deny >/dev/null 2>&1 || {
    echo "licenses: cargo-deny is not installed: cargo install cargo-deny --locked" >&2
    exit 1
}
exec cargo deny check licenses
