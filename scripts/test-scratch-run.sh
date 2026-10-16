#!/usr/bin/env bash
#
# test-scratch-run.sh COMMAND [ARGS...] -- run one test command inside a
# private scratch root, and remove exactly that root afterwards.
#
# Every test invocation gets its own directory under the build's target
# scratch area, `${CARGO_TARGET_DIR:-<repo>/target}/test-scratch/run.XXXXXXXX`,
# never under the system temporary directory. The command sees it as
#
#   FS_BCACHEFS_TEST_SCRATCH_ROOT  <run>/scratch  (the tests' ScratchDir)
#   TMPDIR, TMP, TEMP              <run>/tmp      (everything else it spawns)
#
# The root is removed when the command succeeds, fails or aborts, and when
# this script receives HUP, INT or TERM: the signal is forwarded to the
# command's whole process group, and the command is reaped before anything
# is deleted, so nothing is still writing into the root as it goes. A
# command that ignores the forwarded signal gets FS_BCACHEFS_TEST_SCRATCH_GRACE
# seconds (default 10) and is then killed with SIGKILL, so a signal never
# leaves this script waiting forever. The command's exit status is this
# script's exit status; a forwarded signal ends this script by the same
# signal, after naming the command's own status on stderr when it differs.
#
# Inside the harness guest (FLTH_GUEST=1) the reference tools run chrooted
# in $FS_BCACHEFS_REF_ROOT (default /srv/ref-trixie, scripts/vm-setup.sh),
# which sees only /share. So the run's scratch/ is bind-mounted read-write
# at the same absolute path inside that root, and an image path means the
# same thing on both sides. Only that one directory is mounted, nothing is
# written to the persistent /share, and the mount is removed -- with the
# empty mount-point directories this script created and nothing else --
# before the root is deleted. Native runs mount nothing.
#
# A supervisor killed by SIGKILL cannot clean up, so each run first recovers
# the roots such runs left behind. A root is recovered only when ALL hold:
#   - it is a real directory named run.* directly under test-scratch/ (not
#     a symlink), owned by the current user;
#   - its marker file names this script, this user and this host;
#   - neither its supervisor (pid and start time) nor any process in its
#     command's process group is still alive.
# Anything else -- an active run, a root without a marker, fixtures, other
# users' or hosts' roots, anything outside test-scratch/ -- is left alone.
# A recovered guest root's chroot mount is unmounted first, but only at
# the one path this script would have made for that root.
#
# What SIGKILL of the supervisor leaves until the next run on the same
# host recovers it: the root, and in the guest its bind mount and mount
# points. A command that survives SIGKILL (uninterruptible sleep) keeps
# its root, which is reported and left for that recovery.
set -uo pipefail

[ "$#" -gt 0 ] || { echo "usage: test-scratch-run.sh COMMAND [ARGS...]" >&2; exit 64; }

MARKER_NAME=.fs-bcachefs-test-scratch
MARKER_MAGIC=fs-bcachefs-test-scratch-v1

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
target="${CARGO_TARGET_DIR:-$repo/target}"
# Cargo resolves a relative CARGO_TARGET_DIR against the working directory.
case "$target" in /*) ;; *) target="$PWD/$target" ;; esac
base="$target/test-scratch"
uid="$(id -u)"
host="$(uname -n)"
grace="${FS_BCACHEFS_TEST_SCRATCH_GRACE:-10}"
case "$grace" in ''|*[!0-9]*) echo "test-scratch-run: FS_BCACHEFS_TEST_SCRATCH_GRACE must be whole seconds" >&2; exit 64 ;; esac
guest=""
[ "${FLTH_GUEST:-}" != 1 ] || guest=1
ref_root="${FS_BCACHEFS_REF_ROOT:-/srv/ref-trixie}"
case "$ref_root" in /?*) ref_root="${ref_root%/}" ;; *) echo "test-scratch-run: FS_BCACHEFS_REF_ROOT must be absolute" >&2; exit 64 ;; esac

# The start time of a process, or nothing if it is gone: a pid alone can be
# reused, so a marker records both.
start_of() { ps -o lstart= -p "$1" 2>/dev/null | sed 's/^ *//; s/ *$//'; }

marker_field() { sed -n "s/^$2=//p" "$1" 2>/dev/null | head -n 1; }

# True when the run described by marker $1 may still be running.
run_is_active() {
    local marker="$1" pid start child now
    pid="$(marker_field "$marker" pid)"
    start="$(marker_field "$marker" start)"
    child="$(marker_field "$marker" child)"
    case "$pid" in ''|*[!0-9]*) return 0 ;; esac
    if kill -0 "$pid" 2>/dev/null; then
        now="$(start_of "$pid")"
        # Without a start time on either side, a live pid counts as active.
        [ -z "$start" ] || [ -z "$now" ] || [ "$start" = "$now" ] && return 0
    fi
    case "$child" in
        '') ;;
        *[!0-9]*) return 0 ;;
        *) kill -0 -- "-$child" 2>/dev/null && return 0 ;;
    esac
    return 1
}

remove_root() {
    chmod -R u+rwX -- "$1" 2>/dev/null || true
    rm -rf -- "$1"
}

# The one path inside the reference root where root $1's scratch/ is seen.
chroot_view() { printf '%s%s/scratch' "$ref_root" "$1"; }

# Unmount root $1's chroot view and remove the mount-point directories this
# script created for it: from the view up to, and including, the topmost
# one recorded in marker field `made`, each only while empty (rmdir). A
# `made` that is not an ancestor of the view, inside the reference root, is
# ignored. Fails, leaving the root alone, when the view stays mounted.
unmount_view() {
    local dir="$1" made="${2:-}" view d
    view="$(chroot_view "$dir")"
    if mountpoint -q -- "$view" 2>/dev/null; then
        umount -- "$view" || umount -l -- "$view" || return 1
        ! mountpoint -q -- "$view" 2>/dev/null || return 1
    fi
    case "$made" in "$ref_root"/?*) ;; *) return 0 ;; esac
    case "$view/" in "$made"/*) ;; *) return 0 ;; esac
    d="$view"
    while [ -d "$d" ] && [ ! -L "$d" ]; do
        rmdir -- "$d" 2>/dev/null || break
        [ "$d" != "$made" ] || break
        d="$(dirname -- "$d")"
    done
    return 0
}

# Bind-mount the run's scratch/ at the same path inside the reference root.
# Records in the marker the topmost directory it had to create, so the
# unmount (or a later recovery) removes exactly those.
mount_view() {
    local view d made=""
    view="$(chroot_view "$run")"
    [ -d "$ref_root" ] && [ ! -L "$ref_root" ] ||
        { echo "test-scratch-run: the reference root $ref_root is missing (scripts/vm-setup.sh)" >&2; return 1; }
    d="$view"
    while [ ! -e "$d" ]; do made="$d"; d="$(dirname -- "$d")"; done
    [ -z "$made" ] || echo "made=$made" >>"$run/$MARKER_NAME" || return 1
    mkdir -p -- "$view" || return 1
    if [ -L "$view" ] || mountpoint -q -- "$view" 2>/dev/null; then
        echo "test-scratch-run: $view is already a mount or a symlink; refusing it" >&2
        return 1
    fi
    mount --bind "$run/scratch" "$view"
}

recover_orphans() {
    local dir marker
    [ -d "$base" ] && [ ! -L "$base" ] || return 0
    for dir in "$base"/run.*; do
        [ -d "$dir" ] && [ ! -L "$dir" ] && [ -O "$dir" ] || continue
        marker="$dir/$MARKER_NAME"
        [ -f "$marker" ] && [ ! -L "$marker" ] && [ -O "$marker" ] || continue
        [ "$(marker_field "$marker" magic)" = "$MARKER_MAGIC" ] || continue
        [ "$(marker_field "$marker" uid)" = "$uid" ] || continue
        [ "$(marker_field "$marker" host)" = "$host" ] || continue
        run_is_active "$marker" && continue
        if [ -n "$guest" ] && ! unmount_view "$dir" "$(marker_field "$marker" made)"; then
            echo "test-scratch-run: cannot unmount $(chroot_view "$dir"); left $dir alone" >&2
            continue
        fi
        echo "test-scratch-run: recovered the orphaned scratch root $dir" >&2
        remove_root "$dir"
    done
}

write_marker() {
    local tmp="$run/$MARKER_NAME.tmp"
    {
        echo "magic=$MARKER_MAGIC"
        echo "uid=$uid"
        echo "host=$host"
        echo "pid=$$"
        echo "start=$(start_of $$)"
        [ -z "${1:-}" ] || echo "child=$1"
        [ -z "${made:-}" ] || echo "made=$made"
    } >"$tmp" && mv -f -- "$tmp" "$run/$MARKER_NAME"
}

mkdir -p -- "$base" || { echo "test-scratch-run: cannot create $base" >&2; exit 70; }
[ ! -L "$base" ] || { echo "test-scratch-run: $base is a symlink; refusing it" >&2; exit 70; }
base="$(cd "$base" && pwd -P)" || exit 70
recover_orphans

run="$(mktemp -d "$base/run.XXXXXXXX")" ||
    { echo "test-scratch-run: cannot create a scratch root under $base" >&2; exit 70; }
chmod 700 -- "$run"
write_marker || { remove_root "$run"; echo "test-scratch-run: cannot write the marker in $run" >&2; exit 70; }
mkdir -p -- "$run/scratch" "$run/tmp" || { remove_root "$run"; exit 70; }

made=""
if [ -n "$guest" ]; then
    if ! mount_view; then
        made="$(marker_field "$run/$MARKER_NAME" made)"
        unmount_view "$run" "$made" && remove_root "$run"
        echo "test-scratch-run: cannot bind-mount $run/scratch into $ref_root" >&2
        exit 70
    fi
    made="$(marker_field "$run/$MARKER_NAME" made)"
fi

export FS_BCACHEFS_TEST_SCRATCH_ROOT="$run/scratch"
export TMPDIR="$run/tmp" TMP="$run/tmp" TEMP="$run/tmp"

child=""
caught=""
forward() {
    caught="$1"
    [ -z "$child" ] || kill -s "$1" -- "-$child" 2>/dev/null || kill -s "$1" "$child" 2>/dev/null || true
}
trap 'forward HUP' HUP
trap 'forward INT' INT
trap 'forward TERM' TERM

# Job control puts the command in its own process group, so a forwarded
# signal reaches every process it started, and it keeps SIGINT and SIGQUIT
# at their defaults (a background command of a shell without job control
# ignores both).
set -m
"$@" &
child=$!
set +m
write_marker "$child" || true
# A signal that arrived before the pid was known is forwarded now.
[ -z "$caught" ] || forward "$caught"

# True when the command is gone within $1 tenths of a second.
gone_within() {
    local n=0
    while kill -0 "$child" 2>/dev/null; do
        [ "$n" -lt "$1" ] || return 1
        sleep 0.1
        n=$((n + 1))
    done
}

# `wait` returns early when a trap runs; the command is only reaped once
# `kill -0` can no longer see it. Once a signal has been forwarded, the
# command is never waited for unbounded: it gets the grace period, then
# SIGKILL, then a further bound, after which its root is left for recovery.
status=0
while :; do
    if [ -n "$caught" ]; then
        if ! gone_within $((grace * 10)); then
            echo "test-scratch-run: the command ignored SIG$caught for ${grace}s; sending SIGKILL" >&2
            kill -s KILL -- "-$child" 2>/dev/null || kill -s KILL "$child" 2>/dev/null || true
            if ! gone_within 50; then
                echo "test-scratch-run: the command survived SIGKILL; left $run for the next run to recover" >&2
                trap - "$caught"
                kill -s "$caught" "$$"
                exit 70
            fi
        fi
        wait "$child"
        status=$?
        break
    fi
    wait "$child"
    status=$?
    kill -0 "$child" 2>/dev/null || break
done

# Whatever the command left running in its group is stopped before its
# root is deleted underneath it.
if kill -0 -- "-$child" 2>/dev/null; then
    kill -s TERM -- "-$child" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 -- "-$child" 2>/dev/null || break
        sleep 0.5
    done
    kill -s KILL -- "-$child" 2>/dev/null || true
fi

if [ -n "$guest" ] && ! unmount_view "$run" "$made"; then
    echo "test-scratch-run: cannot unmount $(chroot_view "$run"); left $run for the next run to recover" >&2
    [ "$status" != 0 ] || status=70
else
    if ! remove_root "$run"; then
        echo "test-scratch-run: cannot remove $run; recovery is required" >&2
        [ "$status" != 0 ] || status=70
    fi
fi

if [ -n "$caught" ]; then
    want=$((128 + $(kill -l "$caught")))
    [ "$status" = "$want" ] ||
        echo "test-scratch-run: the command ended with status $status after SIG$caught" >&2
    trap - "$caught"
    kill -s "$caught" "$$"
fi
exit "$status"
