#!/usr/bin/env bash
#
# test-scratch-run-cleans-up.sh -- every test run's scratch root is private
# and is removed however the run ends.
#
# Runs the real scripts/test-scratch-run.sh against a sandboxed
# CARGO_TARGET_DIR, with small shell commands standing in for cargo, and
# checks:
#
#   - the root sits under $CARGO_TARGET_DIR/test-scratch, and the command
#     sees it as FS_BCACHEFS_TEST_SCRATCH_ROOT, TMPDIR, TMP and TEMP;
#   - the root is removed on success, on failure and on a child abort, and
#     the child's exit status is the script's exit status;
#   - on HUP, INT and TERM the signal reaches the child, the child is gone
#     before the root is, and the root is removed;
#   - a child that ignores the forwarded signal is killed after the grace
#     period, so the script ends in bounded time with the root removed, and
#     a child that exits on its own status names that status;
#   - in the guest (FLTH_GUEST=1, with mount, umount and mountpoint stubbed
#     on PATH) scratch/ is bind-mounted at its own path inside the reference
#     root, unmounted before the root goes, and the mount points this script
#     created are removed while the reference root's own directories stay;
#   - a root left by a supervisor killed with SIGKILL is recovered by the
#     next run, but not while its command is still running;
#   - an active run, an unmarked directory, another host's root and the
#     fixtures beside test-scratch/ are never swept;
#   - two concurrent runs get different roots and both are removed.
set -uo pipefail
ulimit -c 0

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUN="$REPO/scripts/test-scratch-run.sh"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d)"
trap 'chmod -R u+rwX "$sandbox" 2>/dev/null; rm -rf -- "$sandbox"' EXIT
export CARGO_TARGET_DIR="$sandbox/target"
base="$CARGO_TARGET_DIR/test-scratch"

no_runs_left() { ! ls -d "$base"/run.* >/dev/null 2>&1; }

# Wait up to ten seconds for a file to appear.
await() {
    for _ in $(seq 1 100); do [ -s "$1" ] && return 0; sleep 0.1; done
    return 1
}

# --- success: the environment, the placement, the removal.
out="$sandbox/env.txt"
bash "$RUN" sh -c 'printf "%s\n%s\n%s\n%s\n" "$FS_BCACHEFS_TEST_SCRATCH_ROOT" "$TMPDIR" "$TMP" "$TEMP" >"$1"
    : >"$FS_BCACHEFS_TEST_SCRATCH_ROOT/image.img" && : >"$TMPDIR/tmp.img"' sh "$out"
status=$?
[ "$status" = 0 ] || fail "a passing command made the script exit $status"
root="$(sed -n 1p "$out")"
case "$root" in "$base"/run.*/scratch) ;; *) fail "the scratch root $root is not under $base" ;; esac
[ "$(sed -n 2p "$out")" = "$(dirname "$root")/tmp" ] || fail "TMPDIR is not the run's tmp/"
[ "$(sed -n 2p "$out")" = "$(sed -n 3p "$out")" ] && [ "$(sed -n 3p "$out")" = "$(sed -n 4p "$out")" ] ||
    fail "TMP and TEMP do not match TMPDIR"
[ ! -e "$(dirname "$root")" ] || fail "the root of a passing run was left behind"

# --- failure and abort: the status is preserved, the root removed.
bash "$RUN" sh -c 'echo "$FS_BCACHEFS_TEST_SCRATCH_ROOT" >"$1"; : >"$FS_BCACHEFS_TEST_SCRATCH_ROOT/x.img"; exit 3' sh "$out"
status=$?
[ "$status" = 3 ] || fail "a command exiting 3 made the script exit $status"
[ ! -e "$(dirname "$(cat "$out")")" ] || fail "the root of a failed run was left behind"

bash "$RUN" sh -c 'echo "$FS_BCACHEFS_TEST_SCRATCH_ROOT" >"$1"; kill -ABRT $$' sh "$out" 2>/dev/null
status=$?
[ "$status" = 134 ] || fail "a command killed by SIGABRT made the script exit $status, not 134"
[ ! -e "$(dirname "$(cat "$out")")" ] || fail "the root of an aborted run was left behind"
no_runs_left || fail "runs were left under $base"

# --- signals: forwarded, the child reaped first, the root removed.
for sig in HUP INT TERM; do
    rm -f "$out" "$sandbox/child.pid"
    # Job control, so the script under test does not start with SIGINT ignored.
    set -m
    bash "$RUN" sh -c 'echo $$ >"$2"; echo "$FS_BCACHEFS_TEST_SCRATCH_ROOT" >"$1"; exec sleep 30' \
        sh "$out" "$sandbox/child.pid" 2>"$sandbox/$sig.err" &
    sup=$!
    set +m
    await "$out" || { fail "$sig: the child never started"; kill -KILL -- "-$sup" 2>/dev/null; continue; }
    kill -s "$sig" "$sup"
    wait "$sup" 2>/dev/null
    status=$?
    want=$((128 + $(kill -l "$sig")))
    [ "$status" = "$want" ] || fail "$sig: the script exited $status, not $want"
    kill -0 "$(cat "$sandbox/child.pid")" 2>/dev/null && fail "$sig: the child was still running"
    [ ! -e "$(dirname "$(cat "$out")")" ] || fail "$sig: the root was left behind"
done
no_runs_left || fail "signalled runs were left under $base"

# --- a child that ignores the signal: bounded escalation, cleanup, status.
rm -f "$out" "$sandbox/child.pid"
set -m
FS_BCACHEFS_TEST_SCRATCH_GRACE=1 bash "$RUN" sh -c 'trap "" TERM; echo $$ >"$2"
    echo "$FS_BCACHEFS_TEST_SCRATCH_ROOT" >"$1"; exec sleep 60' \
    sh "$out" "$sandbox/child.pid" 2>"$sandbox/ignore.err" &
sup=$!
set +m
if await "$out" && await "$sandbox/child.pid"; then
    kill -s TERM "$sup"
    # Bounded here too: a supervisor that waits forever fails, not hangs.
    for _ in $(seq 1 150); do kill -0 "$sup" 2>/dev/null || break; sleep 0.1; done
    if kill -0 "$sup" 2>/dev/null; then
        fail "ignored TERM: the script was still waiting after 15s"
        kill -KILL -- "-$sup" 2>/dev/null
        kill -KILL -- "-$(cat "$sandbox/child.pid")" 2>/dev/null
    fi
    wait "$sup" 2>/dev/null
    status=$?
    [ "$status" = 143 ] || fail "ignored TERM: the script exited $status, not 143"
    kill -0 "$(cat "$sandbox/child.pid")" 2>/dev/null && fail "ignored TERM: the child was still running"
    [ ! -e "$(dirname "$(cat "$out")")" ] || fail "ignored TERM: the root was left behind"
    grep -q 'sending SIGKILL' "$sandbox/ignore.err" || fail "ignored TERM: the escalation was not reported"
    grep -q 'status 137' "$sandbox/ignore.err" || fail "ignored TERM: the child's status 137 was not reported"
else
    fail "ignored TERM: the child never started"
    kill -KILL -- "-$sup" 2>/dev/null
fi

rm -f "$out" "$sandbox/child.pid"
set -m
bash "$RUN" sh -c 'trap "exit 7" TERM; echo $$ >"$2"; echo "$FS_BCACHEFS_TEST_SCRATCH_ROOT" >"$1"
    while :; do sleep 0.1; done' sh "$out" "$sandbox/child.pid" 2>"$sandbox/own.err" &
sup=$!
set +m
if await "$out"; then
    kill -s TERM "$sup"
    wait "$sup" 2>/dev/null
    status=$?
    [ "$status" = 143 ] || fail "handled TERM: the script exited $status, not 143"
    grep -q 'status 7 after SIGTERM' "$sandbox/own.err" || fail "handled TERM: the child's status 7 was not reported"
    [ ! -e "$(dirname "$(cat "$out")")" ] || fail "handled TERM: the root was left behind"
else
    fail "handled TERM: the child never started"
    kill -KILL -- "-$sup" 2>/dev/null
fi
no_runs_left || fail "escalated runs were left under $base"

# --- the guest: the chroot view is mounted, unmounted, and leaves nothing.
stubs="$sandbox/stubs"
ref="$sandbox/ref"
mkdir -p "$stubs" "$ref/keep"
: >"$sandbox/mounts"
cat >"$stubs/mountpoint" <<'STUB'
#!/usr/bin/env bash
[ "$1" = -q ] && shift; [ "$1" = -- ] && shift
grep -qxF -- "$1" "$STUB_MOUNTS"
STUB
cat >"$stubs/mount" <<'STUB'
#!/usr/bin/env bash
echo "mount $*" >>"$STUB_LOG"
[ "$1" = --bind ] && [ -d "$2" ] && [ -d "$3" ] || exit 32
echo "$3" >>"$STUB_MOUNTS"
STUB
cat >"$stubs/umount" <<'STUB'
#!/usr/bin/env bash
[ "$1" = -- ] && shift
echo "umount $1" >>"$STUB_LOG"
grep -vxF -- "$1" "$STUB_MOUNTS" >"$STUB_MOUNTS.new"; mv -f "$STUB_MOUNTS.new" "$STUB_MOUNTS"
STUB
chmod +x "$stubs/mountpoint" "$stubs/mount" "$stubs/umount"
guest_run() {
    PATH="$stubs:$PATH" STUB_LOG="$sandbox/mount.log" STUB_MOUNTS="$sandbox/mounts" \
        FLTH_GUEST=1 FS_BCACHEFS_REF_ROOT="$ref" bash "$RUN" "$@"
}
for want in 0 3; do
    : >"$sandbox/mount.log"
    # The child must expand its own run environment, not this shell's.
    # shellcheck disable=SC2016
    guest_run sh -c 'echo "$FS_BCACHEFS_TEST_SCRATCH_ROOT" >"$1"
        [ -d "$2$FS_BCACHEFS_TEST_SCRATCH_ROOT" ] && grep -qxF "$2$FS_BCACHEFS_TEST_SCRATCH_ROOT" "$STUB_MOUNTS" || exit 9
        exit "$3"' sh "$out" "$ref" "$want"
    status=$?
    [ "$status" = "$want" ] || fail "guest: a command exiting $want made the script exit $status"
    scratch_root="$(cat "$out")"
    printf 'mount --bind %s %s\numount %s\n' "$scratch_root" "$ref$scratch_root" "$ref$scratch_root" |
        cmp -s - "$sandbox/mount.log" || fail "guest ($want): the mount commands were: $(cat "$sandbox/mount.log")"
    [ ! -s "$sandbox/mounts" ] || fail "guest ($want): a mount was left: $(cat "$sandbox/mounts")"
    [ ! -e "$(dirname "$scratch_root")" ] || fail "guest ($want): the root was left behind"
    [ "$(ls -A "$ref")" = keep ] || fail "guest ($want): the reference root holds $(ls -A "$ref"), not just keep"
done

# A guest supervisor killed with SIGKILL: the next guest run unmounts and recovers.
: >"$sandbox/mount.log"
rm -f "$out" "$sandbox/child.pid"
set -m
PATH="$stubs:$PATH" STUB_LOG="$sandbox/mount.log" STUB_MOUNTS="$sandbox/mounts" \
    FLTH_GUEST=1 FS_BCACHEFS_REF_ROOT="$ref" bash "$RUN" \
    sh -c 'echo $$ >"$2"; echo "$FS_BCACHEFS_TEST_SCRATCH_ROOT" >"$1"; exec sleep 30' \
    sh "$out" "$sandbox/child.pid" &
sup=$!
set +m
if await "$out" && await "$sandbox/child.pid"; then
    orphan="$(dirname "$(cat "$out")")"
    child="$(cat "$sandbox/child.pid")"
    for _ in $(seq 1 100); do grep -q '^child=' "$orphan/.fs-bcachefs-test-scratch" && break; sleep 0.1; done
    kill -KILL "$sup"
    wait "$sup" 2>/dev/null
    kill -KILL -- "-$child" 2>/dev/null || kill -KILL "$child" 2>/dev/null
    for _ in $(seq 1 50); do kill -0 "$child" 2>/dev/null || break; sleep 0.1; done
    grep -qxF "$ref$orphan/scratch" "$sandbox/mounts" || fail "guest SIGKILL: the orphan's view was not mounted"
    guest_run true 2>/dev/null || fail "guest SIGKILL: the recovering run failed"
    [ ! -e "$orphan" ] || fail "guest SIGKILL: the orphaned root was not recovered"
    [ ! -s "$sandbox/mounts" ] || fail "guest SIGKILL: a mount was left: $(cat "$sandbox/mounts")"
    [ "$(ls -A "$ref")" = keep ] || fail "guest SIGKILL: the reference root holds $(ls -A "$ref"), not just keep"
else
    fail "guest SIGKILL: the child never started"
    kill -KILL -- "-$sup" 2>/dev/null
fi
no_runs_left || fail "guest runs were left under $base"

# --- SIGKILL of the supervisor: not swept while its command runs, swept after.
rm -f "$out" "$sandbox/child.pid"
set -m
bash "$RUN" sh -c 'echo $$ >"$2"; echo "$FS_BCACHEFS_TEST_SCRATCH_ROOT" >"$1"; exec sleep 30' \
    sh "$out" "$sandbox/child.pid" &
sup=$!
set +m
if await "$out" && await "$sandbox/child.pid"; then
    orphan="$(dirname "$(cat "$out")")"
    child="$(cat "$sandbox/child.pid")"
    # The marker names the child once the script has started it.
    for _ in $(seq 1 100); do grep -q '^child=' "$orphan/.fs-bcachefs-test-scratch" && break; sleep 0.1; done
    kill -KILL "$sup"
    wait "$sup" 2>/dev/null
    [ -d "$orphan" ] || fail "SIGKILL: the root vanished with no supervisor to remove it"
    bash "$RUN" true || fail "a run beside an active orphan failed"
    [ -d "$orphan" ] || fail "SIGKILL: a root whose command was still running was swept"
    kill -KILL -- "-$child" 2>/dev/null || kill -KILL "$child" 2>/dev/null
    for _ in $(seq 1 50); do kill -0 "$child" 2>/dev/null || break; sleep 0.1; done
    bash "$RUN" true 2>/dev/null || fail "the recovering run failed"
    [ ! -e "$orphan" ] || fail "SIGKILL: the orphaned root was not recovered by the next run"
else
    fail "SIGKILL: the child never started"
    kill -KILL -- "-$sup" 2>/dev/null
fi

# --- never swept: an active run, unmarked dirs, other hosts, fixtures.
mkdir -p "$base/run.active" "$base/run.nomarker" "$base/run.otherhost" \
    "$base/fixtures" "$CARGO_TARGET_DIR/fixtures"
: >"$base/fixtures/keep.img"
: >"$CARGO_TARGET_DIR/fixtures/keep.img"
sleep 30 &
alive=$!
printf 'magic=fs-bcachefs-test-scratch-v1\nuid=%s\nhost=%s\npid=%s\nstart=%s\n' \
    "$(id -u)" "$(uname -n)" "$alive" "$(ps -o lstart= -p "$alive" | sed 's/^ *//; s/ *$//')" \
    >"$base/run.active/.fs-bcachefs-test-scratch"
printf 'magic=fs-bcachefs-test-scratch-v1\nuid=%s\nhost=%s\npid=%s\nstart=\n' \
    "$(id -u)" "not-$(uname -n)" 1 >"$base/run.otherhost/.fs-bcachefs-test-scratch"
bash "$RUN" true || fail "a run beside foreign directories failed"
for kept in "$base/run.active" "$base/run.nomarker" "$base/run.otherhost" \
    "$base/fixtures/keep.img" "$CARGO_TARGET_DIR/fixtures/keep.img"; do
    [ -e "$kept" ] || fail "$kept was swept"
done
kill "$alive" 2>/dev/null
wait "$alive" 2>/dev/null
rm -rf -- "$base/run.active" "$base/run.nomarker" "$base/run.otherhost"

# --- concurrent runs: independent roots, both removed.
concurrent=()
for n in 1 2; do
    bash "$RUN" sh -c 'echo "$FS_BCACHEFS_TEST_SCRATCH_ROOT" >"$1"; : >"$FS_BCACHEFS_TEST_SCRATCH_ROOT/mine.img"
        sleep 1; [ -f "$FS_BCACHEFS_TEST_SCRATCH_ROOT/mine.img" ]' sh "$sandbox/conc$n" &
    concurrent+=("$!")
done
wait "${concurrent[0]}" || fail "the first concurrent run lost its scratch file"
wait "${concurrent[1]}" || fail "the second concurrent run lost its scratch file"
[ "$(cat "$sandbox/conc1")" != "$(cat "$sandbox/conc2")" ] || fail "two concurrent runs shared a root"
no_runs_left || fail "concurrent runs were left under $base"

[ "$fails" -eq 0 ] || exit 1
echo "PASS  test-scratch-run.sh removes exactly its own root however a run ends"
