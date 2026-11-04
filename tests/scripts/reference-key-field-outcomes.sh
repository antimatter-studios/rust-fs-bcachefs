#!/usr/bin/env bash
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
sandbox="$(mktemp -d)"
trap 'rm -r -- "$sandbox"' EXIT

python3 - "$repo/tests/check_oracle.rs" "$sandbox/outcomes.rs" <<'PY'
from pathlib import Path
import json
import re
import sys

source = Path(sys.argv[1]).read_text()
helper = source.split('fn reference_on_key_field(', 1)[1].split('\n}\n', 1)[0]
classification = helper[helper.index('    let text ='):]
wrapper = re.search(r'"(ulimit -c 0;.*?)",\n', helper).group(1)
wrapper = json.loads('"' + wrapper + '"')
Path(sys.argv[2]).write_text('use std::process::{Command, Output};\n'
                           'const WRAPPER: &str = ' + json.dumps(wrapper) + ';\n'
                           'fn classify(native: Output) -> (i32, String) {\n'
                           + classification + '\n}\n' + r'''
fn stand_in(status: i32, text: &str) -> Output {
    Command::new("sh")
        .args(["-c", "printf '%s' \"$1\"; printf '%s' \"$1\" >&2; exit \"$2\"", "stand-in", text, &status.to_string()])
        .output().unwrap()
}

fn refused(output: Output, crashes: u8, timeouts: u8) {
    let panic = std::panic::catch_unwind(|| classify(output)).expect_err("an incomplete reference run must fail");
    let message = panic.downcast_ref::<String>().map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied()).unwrap();
    assert_eq!(message, format!("reference crashes or internal faults: {crashes}; reference timeouts (slow host): {timeouts}; no case details retained"));
    assert!(!message.contains("synthetic-private"));
}

#[test]
fn accepted_verdicts_are_unchanged() {
    for status in [0, 1, 9, 12, 13] {
        let (actual, text) = classify(stand_in(status, "ordinary verdict"));
        assert_eq!(actual, status);
        assert_eq!(text, "ordinary verdictordinary verdict");
    }
}

#[test]
fn reference_crashes_and_unexpected_statuses_are_private() {
    for status in [2, 125, 126, 127, 137, 139] {
        refused(stand_in(status, "synthetic-private"), 1, 0);
    }
    use std::os::unix::process::ExitStatusExt;
    let mut output = stand_in(0, "synthetic-private");
    output.status = std::process::ExitStatus::from_raw(11);
    refused(output, 1, 0);
}

#[test]
fn internal_fault_markers_are_private_even_with_an_accepted_status() {
    for marker in ["BUG", "assertion", "backtrace", "Call Trace", "panicked at", "Kernel panic"] {
        refused(stand_in(0, &format!("synthetic-private {marker}")), 1, 0);
    }
}

#[test]
fn timeout_status_without_diagnostics_is_not_a_reference_crash() {
    refused(stand_in(124, "synthetic-private"), 0, 1);
}

fn deadline(escalated: bool) -> Output {
    assert!(WRAPPER.contains("export LC_ALL=C; exec timeout --verbose --kill-after=5 60 "),
        "the deadline wrapper must capture its signal diagnostics in the C locale");
    let mut output = stand_in(124, "synthetic-private");
    output.stderr.extend_from_slice(b"timeout: sending signal TERM to command 'stand-in'\n");
    if escalated {
        use std::os::unix::process::ExitStatusExt;
        output.status = std::process::ExitStatus::from_raw(9);
        output.stderr.extend_from_slice(b"timeout: sending signal KILL to command 'stand-in'\n");
    }
    output
}

#[test]
fn a_deadline_is_a_private_host_timeout() {
    refused(deadline(false), 0, 1);
}

#[test]
fn an_escalated_deadline_is_not_a_reference_crash() {
    refused(deadline(true), 0, 1);
}
''')
PY
status=$?
[ "$status" -eq 0 ] || exit "$status"
rustc --edition=2021 --test "$sandbox/outcomes.rs" -o "$sandbox/outcomes" || exit "$?"
"$sandbox/outcomes" --quiet || exit "$?"
echo "PASS  the shared key-field oracle counts crashes and host timeouts separately without case details"
