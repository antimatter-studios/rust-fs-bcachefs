#!/usr/bin/env bash
# The lifecycle guards and their judges must not report on what they did not
# judge, and each way they could is a control here.
#
# A shell whose trace enters a native capture makes a guard report a defect in
# fixture output, or nothing at all, and one with no trace leaves a guard no
# progress to follow (#519). The stand-in shells below do those on any host, so
# Linux CI shows them too. The guard must name the shell at once.
#
# A mutation a judge could not build is one it never exercised (#520). Each
# control here leaves the supervisor's behaviour alone: one rewrites an anchor
# as its equivalent, and the other fails only the mutation's construction.
#
#   bash tests/scripts/lifecycle-guards-refuse-what-they-cannot-judge.sh
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
guards='guest-build-fixtures-cleans-up guest-write-study-cleans-up'
judges='guest-build-fixtures-guard-judges guest-write-study-guard-judges'

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d)" || { echo 'FAIL  no sandbox: mktemp -d failed' >&2; exit 1; }
trap 'rm -r -- "$sandbox"' EXIT
real_bash="$(command -v bash)"
real_python="$(command -v python3)"

holds() {
    case "$1" in
        *"$2"*) return 0 ;;
        *) return 1 ;;
    esac
}

mkdir -p "$sandbox/shells/contaminating" "$sandbox/shells/traceless" "$sandbox/python"
# In place of the guard's own trace: xtrace on stderr, as Bash 3.2 gives it.
cat >"$sandbox/trace-on-stderr.sh" <<'STANDIN'
unset BASH_ENV
set -x
STANDIN
cat >"$sandbox/shells/contaminating/bash" <<STANDIN
#!$real_bash
export BASH_ENV="$sandbox/trace-on-stderr.sh"
exec "$real_bash" "\$@"
STANDIN
# Native output stays clean, but a guard then has no progress to follow.
cat >"$sandbox/shells/traceless/bash" <<STANDIN
#!$real_bash
unset BASH_ENV
exec "$real_bash" "\$@"
STANDIN
# Fails a judge's mutation, whose third argument is the copy it writes, and
# passes through a guard's python3, whose third is the repository.
cat >"$sandbox/python/python3" <<STANDIN
#!$real_bash
case "\${3:-}" in
    *.sh) echo 'stand-in python3: the mutated copy cannot be written' >&2; exit 71 ;;
esac
exec "$real_python" "\$@"
STANDIN
chmod +x "$sandbox/shells/contaminating/bash" "$sandbox/shells/traceless/bash" "$sandbox/python/python3"

# expect_shell_refused SCRIPT KIND SAYS: run under that stand-in shell, the
# script fails naming the shell and what it lacks, and has judged nothing.
expect_shell_refused() {
    local name="$1" kind="$2" says="$3" output status=0 text
    output="$(PATH="$sandbox/shells/$kind:$PATH" FIXTURE_GUARD_CASES=format-fail STUDY_GUARD_CASES=format-fail \
        "$real_bash" "$repo/tests/scripts/$name.sh" 2>&1)" || status=$?
    if [ "$status" = 0 ]; then
        fail "$name: accepted a $kind shell"
        return
    fi
    for text in 'shell capability' "$says"; do
        holds "$output" "$text" || fail "$name: a $kind shell's refusal does not say: $text"
    done
    for text in 'AssertionError' 'Traceback' 'a slow host' 'PARTIAL' 'PASS'; do
        holds "$output" "$text" && fail "$name: went on to judge under a $kind shell: it says $text"
    done
}

for name in $guards $judges; do
    expect_shell_refused "$name" contaminating 'native capture'
done
for name in $guards; do
    expect_shell_refused "$name" traceless 'no progress'
done

# The refusal is about the shell: this host's own runs the guard.
for name in $guards; do
    output="$(FIXTURE_GUARD_CASES=format-fail STUDY_GUARD_CASES=format-fail \
        "$real_bash" "$repo/tests/scripts/$name.sh" 2>&1)" ||
        fail "$name: failed under $real_bash: $(printf '%s\n' "$output" | tail -n 3 | cut -c1-300)"
    holds "$output" 'PARTIAL' || fail "$name: did not run its case under $real_bash"
    holds "$output" 'shell capability' && fail "$name: refused $real_bash"
done

# expect_unbuilt JUDGE MUTATION ROOT PATH_PREFIX: that judge, from that tree,
# exits nonzero, says the mutation could not be built, and prints no PASS.
# Its output stays in judge_output for the caller.
judge_output=
expect_unbuilt() {
    local judge="$1" mutation="$2" root="$3" prefix="$4" status=0
    judge_output="$(PATH="$prefix$PATH" "$real_bash" "$root/tests/scripts/$judge.sh" 2>&1)" || status=$?
    [ "$status" != 0 ] || fail "$judge: exited 0 with $mutation unbuilt"
    holds "$judge_output" "$mutation: the mutation could not be built" ||
        fail "$judge: does not name the mutation it could not build: $mutation"
    holds "$(printf '\n%s' "$judge_output")" "$(printf '\nPASS')" && fail "$judge: printed PASS with $mutation unbuilt"
}

# anchor_control JUDGE SUPERVISOR OLD NEW MUTATION OTHER: a copy of the repository's
# scripts whose supervisor holds NEW for OLD, and nothing else changed. Only the
# mutation that needs OLD is named: OTHER, which does not, still builds.
anchor_control() {
    local judge="$1" supervisor="$2" old="$3" new="$4" mutation="$5" other="$6"
    local tree="$sandbox/tree-$judge"
    mkdir -p "$tree/tests"
    cp -R "$repo/scripts" "$tree/scripts"
    cp -R "$repo/tests/scripts" "$tree/tests/scripts"
    "$real_python" - "$tree/scripts/$supervisor" "$old" "$new" <<'PY' || { fail "$judge: the control's anchor is gone: $old"; return; }
import sys
path, old, new = sys.argv[1:]
text = open(path).read()
assert text.count(old) == 1, (old, text.count(old))
open(path, 'w').write(text.replace(old, new))
PY
    "$real_bash" -n "$tree/scripts/$supervisor" || { fail "$judge: the control's rewrite does not parse"; return; }
    expect_unbuilt "$judge" "$mutation" "$tree" ''
    holds "$judge_output" "$other: the mutation could not be built" && fail "$judge: blames $other, whose anchors are unchanged"
}

# shellcheck disable=SC2016 # the supervisors' own text, not this shell's
anchor_control guest-build-fixtures-guard-judges guest-build-fixtures.sh \
    'local status=$? unmounted=yes' 'local status="${?}" unmounted=yes' stuck-in-cleanup stuck-before-ready
# shellcheck disable=SC2016 # the supervisors' own text, not this shell's
anchor_control guest-write-study-guard-judges guest-write-study.sh \
    'local status=$?' 'local status="${?}"' stuck-in-cleanup stuck-before-ready

# A mutation's construction failing for any reason other than its anchor.
for judge in $judges; do
    expect_unbuilt "$judge" stuck-before-ready "$repo" "$sandbox/python:"
done

[ "$fails" -eq 0 ] || exit 1
echo 'PASS  the lifecycle guards refuse a shell that cannot keep captures clean, and the judges refuse a mutation they could not build'
