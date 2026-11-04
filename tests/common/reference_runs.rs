//! How the guest targets run the reference tools under their deadline, and
//! what a run that reaches it means. Nothing here needs the reference, so
//! tests/scripts/reference-key-field-outcomes.sh drives all of it with stand-in
//! process results. docs/oracle-reference-failures.md gives the policy.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// How many runs of one image may reach the deadline before it has no verdict.
/// Measured in the guest on the image that reached it most: 58 of 800 runs, and
/// after a run at the deadline the next was at it once in 58, so four in a row
/// are about one in 36,000 (docs/oracle-reference-failures.md).
pub const RUNS_PER_IMAGE: usize = 4;

/// One run's deadline, two minutes, is far beyond a judgement (about 0.2 s an
/// image in queue run 39895568461, 59.53 s the longest measured on any) and short
/// of letting a few hangs run out the job. Its signal diagnostics are in the C
/// locale so that `reached_deadline` can read them, because a deadline that
/// escalates to SIGKILL does not exit 124.
pub const WRAPPER: &str =
    "ulimit -c 0; export LC_ALL=C; exec timeout --verbose --kill-after=5 120 bcachefs-ref \"$@\"";

static RETRIED: AtomicUsize = AtomicUsize::new(0);

fn reached_deadline(run: &Output) -> bool {
    run.status.code() == Some(124)
        || String::from_utf8_lossy(&run.stderr).contains("timeout: sending signal ")
}

/// The first of `run`'s results that finished before the deadline.
///
/// A run at the deadline gave no verdict, so the image is run again and the run
/// counted. Only a deadline is repeated: a run that finishes is returned whatever
/// it says, so a verdict that disagrees is never run away from. `RUNS_PER_IMAGE`
/// deadlines on one image leave nothing to judge it by, and fail with counts only.
pub fn finished(mut run: impl FnMut() -> Output) -> Output {
    let mut timeouts = 0;
    loop {
        let output = run();
        if !reached_deadline(&output) {
            return output;
        }
        timeouts += 1;
        assert!(
            timeouts < RUNS_PER_IMAGE,
            "reference crashes or internal faults: 0; reference timeouts: {timeouts}; no case details retained"
        );
        let retried = RETRIED.fetch_add(1, Ordering::Relaxed) + 1;
        // To the stream itself: the test harness keeps what `eprintln!` says about
        // a test that passes, and this count has to reach the log.
        let _ = writeln!(
            std::io::stderr(),
            "reference timeouts retried in this test binary: {retried}; no case details retained"
        );
    }
}

/// The reference tool's finished run with these arguments, then the image.
pub fn output(args: &[&str], image: &Path) -> Output {
    assert!(
        Path::new("/usr/local/bin/bcachefs-ref").exists(),
        "the reference tools are required: run `chore test:vm`"
    );
    finished(|| {
        Command::new("sh")
            .args(["-c", WRAPPER, "reference"])
            .args(args)
            .arg(image)
            .output()
            .expect("chore test:vm provides the reference tools")
    })
}

/// What two runs of the reference on one image can be compared by: the lines of
/// `text`, without where the image is kept, the journal sequence, how long a pass
/// took, and the name of a pass in progress.
///
/// A pass names itself and ends its line only when it finishes, so whatever is
/// printed meanwhile can follow its name on the same line, or come before it
/// (queue runs 39847363486 and 41795571429). The text after the name is kept, so a
/// finding one run has and the other does not still differs.
pub fn comparable_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| {
            !line.contains(".img") && !line.contains("journal seq") && !line.contains("seconds)")
        })
        .map(without_pass_in_progress)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn without_pass_in_progress(line: &str) -> &str {
    match line.split_once("...") {
        Some((pass, rest))
            if !pass.is_empty()
                && pass.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                }) =>
        {
            rest
        }
        _ => line,
    }
}
