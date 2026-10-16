#!/usr/bin/env python3
"""Run the production crc128 overwrite experiment in the harness guest.

Select the actual supervisor, mount helpers and experiment rather than keep
a second implementation of their lifecycle. No reference source is read.
The full fixture builder and all its controls remain unchanged.
"""

import sys
from pathlib import Path


def section(source, first, last):
    if source.count(first) != 1 or source.count(last) != 1:
        raise SystemExit("crc128 repro: production section moved; update the selector")
    start, end = source.index(first), source.index(last)
    if end <= start:
        raise SystemExit("crc128 repro: production section order changed")
    return source[start:end]


def program(source):
    supervisor = section(
        source, "#!/usr/bin/env bash\n", "# name | formatter options\n"
    )
    mounts = section(
        source,
        "fuse_mount() { # IMAGE OPTIONS LOG\n",
        'step "aged (mounted and aged by the reference implementation)"\n',
    )
    experiment = section(
        source,
        'echo "== crc128-overwrite (a crc128 extent partly overwritten, through the mount)"\n',
        "# POISONED EXTENTS.",
    )
    # Rename only the shell entry points so diagnostics surround the exact
    # production bodies. Their commands, options and return values are intact.
    supervisor = supervisor.replace("fuse_kill() {\n", "producer_fuse_kill() {\n", 1)
    mounts = mounts.replace("fuse_mount() {", "producer_fuse_mount() {", 1)
    diagnostics = r"""
native_mount_state() {
    local status=0
    command mountpoint -q "$ROOT$mnt" || status=$?
    printf 'native-state: %s mountpoint-status=%s target=%s\n' "$1" "$status" "$ROOT$mnt" >&2
    # mountinfo does not stat the target: a disconnected FUSE endpoint must
    # remain observable even if mountpoint refuses it.
    awk -v target="$ROOT$mnt" '$5 == target { print "native-mountinfo: " $0 }' /proc/self/mountinfo >&2
}
fuse_kill() {
    local status=0
    native_mount_state before-kill
    producer_fuse_kill || status=$?
    native_mount_state after-kill
    return "$status"
}
fuse_mount() {
    local status=0
    native_mount_state before-mount
    producer_fuse_mount "$@" || status=$?
    native_mount_state after-mount
    return "$status"
}
step "native crc128-overwrite reproducer" 240
"""
    proof = r"""
python3 - "$out" <<'PY'
import hashlib, json, random, sys
from pathlib import Path
out = Path(sys.argv[1])
for name in ("img", "json", "written.extents.txt", "written.settle.txt",
             "replay.txt", "extents.txt", "fsck.txt"):
    path = out / ("crc128-overwrite." + name)
    assert path.is_file() and path.stat().st_size > 0, f"native proof missing: {path.name}"
transcript = (out / "crc128-overwrite.txt").read_text()
assert "fsck -n: clean" in transcript, "native checker did not accept the overwritten image"
assert "did not mount" not in transcript, "native mount refused the experiment"
data = bytearray(random.Random(128).randbytes(1 << 20))
data[4096:8192] = b"overwritten\n" * 341 + b"end\n"
entries = json.loads((out / "crc128-overwrite.json").read_text())["entries"]
entry = next(e for e in entries if e["path"] == "/f")
assert entry["size"] == len(data), "native overwrite changed the file size"
assert entry["sha256"] == hashlib.sha256(data).hexdigest(), "native overwrite bytes differ"
print("crc128 native proof: both mounts, overwritten bytes and reference fsck completed")
PY
"""
    return supervisor + mounts + diagnostics + experiment + proof


if __name__ == "__main__":
    source = Path(__file__).with_name("guest-build-fixtures.sh").read_text()
    script = program(source)
    if sys.argv[1:] == ["--print-script"]:
        # Syntax inspection only; this mode never starts an oracle or a VM.
        print(script)
    elif sys.argv[1:]:
        raise SystemExit("usage: guest-repro-crc128-overwrite.py [--print-script]")
    else:
        # exec keeps the production signal/cleanup traps directly supervised
        # by the outer timeout instead of leaving a Python parent to kill.
        import os

        os.execvp("bash", ["bash", "-c", script])
