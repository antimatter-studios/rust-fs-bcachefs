#!/usr/bin/env bash
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$repo/tests/write_oracle.rs" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1]).read_text()
processes = ("bcachefs-ref", "chroot", "mountpoint", "fusermount3")
unbounded = [name for name in processes if f'Command::new("{name}")' in source]
if unbounded:
    print("FAIL  write_oracle has unbounded process launches: " + ", ".join(unbounded))
    raise SystemExit(1)
if 'Command::new("timeout")' not in source or "--kill-after=5" not in source:
    print("FAIL  write_oracle does not put external commands behind a kill-after deadline")
    raise SystemExit(1)
print("write-oracle-reference-tools-are-bounded: all reference and mount helpers use deadlines")
PY
