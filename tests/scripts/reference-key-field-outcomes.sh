#!/usr/bin/env bash
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
sandbox="$(mktemp -d)"
trap 'rm -r -- "$sandbox"' EXIT

# A target with a deadline of its own fails a group on one slow run, which is
# what the shared module exists to stop.
own_deadline="$(grep -lE 'exec timeout |Command::new\("timeout"\)' "$repo"/tests/*.rs "$repo"/tests/common/*.rs |
    grep -v '/tests/common/reference_runs\.rs$')"
if [ -n "$own_deadline" ]; then
    echo "FAIL  a guest target runs the reference under its own deadline, not common::reference_runs:" >&2
    echo "$own_deadline" >&2
    exit 1
fi

python3 - "$repo/tests/check_oracle.rs" "$repo/tests/common/reference_runs.rs" "$sandbox/outcomes.rs" <<'PY'
from pathlib import Path
import sys

source = Path(sys.argv[1]).read_text()
helper = source.split('fn reference_on_key_field(', 1)[1].split('\n}\n', 1)[0]
assert 'common::reference_runs::output(&["fsck", "-n"], image)' in helper
assert 'common::reference_runs::comparable_lines(&text)' in source
classification = helper[helper.index('    let text ='):]
Path(sys.argv[3]).write_text('#[path = "' + sys.argv[2] + '"]\n#[allow(dead_code)]\nmod reference_runs;\n'
                             'use std::process::Output;\n'
                             'fn classify(native: Output) -> (i32, String) {\n'
                             + classification + '\n}\n' + r'''
use reference_runs::{comparable_lines, finished, RUNS_PER_IMAGE, WRAPPER};
use std::cell::Cell;
use std::collections::VecDeque;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;

fn stand_in(status: i32, text: &str) -> Output {
    Command::new("sh")
        .args(["-c", "printf '%s' \"$1\"; printf '%s' \"$1\" >&2; exit \"$2\"", "stand-in", text, &status.to_string()])
        .output().unwrap()
}

fn deadline(escalated: bool) -> Output {
    let mut output = stand_in(124, "synthetic-private");
    output.stderr.extend_from_slice(b"timeout: sending signal TERM to command 'stand-in'\n");
    if escalated {
        output.status = std::process::ExitStatus::from_raw(9);
        output.stderr.extend_from_slice(b"timeout: sending signal KILL to command 'stand-in'\n");
    }
    output
}

/// The key-field verdict from these runs in turn, every one of them used.
fn verdict(results: Vec<Output>) -> (i32, String) {
    let total = results.len();
    let mut results: VecDeque<Output> = results.into();
    let asked = Cell::new(0);
    let output = finished(|| {
        asked.set(asked.get() + 1);
        results.pop_front().expect("a run past a finished one")
    });
    assert_eq!(asked.get(), total);
    classify(output)
}

fn refused(results: Vec<Output>, crashes: u8, timeouts: usize) {
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| verdict(results)))
        .expect_err("an incomplete reference run must fail");
    let message = panic.downcast_ref::<String>().map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied()).unwrap();
    assert_eq!(message, format!("reference crashes or internal faults: {crashes}; reference timeouts: {timeouts}; no case details retained"));
    assert!(!message.contains("synthetic-private"));
}

#[test]
fn accepted_verdicts_are_unchanged_and_run_once() {
    for status in [0, 1, 9, 12, 13] {
        let (actual, text) = verdict(vec![stand_in(status, "ordinary verdict")]);
        assert_eq!(actual, status);
        assert_eq!(text, "ordinary verdictordinary verdict");
    }
}

#[test]
fn reference_crashes_and_unexpected_statuses_are_private_and_not_repeated() {
    for status in [2, 125, 126, 127, 137, 139] {
        refused(vec![stand_in(status, "synthetic-private")], 1, 0);
    }
    let mut output = stand_in(0, "synthetic-private");
    output.status = std::process::ExitStatus::from_raw(11);
    refused(vec![output], 1, 0);
}

#[test]
fn internal_fault_markers_are_private_even_with_an_accepted_status() {
    for marker in ["BUG", "assertion", "backtrace", "Call Trace", "panicked at", "Kernel panic"] {
        refused(vec![stand_in(0, &format!("synthetic-private {marker}"))], 1, 0);
    }
}

#[test]
fn the_deadline_wrapper_says_why_in_the_c_locale() {
    assert!(WRAPPER.contains("export LC_ALL=C; exec timeout --verbose --kill-after=5 120 "),
        "the deadline wrapper must capture its signal diagnostics in the C locale");
}

#[test]
fn a_deadline_is_counted_and_the_next_finished_run_gives_the_verdict() {
    let (status, text) = verdict(vec![deadline(false), stand_in(9, "later verdict")]);
    assert_eq!(status, 9);
    assert_eq!(text, "later verdictlater verdict");
    let mut runs: Vec<Output> = (1..RUNS_PER_IMAGE).map(|i| deadline(i % 2 == 0)).collect();
    runs.push(stand_in(1, "later verdict"));
    assert_eq!(verdict(runs).0, 1);
}

#[test]
fn a_verdict_reached_after_a_deadline_is_final_whatever_it_says() {
    for status in [0, 1, 9, 12, 13] {
        let (actual, _) = verdict(vec![deadline(false), stand_in(status, "later verdict")]);
        assert_eq!(actual, status);
    }
}

#[test]
fn every_run_of_an_image_at_the_deadline_fails_counting_them_only() {
    refused((0..RUNS_PER_IMAGE).map(|_| deadline(false)).collect(), 0, RUNS_PER_IMAGE);
    refused((0..RUNS_PER_IMAGE).map(|_| stand_in(124, "synthetic-private")).collect(), 0, RUNS_PER_IMAGE);
}

#[test]
fn an_escalated_deadline_is_not_a_reference_crash() {
    refused((0..RUNS_PER_IMAGE).map(|_| deadline(true)).collect(), 0, RUNS_PER_IMAGE);
}

#[test]
fn a_crash_after_a_deadline_is_still_a_crash() {
    refused(vec![deadline(false), stand_in(139, "synthetic-private")], 1, 0);
    refused(vec![deadline(false), stand_in(0, "synthetic-private BUG")], 1, 0);
}

const RUN: [&str; 7] = [
    "starting version directory_size (1.20)",
    "accounting_read...accounting not marked in superblock replicas",
    "  replicas user: 1/1 [0], fixing",
    "going read-write",
    "requested incompat feature reconcile (1.33) currently not enabled",
    "  set version_upgrade=incompatible to enable",
    "check_reconcile_work... done (0 seconds)",
];

#[test]
fn a_pass_in_progress_makes_no_difference_where_its_name_lands() {
    // What the two sides printed in queue run 41795571429.
    let ended_first = RUN.join("\n");
    let joined = RUN[..4].join("\n")
        + "\ncheck_reconcile_work...requested incompat feature reconcile (1.33) currently not enabled\n"
        + RUN[5]
        + "\n done (0 seconds)";
    assert_ne!(ended_first, joined);
    assert_eq!(comparable_lines(&ended_first), comparable_lines(&joined));
    assert_eq!(comparable_lines("check_alloc_info...\n"), Vec::<String>::new());
}

#[test]
fn a_finding_only_one_run_has_still_differs() {
    let quiet = RUN.join("\n");
    for finding in [
        "subvolume 1 has invalid state",
        "check_subvols...subvolume 1 has invalid state",
        "  check_subvols...subvolume 1 has invalid state",
    ] {
        let found = quiet.clone() + "\n" + finding;
        assert_ne!(comparable_lines(&quiet), comparable_lines(&found), "{finding}");
    }
    assert_ne!(
        comparable_lines("check_subvols...subvolume 1 has invalid state"),
        comparable_lines("check_subvols...subvolume 2 has invalid state"),
    );
}

#[test]
fn only_a_lower_case_pass_name_at_the_start_of_a_line_is_removed() {
    for line in ["Doing the upgrade...later", "  indented...later", "a b...later", "...later"] {
        assert_eq!(comparable_lines(line), vec![line.to_string()]);
    }
}

#[test]
fn what_two_runs_cannot_share_is_left_out() {
    for line in ["opening /tmp/x/changed.img", "journal seq 12", "check_lrus... done (3 seconds)"] {
        assert_eq!(comparable_lines(line), Vec::<String>::new());
    }
}
''')
PY
status=$?
[ "$status" -eq 0 ] || exit "$status"
rustc --edition=2021 --test "$sandbox/outcomes.rs" -o "$sandbox/outcomes" || exit "$?"
"$sandbox/outcomes" --quiet >"$sandbox/stdout" 2>"$sandbox/stderr" || {
    status=$?
    cat "$sandbox/stdout" "$sandbox/stderr"
    exit "$status"
}
# A count that never reaches the log is no count, and it says nothing else.
grep -Eq '^reference timeouts retried in this test binary: [0-9]+; no case details retained$' "$sandbox/stderr" || {
    echo "FAIL  a retried deadline is not counted on stderr" >&2
    exit 1
}
if grep -F 'retried' "$sandbox/stderr" | grep -q 'synthetic-private'; then
    echo "FAIL  the retried count carries reference output" >&2
    exit 1
fi
echo "PASS  the guest targets repeat only a deadline and count it, and comparisons drop only what two runs cannot share"
