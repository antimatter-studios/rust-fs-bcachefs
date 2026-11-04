# Reference failures in the key-field oracle

The key-field oracle tests in `tests/check_oracle.rs` share
`reference_on_key_field`. This includes the snapshot, subvolume, dirent,
older-version and replicas-accounting comparisons that call it directly or
through another helper. A failure is classified here, before any caller can
print a case or the reference's output.

Each failed invocation reports two counts: reference crashes or internal
faults, and reference timeouts (a slow-host outcome, not a reference finding).
The counts describe that invocation, not the entire test suite. Exit status
124 identifies a deadline. The deadline wrapper also emits its own signal
diagnostics in the C locale, captured privately, so a deadline that escalates
to SIGKILL remains a timeout rather than becoming a reference-crash finding.
A killed reference without a deadline diagnostic remains a reference fault.
Neither diagnostic nor reference output is included in the failure message.

The existing accepted statuses and internal-fault checks remain required.
The assertion now also explicitly rejects a deadline diagnostic, and its
public message separates the two failure classes. Neither class is accepted
as a checker comparison, retried, or skipped. The test still stops at the
first such invocation; it does not compare subsequent cases in that run.
Continuing or retrying would change the test policy and is not part of this
diagnostic fix.

No case details are retained, including in a private report. This is the
deliberate retention policy required by **A crash in the reference stays
private** in `AGENTS.md`: no word, fixture set, image, patch, seed or tool
output is published or saved for later reporting. Core dumps remain disabled.
Captured output is dropped on assertion failure, and the owned changed image
is removed when its scratch owner unwinds. The maintainer receives only the
counts and decides whether and how to investigate privately. A future
private-retention mechanism requires an explicit change to that policy;
this test does not create one.

`tests/scripts/reference-key-field-outcomes.sh` runs the actual classifier
with stand-in process results, without fixtures, reference tools or a host
GNU timeout dependency. It also checks the deadline wrapper's C-locale
diagnostic configuration. It covers accepted verdicts, unexpected exits,
a signalled exit, internal fault markers, ordinary deadlines and deadlines
requiring SIGKILL, and requires count-only failure messages.
