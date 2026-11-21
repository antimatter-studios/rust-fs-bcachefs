#!/usr/bin/env bash
# The staticlib task copies the artifact Cargo actually built, regardless of
# CARGO_TARGET_DIR or a configured build.target-dir. A stale default artifact
# must never hide a redirected or failed build.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
sandbox="$(mktemp -d)"
trap 'rm -rf -- "$sandbox"' EXIT
project="$sandbox/project"
mkdir -p "$project/include" "$sandbox/rust-fs-core/include" "$sandbox/bin"
cp "$repo/chores.yml" "$project/chores.yml"
cp "$repo/include/fs_bcachefs.h" "$project/include/fs_bcachefs.h"
cp "$repo/LICENSE" "$repo/THIRD-PARTY-LICENSES.md" "$project/"
cp "$repo/../rust-fs-core/include/fs_core.h" "$sandbox/rust-fs-core/include/fs_core.h"

cat > "$sandbox/bin/rustup" <<'STUB'
#!/usr/bin/env sh
exit 0
STUB
cat > "$sandbox/bin/cargo" <<'STUB'
#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys
import tomllib

cwd = Path.cwd()
configured = cwd / '.cargo' / 'config.toml'
if 'CARGO_TARGET_DIR' in os.environ:
    target = Path(os.environ['CARGO_TARGET_DIR'])
elif configured.exists():
    target = Path(tomllib.loads(configured.read_text()).get('build', {}).get('target-dir', 'target'))
else:
    target = Path('target')
if not target.is_absolute():
    target = cwd / target
if sys.argv[1:2] == ['metadata']:
    print(json.dumps({'target_directory': str(target.resolve())}))
elif sys.argv[1:2] == ['build']:
    status = int(os.environ.get('STATICLIB_STUB_BUILD_STATUS', '0'))
    if status:
        sys.exit(status)
    target_triple = sys.argv[sys.argv.index('--target') + 1]
    artifact = target / target_triple / 'release' / 'libfs_bcachefs.a'
    artifact.parent.mkdir(parents=True, exist_ok=True)
    artifact.write_text('fresh:' + os.environ['STATICLIB_CASE'])
else:
    raise SystemExit('unexpected cargo invocation: ' + repr(sys.argv[1:]))
STUB
chmod +x "$sandbox/bin/rustup" "$sandbox/bin/cargo"

run_case() {
    local name="$1" target_dir="$2" config="$3"
    rm -rf "$project/target" "$project/absolute-target" "$project/relative-target" \
        "$project/configured-target" "$project/.cargo" "$project/dist"
    mkdir -p "$project/target/aarch64-apple-darwin/release"
    printf 'stale-default:%s' "$name" > "$project/target/aarch64-apple-darwin/release/libfs_bcachefs.a"
    if [ -n "$config" ]; then
        mkdir -p "$project/.cargo"
        printf '[build]\ntarget-dir = "%s"\n' "$config" > "$project/.cargo/config.toml"
    fi
    export STATICLIB_CASE="$name"
    unset STATICLIB_STUB_BUILD_STATUS
    if [ "$target_dir" = "unset" ]; then
        unset CARGO_TARGET_DIR
    else
        export CARGO_TARGET_DIR="$target_dir"
    fi
    if ! PATH="$sandbox/bin:$PATH" chore -C "$project" staticlib >/dev/null 2>&1; then
        echo "FAIL  $name: chore staticlib failed"
        return 1
    fi
    local got
    got="$(cat "$project/dist/libfs_bcachefs.a")"
    if [ "$got" != "fresh:$name" ]; then
        echo "FAIL  $name: copied '$got', expected fresh:$name"
        return 1
    fi
    echo "PASS  $name: copied Cargo's artifact"
}

run_case default unset ''
run_case absolute "$project/absolute-target" ''
run_case relative relative-target ''
run_case configured unset configured-target

# Even when the old default artifact exists, a failed build cannot produce a
# successful-looking dist/ archive.
rm -rf "$project/target" "$project/absolute-target" "$project/dist" "$project/.cargo"
mkdir -p "$project/target/aarch64-apple-darwin/release"
printf 'stale-default' > "$project/target/aarch64-apple-darwin/release/libfs_bcachefs.a"
export CARGO_TARGET_DIR="$project/absolute-target"
export STATICLIB_CASE=failed-build
export STATICLIB_STUB_BUILD_STATUS=23
if PATH="$sandbox/bin:$PATH" chore -C "$project" staticlib >/dev/null 2>&1; then
    echo 'FAIL  failed build: chore staticlib returned success'
    exit 1
fi
if [ -e "$project/dist/libfs_bcachefs.a" ]; then
    echo 'FAIL  failed build: stale artifact was published to dist/'
    exit 1
fi
echo 'PASS  failed build: stale default artifact was not published'
