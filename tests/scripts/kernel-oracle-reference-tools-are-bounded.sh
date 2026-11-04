#!/usr/bin/env bash
# A hung kernel-oracle boot ends the fixture build with a named failure, and
# leaves its console logs where a failed or cancelled job can upload them
# (#500). Runs 41575010746 and 41586917472 stopped a hung first boot at its
# limit, then printed nothing until the job's 45-minute limit cancelled them
# with a docker process still running, and the console logs were never
# uploaded. Stopping the docker client does not stop its container: a
# container's first process ignores SIGTERM unless it installs a handler, so
# the stand-in here ignores SIGTERM too, and goes on running until it is
# removed by name.
#
# The whole script runs, under a watchdog, in a sandbox copy of scripts/ with
# stand-ins for QEMU, docker and this crate's writer. The copy differs in one
# place: its check for /dev/kvm passes, because the runners of this tier have
# no KVM and no stand-in can provide a device.
#
#   bash tests/scripts/kernel-oracle-reference-tools-are-bounded.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

# The host's own timeout(1) wins; tests/scripts/support is for a host with none.
export PATH="$PATH:$REPO/tests/scripts/support"
command -v timeout >/dev/null || {
    fail "timeout(1) is missing: this test and kernel-oracle.sh need GNU coreutils', or tests/scripts/support/timeout"
    exit 1
}

# Long enough for a script that stops one call at the 1-second limits below,
# short enough that one that never ends costs this tier little.
WATCHDOG=30

tmp="$(mktemp -d "${TMPDIR:-/tmp}/ko-tools.XXXXXX")"
cleanup() {
    local status=$? diagnostic scenario_log
    if [ "$status" -ne 0 ]; then
        diagnostic="$REPO/tmp/logs/kernel-oracle-reference-tools-are-bounded.log"
        mkdir -p "$REPO/tmp/logs"
        for scenario_log in "$tmp"/*/log; do
            [ -f "$scenario_log" ] || continue
            printf '== %s\n' "$scenario_log"
            cat "$scenario_log"
        done >"$diagnostic"
        echo "kernel-oracle-reference-tools-are-bounded: diagnostics: $diagnostic" >&2
    fi
    rm -r -- "$tmp"
    exit "$status"
}
trap cleanup EXIT
stubs="$tmp/stubs"
mkdir "$stubs"

# QEMU: writes each serial port's file, as a booted guest does, and exits with
# the status KO_BOOT names, which for a first boot `boot` passes on. 124 is
# what a guest stopped at its limit returns; the real limit is 300 seconds,
# and tests/scripts/kernel-oracle-boot-is-bounded.sh already holds `boot` to it.
# A first boot's console is KO_CONSOLE, when that names a file.
cat >"$stubs/qemu-system-x86_64" <<'EOF'
#!/usr/bin/env bash
: >"$KO_STATE/booted"
judge=
for arg in "$@"; do
    case "$arg" in
        *oracle=judge*) judge=1 ;;
        file:*) echo "ORACLE kernel: stand-in" >"${arg#file:}" ;;
    esac
done
if [ -n "$judge" ]; then
    : >"$KO_STATE/judge-booted"
    exit 0
fi
if [ -n "${KO_CONSOLE:-}" ]; then
    for arg in "$@"; do
        case "$arg" in file:*/console.log) cp "$KO_CONSOLE" "${arg#file:}" ;; esac
    done
fi
exit "$KO_BOOT"
EOF

# The docker client and its daemon. KO_DOCKER says what happens after the
# module's container has been built:
#   answers      every reference tool returns
#   fsck-errors  the reference checker reports errors and returns
#   hang         a reference tool's container never ends until removed by name
#   wedged       nothing answers once the guest has booted, removal included
# A stand-in that is never stopped gives up after a minute, past the watchdog,
# so that nothing outlives this test.
cat >"$stubs/docker" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$KO_STATE/docker.calls"
never_returns() {
    trap '' TERM
    [ "$KO_DOCKER" != build-term ] || trap 'exit 143' TERM
    i=0
    while [ "$i" -lt 240 ] && { [ -z "${1:-}" ] || [ -e "$1" ]; }; do
        sleep 0.25
        i=$((i + 1))
    done
    exit 137
}
if [ "$KO_DOCKER" = wedged ] && [ -e "$KO_STATE/booted" ]; then
    never_returns ""
fi
case "$1" in
    rm | kill)
        case "$KO_DOCKER:$*" in
            build-remove-stale:*) never_returns "" ;;
            "build-remove:rm rust-fs-bcachefs-ko") never_returns "" ;;
        esac
        for last; do :; done
        rm -f "$KO_STATE/running.$last"
        exit 0
        ;;
    commit)
        [ "$KO_DOCKER" != build-commit ] || never_returns ""
        exit 0
        ;;
    run) ;;
    *) exit 0 ;;
esac
name=unnamed
previous=
for arg; do
    [ "$previous" = --name ] && name="$arg"
    previous="$arg"
done
case "$*" in
    *debian:trixie*)
        : >"$KO_STATE/running.$name"
        case "$KO_DOCKER" in
            build-run | build-term) never_returns "$KO_STATE/running.$name" ;;
        esac
        work="$KO_REPO/tmp/kernel-oracle"
        echo 0.0.0-stand-in >"$work/kver"
        : >"$work/vmlinuz"
        : >"$work/root/modules.order"
        : >"$work/options-help.txt"
        echo ok >"$work/written.format.status"
        for image in kernel kernel-reuse kernel-rootsnap kernel-prealloc kernel-written; do
            : >"$work/$image.img"
        done
        exit 0
        ;;
esac
case "$KO_DOCKER" in
    hang)
        : >"$KO_STATE/running.$name"
        never_returns "$KO_STATE/running.$name"
        ;;
    fsck-errors)
        case "$*" in *" fsck "*)
            echo "stand-in checker: errors"
            exit 1
            ;;
        esac
        ;;
esac
echo "stand-in tool: $*"
EOF

# This crate's writer. KO_WRITER=hang never returns.
cat >"$stubs/writer" <<'EOF'
#!/usr/bin/env bash
if [ "$KO_WRITER" = hang ]; then
    trap '' TERM
    i=0
    while [ "$i" -lt 240 ]; do
        sleep 0.25
        i=$((i + 1))
    done
fi
exit 0
EOF

# BSD cp has no --sparse. The holes are not what this test is about.
cat >"$stubs/cp" <<'EOF'
#!/usr/bin/env bash
args=()
for arg; do
    case "$arg" in --sparse=*) ;; *) args+=("$arg") ;; esac
done
exec /bin/cp "${args[@]}"
EOF
chmod +x "$stubs"/*

# scenario NAME BOOT_STATUS DOCKER_MODE WRITER_MODE sets repo, out, state,
# log, status and fin, and fails if the script had to be killed.
scenario() {
    name="$1"
    repo="$tmp/$name/repo"
    out="$tmp/$name/out"
    state="$tmp/$name/state"
    log="$tmp/$name/log"
    mkdir -p "$repo" "$state"
    cp -R "$REPO/scripts" "$repo/scripts"
    sed 's#\[ -w /dev/kvm \]#true#' "$REPO/scripts/kernel-oracle.sh" >"$repo/scripts/kernel-oracle.sh"
    cmp -s "$REPO/scripts/kernel-oracle.sh" "$repo/scripts/kernel-oracle.sh" &&
        fail "$name: kernel-oracle.sh no longer checks /dev/kvm the way this test passes it"
    start=$SECONDS
    KO_STATE="$state" KO_REPO="$repo" KO_BOOT="$2" KO_DOCKER="$3" KO_WRITER="$4" \
        KERNEL_ORACLE_WRITER="$stubs/writer" KERNEL_ORACLE_BUILD_LIMIT=1 KERNEL_ORACLE_TOOL_LIMIT=1 \
        KERNEL_ORACLE_CLEANUP_LIMIT=1 KERNEL_ORACLE_KILL_AFTER=1 PATH="$stubs:$PATH" \
        timeout -s KILL "$WATCHDOG" bash "$repo/scripts/kernel-oracle.sh" "$out" >"$log" 2>&1
    status=$?
    took=$((SECONDS - start))
    fin=1
    if [ "$took" -ge "$((WATCHDOG - 1))" ]; then
        fin=
        fail "$name: the script ran ${took}s until the watchdog killed it, after: $(tail -n 1 "$state/docker.calls" | cut -c1-120)"
    fi
}

recorded() { grep -q "^$1" "$out/kernel.txt" 2>/dev/null || fail "$name: kernel.txt has no line starting '$1'"; }
not_recorded() { ! grep -q "^$1" "$out/kernel.txt" 2>/dev/null || fail "$name: kernel.txt has a line starting '$1'"; }
named() { grep -q "^kernel-oracle: FAILED.*$1" "$log" || fail "$name: no 'kernel-oracle: FAILED' line names '$1'"; }
not_named() { ! grep -q "^kernel-oracle: FAILED.*$1" "$log" || fail "$name: the 'kernel-oracle: FAILED' line names '$1'"; }
exits() { [ "$status" -eq "$1" ] || fail "$name: exited $status, not $1"; }
no_container_left() {
    for left in "$state"/running.*; do
        [ ! -e "$left" ] || fail "$name: a container is still running: ${left##*/running.}"
    done
}

# Nothing is wrong: the script succeeds, every output is written, and nothing
# was stopped. Without this, a script that always fails would pass the rest.
scenario clean 0 answers none
clean_repo="$repo"
exits 0
recorded "fsck: clean"
recorded "reuse-fsck: clean"
recorded "rootsnap-fsck: clean"
recorded "written-fsck: clean"
recorded "prealloc-fsck: clean"
recorded "written: ok"
recorded "written-opt: ok"
not_recorded "[a-z.-]*: stopped"
not_recorded "[a-z.-]*: not run"
not_named ""
[ -e "$state/judge-booted" ] || fail "$name: the judge boot did not run"
no_container_left
for output in kernel.fsck kernel-reuse.fsck kernel-rootsnap.fsck kernel-written.fsck \
    kernel-prealloc.fsck kernel-prealloc.extents kernel-prealloc.inodes kernel-written.inodes \
    kernel.reflink kernel.subvolumes kernel.snapshots kernel.snapshot_trees kernel.inodes \
    kernel.dirents kernel.extents; do
    [ -s "$out/$output.txt" ] || fail "$name: $output.txt is empty or missing"
done

for build_call in run term commit remove remove-stale; do
    scenario "hung-build-$build_call" 0 "build-$build_call" none
    if [ -n "$fin" ]; then
        exits 1
        build_key="build-$build_call"
        [ "$build_call" != term ] || build_key=build-run
        recorded "$build_key: stopped at its 1 s limit"
        named "$build_key"
        [ ! -e "$state/booted" ] || fail "$name: the guest booted after a build call stopped"
        ! grep -q ':tools bcachefs ' "$state/docker.calls" ||
            fail "$name: a reference tool ran after a build call stopped"
        no_container_left
    fi
done

build_limit="$(sed -n 's/^BUILD_LIMIT="${KERNEL_ORACLE_BUILD_LIMIT:-\([0-9][0-9]*\)}"$/\1/p' "$REPO/scripts/kernel-oracle.sh")"
case "$build_limit" in
    '' | *[!0-9]*) fail "the module build has no numeric default limit" ;;
    *)
        [ "$build_limit" -ge 200 ] || fail "the build limit leaves less than twice the measured 100 s"
        [ "$((build_limit + 15 + 2 * (30 + 15)))" -lt "$((45 * 60))" ] ||
            fail "the build and its removals can exhaust the fixtures job's 45-minute limit"
        ;;
esac

# A reference checker that finds errors is a recorded result, for
# tests/oracle_kernel.rs to fail on, and not a hang.
scenario checker-errors 0 fsck-errors none
exits 0
recorded "fsck: errors (kernel.fsck.txt)"
not_recorded "[a-z.-]*: stopped"
not_named ""

# The hang the issue reports: a first boot stopped at its limit, then a
# reference tool that never returns.
scenario hung-boot-hung-tool 124 hang none
if [ -n "$fin" ]; then
    exits 1
    recorded "boot: "
    recorded "judge: not booted"
    recorded "fsck: stopped"
    recorded "reuse-fsck: not run"
    named "first boot"
    named "fsck"
    no_container_left
    [ -e "$state/judge-booted" ] && fail "$name: the judge boot ran after a hung first boot"
    tools="$(grep -c ':tools bcachefs ' "$state/docker.calls")"
    [ "$tools" -eq 1 ] || fail "$name: $tools reference tool calls, not 1: after one is stopped the rest are not run"
fi
[ -s "$out/kernel.console.txt" ] || fail "$name: kernel.console.txt is empty or missing"

# A reference tool that never returns, after boots that went well.
scenario hung-tool 0 hang none
if [ -n "$fin" ]; then
    exits 1
    recorded "fsck: stopped"
    named "fsck"
    not_named "first boot"
    no_container_left
    [ -e "$state/judge-booted" ] || fail "$name: the judge boot did not run"
fi

# A hung first boot alone fails the build, and the tools still judge its image.
scenario hung-boot 124 answers none
if [ -n "$fin" ]; then
    exits 1
    recorded "boot: "
    recorded "judge: not booted"
    recorded "fsck: clean"
    recorded "prealloc-fsck: clean"
    named "first boot"
fi

# Its console says nothing about the module, so nobody is accused.
if [ -n "$fin" ]; then
    named "no report from the reference kernel module"
    recorded "boot: .*no report from the reference kernel module"
fi

# The module's own hang fails the build just the same, and says it is the
# module's (#514): naming it must not turn a hang into a pass.
module_console="$tmp/module-console"
{
    echo '[   15.353711] bcachefs (vdb): Journal stuck? Waited for 10s, err journal_full'
    echo '[   15.353711] seq:                       66'
    echo '[   15.353711] seq_ondisk:                66'
    echo '[  243.002633] INFO: task rm:1697 blocked for more than 120 seconds.'
    echo '[  243.012531]  some_module_function+0x528/0x590 [bcachefs]'
    echo '[  243.051753]  </TASK>'
} >"$module_console"
export KO_CONSOLE="$module_console"
scenario hung-module 124 answers none
unset KO_CONSOLE
if [ -n "$fin" ]; then
    exits 1
    named "first boot"
    named "reference kernel module hung"
    named "vdb"
    recorded "boot: .*reference kernel module hung"
    recorded "judge: not booted"
    recorded "fsck: clean"
    [ -e "$state/judge-booted" ] && fail "$name: the judge boot ran after a hung first boot"
fi

# A guest killed after ignoring its limit returns 137 and is the same hang.
scenario killed-boot 137 answers none
if [ -n "$fin" ]; then
    exits 1
    recorded "boot: "
    named "first boot"
fi

# A docker daemon that has stopped answering: the removal of the container and
# of the image hang as well.
scenario wedged-daemon 124 wedged none
if [ -n "$fin" ]; then
    exits 1
    recorded "fsck: stopped"
    named "first boot"
    named "fsck"
fi

# This crate's writer runs on the image a hung guest left, ahead of the tools.
scenario hung-boot-hung-writer 124 answers hang
if [ -n "$fin" ]; then
    exits 1
    recorded "written: stopped"
    recorded "fsck: not run"
    named "first boot"
    named "writer"
fi

# What the job keeps when it fails or is cancelled, read from the workflow and
# checked against what the script writes. Hidden paths do not count, because
# actions/upload-artifact leaves them out.
if python3 -c 'import yaml' 2>/dev/null; then
    uploaded="$(
        python3 - "$REPO/.github/workflows/ci.yml" <<'PY'
import sys, yaml

steps = yaml.safe_load(open(sys.argv[1]))["jobs"]["fixtures"]["steps"]
build = [i for i, s in enumerate(steps) if s.get("run") == "chore fixtures"]
if not build:
    sys.exit("the fixtures job has no chore fixtures step")
for step in steps[build[0] + 1:]:
    when = str(step.get("if", ""))
    if not str(step.get("uses", "")).startswith("actions/upload-artifact@"):
        continue
    if "always()" in when or ("failure()" in when and "cancelled()" in when):
        print(step["with"]["path"])
PY
    )"
    for file in console.log oracle.log judge-console.log judge.log; do
        path="tmp/kernel-oracle/$file"
        printf '%s\n' "$uploaded" | grep -qx "$path" ||
            fail "the fixtures job uploads no $path when it fails or is cancelled"
        [ -s "$clean_repo/$path" ] || fail "kernel-oracle.sh writes no $path for the job to upload"
    done
else
    fail "the python3 yaml module is required to read ci.yml (pip install pyyaml)"
fi

if [ "$fails" -gt 0 ]; then
    echo "kernel-oracle-reference-tools-are-bounded: $fails check(s) failed" >&2
    exit 1
fi
echo "PASS  a hung kernel-oracle boot or reference tool ends the build by name, and its console logs are kept"
