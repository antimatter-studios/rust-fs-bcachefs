#!/usr/bin/env bash
# An outside project can depend on this crate by git alone (#706, API-01).
#
#   bash scripts/check-external-consumer.sh
#
# The tracked files of HEAD, and nothing else of this checkout, are committed
# to a new repository. A new consumer crate, outside this checkout, depends on
# it by git. cargo must resolve and build it with rust-fs-core from the
# registry: a sibling path, an untracked override or a committed [patch] that
# the build leaned on would fail here.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
work="$(mktemp -d "${TMPDIR:-/tmp}/external-consumer.XXXXXX")"
trap 'rm -rf -- "${work:?}"' EXIT

mkdir -p "$work/published" "$work/consumer/src"
git -C "$repo" archive HEAD | tar -x -C "$work/published"
git -C "$work/published" init -q -b main
git -C "$work/published" add -A
git -C "$work/published" -c user.name=ci -c user.email=ci@example.invalid \
    -c core.hooksPath=/dev/null commit -q -m published

cat >"$work/consumer/Cargo.toml" <<TOML
[package]
name = "external-consumer"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
rust-fs-bcachefs = { git = "file://$work/published", branch = "main", features = ["write"] }

[workspace]
TOML
cat >"$work/consumer/src/lib.rs" <<'RS'
pub use fs_bcachefs::Filesystem;
RS

cd "$work/consumer"
export CARGO_TARGET_DIR="$work/target"
cargo build --quiet
cargo metadata --format-version 1 --locked >"$work/metadata.json"
python3 - "$work/metadata.json" <<'PY'
import json
import sys

packages = {p["name"]: p for p in json.load(open(sys.argv[1]))["packages"]}
core = packages["rust-fs-core"]["source"] or "a path"
if not core.startswith("registry+"):
    raise SystemExit(f"check-external-consumer: rust-fs-core came from {core}, not the registry")
local = sorted(name for name, p in packages.items() if p["source"] is None)
if local != ["external-consumer"]:
    raise SystemExit(f"check-external-consumer: packages taken from a path: {local}")
version = packages["rust-fs-core"]["version"]
print(f"check-external-consumer: builds by git, with rust-fs-core {version} from the registry")
PY
