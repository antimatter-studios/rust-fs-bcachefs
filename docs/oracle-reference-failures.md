# Reference failures in the guest oracle tests

Every guest target that runs the reference tools under a deadline does it through
`tests/common/reference_runs.rs`: `check_oracle`, `check_oracle_key_types`,
`check_oracle_never_started`, `check_oracle_started_superblock`,
`check_oracle_reconcile_dirent`, `check_oracle_snapshot_claims` and
`read_oracle_never_started`.
`tests/scripts/reference-key-field-outcomes.sh` fails on a test target with a
deadline of its own.

`write_oracle` is outside this: it runs the reference with no deadline, so a run
that does not end blocks the job rather than failing a test (#584).

## A run that reaches the deadline

A run has two minutes. Exit status 124 identifies a deadline. The deadline wrapper
also emits its own signal diagnostics in the C locale, captured privately, so a
deadline that escalates to SIGKILL remains a timeout rather than becoming a
reference-crash finding. A killed reference without a deadline diagnostic remains
a reference fault.

A run at the deadline gave no verdict, so the same image is run again, and the run
is counted on stderr, where the tier's log keeps it whether or not the test passes:

```text
reference timeouts retried in this test binary: 2; no case details retained
```

Four runs of one image at the deadline leave nothing to judge it by. That fails the
test, giving counts and no case details:

```text
reference crashes or internal faults: 0; reference timeouts: 4; no case details retained
```

Only a deadline is repeated. A run that finishes is final whatever it says, so
a verdict that disagrees, a crash and an internal fault are never run again to get
another answer, and each still fails as it did. The guard holds this with stand-in
results: it counts the runs asked for.

### Why these are repeated rather than failed

The reference can stop making progress on an image it usually judges in under a
second, and a longer deadline does not help (#561). Repeating one such image in the
harness guest with reference checker 1.39.7, four private copies at once:

| Deadline | Runs | At the deadline | Longest run that finished |
|---|---|---|---|
| 60 s | 800 | 58 (12 to 17 per copy) | 3.14 s |
| 600 s | 183 | 10 | 0.76 s |

After a run at the deadline, the next was at it once in 58, no more often than any
other run, so runs are close to independent, and four in a row are about one in
36,000. Repeated `check_oracle` in the guest passed 10 of 10 with this change, with
3 deadlines repeated; each of those was a failed run before it.

### Why the deadline is two minutes

Some images are simply slow, which no repeat helps with if every run is. On another
image, alone in the guest, 40 runs took 14.58 s at the median and 17.24 s at most,
while a guest shared with a `check_oracle` run took one to 59.53 s, with no deadline
in 464 runs. A minute left nothing in hand there. A run that does not end costs one
minute more, and these are rare.

## What stays private

A hang is a finding about the reference, under **A crash in the reference stays
private** in `AGENTS.md`. Only the counts are reported: no word, fixture set, image,
patch, seed or tool output is published or saved for later reporting, including in
a private report. Core dumps remain disabled. Captured output is dropped, and the
owned changed image is removed when its scratch owner unwinds. The maintainer
receives the counts and decides whether and how to investigate. A future
private-retention mechanism requires an explicit change to that policy.

## Comparing the lines of two runs

`both_checkers_agree_on_state_words_below_the_required_version` holds the reference
to the same output on a changed image as on the same image unchanged, through
`comparable_lines`. That leaves out where the image is kept, the journal sequence,
how long a pass took, and the name of a pass in progress.

A pass prints its name and ends the line only when it finishes, so whatever is
printed meanwhile can follow the name on the same line, or come before it (#579).
Queue run 41795571429 failed on exactly that, both sides exit 9:

```text
check_reconcile_work...requested incompat feature reconcile (1.33) currently not enabled
requested incompat feature reconcile (1.33) currently not enabled
```

The text after the name is kept, so a line one run has and the other does not
still fails the test.

That order was seen once, in that run. Repeating the lowered copies in the guest
gave one order only: 66 images, 15,180 runs four at once, then the 11 copies at 1.20,
3,300 runs eleven at once, with exit status and every line the same on each image.
The rate is too low to give, so the comparison does not depend on it.
