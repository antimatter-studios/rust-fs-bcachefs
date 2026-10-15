#!/usr/bin/env python3
"""Differential fuzzing's judge and classifier (#164).

scripts/differential-fuzz.sh runs both, after the emitter
(examples/differential.rs) has written OUT/cases.tsv and OUT/patches/.

  differential-fuzz.py judge OUT FIXTURES (--container NAME | --reference CMD)
      [--jobs N] [--timeout S]
  differential-fuzz.py classify OUT --baseline FILE --report FILE

JUDGE lays each case's patch over its own sparse copy of the fixture and
runs the reference checker on it (`fsck -n`), in the container the shell
script started (or, for the tests, a stand-in command given the image's
path). It writes OUT/ref/<id>.status and, unless the reference crashed or
hung, OUT/ref/<id>.txt.

CLASSIFY puts every case in one class -- both clean, both found something,
the reference found something this crate called clean, this crate found
something the reference did not, the reference crashed or hung, this crate
panicked -- and keys each disagreement by its direction, the kind of
structure the case changed and a signature of what was found. A key not in
the committed baseline is a NEW class and fails the run.

What the reference found is read from what it PRINTED, not only from its
exit status: under -n it exits 0 after an error it repaired in memory. Any
line that is not one of the lines it prints on a clean image is a finding.
A fixture's control (the fixture unchanged) widens that set for its own
cases: a note the reference prints on the unchanged fixture (block4k's
bitmap note, say) is that fixture's normal, not something a case found.
Its exit status is that fixture's normal status, too.

DISCLOSURE (AGENTS.md). A case on which the reference crashed, hung or
reported an internal fault is a finding about the reference, not this
crate. Its output is never kept, its patch is deleted as soon as it is
judged, and the report gives only how many there were: no fixture, no seed,
nothing that would reproduce it.
"""

import argparse
import concurrent.futures
import os
import re
import struct
import subprocess
import sys
from collections import Counter, OrderedDict

# The lines the reference checker prints on a clean image (the fixtures'
# NAME.fsck.txt, recorded when they are built). Anything else it prints is
# a finding.
NORMAL = [
    re.compile(p)
    for p in (
        r"^Running userspace offline fsck$",
        r"^Using encoding defined by superblock: \S+$",
        r"^starting version \S+ \(\d+\.\d+\)$",
        r"^\s*with options: \S*$",
        r"^recovering from clean shutdown, journal seq \d+$",
        r"^Journal keys: \d+ read, \d+ after sorting and compacting$",
        r"^going read-write$",
        r"^clean shutdown complete, journal seq \d+$",
        r"^[a-z_]+\.\.\. done \(\d+ seconds\)$",
        # A pass whose progress line is broken by a note, then its `done`.
        r"^[a-z_]+\.\.\.scanning for missing backpointers in \d+/\d+ buckets, \d+ buckets with no backpointers$",
        r"^ done \(\d+ seconds\)$",
        r"^$",
    )
]

# A line naming a place in the reference's own code, or an internal check
# that fired: the reference faulted. Such output is treated as a crash's --
# never kept, never printed -- because an assertion can quote the line it
# checks, and nobody working here reads the reference source in any form.
INTERNAL = re.compile(
    r"\.(?:c|h|rs):\d+|\bBUG\b|[Aa]ssertion|assertion failed|[Bb]acktrace|Call Trace"
    r"|panicked at|Kernel panic"
)

ZERO = bytes(1 << 20)

CLASSES = OrderedDict(
    [
        ("both-clean", "both clean"),
        ("both-found", "both found something"),
        ("ref-only", "the reference found something; this crate called it clean"),
        ("ours-only", "this crate found something; the reference did not"),
        ("ref-crashed", "the reference crashed, hung or faulted (counts only)"),
        ("ours-panicked", "this crate panicked"),
    ]
)


def read_cases(out):
    cases = []
    with open(os.path.join(out, "cases.tsv"), encoding="utf-8") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) != 8:
                raise SystemExit(f"cases.tsv: a line without 8 fields: {line!r}")
            keys = (
                "id",
                "fixture",
                "seed",
                "structure",
                "what",
                "ours",
                "signature",
                "detail",
            )
            cases.append(dict(zip(keys, parts)))
    return cases


# ---------------------------------------------------------------- judge


def nonzero_chunks(path):
    """The fixture's non-zero MiB chunks and its size: a copy writes these
    and leaves the rest a hole."""
    chunks = []
    with open(path, "rb") as f:
        at = 0
        while True:
            b = f.read(len(ZERO))
            if not b:
                break
            if b != ZERO[: len(b)]:
                chunks.append((at, b))
            at += len(b)
    return chunks, at


def lay(image, chunks, size, patch):
    with open(image, "wb") as f:
        for at, b in chunks:
            f.seek(at)
            f.write(b)
        f.truncate(size)
        with open(patch, "rb") as p:
            data = p.read()
        i = 0
        while i < len(data):
            at, n = struct.unpack_from("<QI", data, i)
            i += 12
            f.seek(at)
            f.write(data[i : i + n])
            i += n


def crashed(status):
    """A timeout (124), or a death by signal: 128 and above, as the shell
    reports it, or negative, as Python does."""
    return status == 124 or status >= 128 or status < 0


def judge_one(case, out, chunks, size, args):
    cid = case["id"]
    scratch = os.path.join(out, "scratch", cid + ".img")
    patch = os.path.join(out, "patches", cid + ".patch")
    lay(scratch, chunks, size, patch)
    if args.container:
        cmd = [
            "docker",
            "exec",
            args.container,
            "timeout",
            "-k",
            "5",
            str(args.timeout),
            "bcachefs",
            "fsck",
            "-n",
            "/work/scratch/" + cid + ".img",
        ]
    else:
        cmd = [args.reference, scratch]
    # In the container, `timeout` stops the checker and this is a backstop
    # for docker itself; a stand-in has only this.
    limit = args.timeout + 30 if args.container else args.timeout
    try:
        r = subprocess.run(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=limit,
            check=False,
        )
        status, text = r.returncode, r.stdout.decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        status, text = 124, ""
    os.remove(scratch)
    if crashed(status) or any(INTERNAL.search(l) for l in text.splitlines()):
        # Kept as a count and nothing else (DISCLOSURE above).
        status = status if crashed(status) else 134
        text = None
        os.remove(patch)
    ref = os.path.join(out, "ref", cid)
    with open(ref + ".status", "w") as f:
        f.write(f"{status}\n")
    if text is not None:
        with open(ref + ".txt", "w", encoding="utf-8") as f:
            f.write(text)
    return status


def judge(args):
    out, fixtures = args.out, args.fixtures
    for d in ("scratch", "ref"):
        os.makedirs(os.path.join(out, d), exist_ok=True)
    cases = read_cases(out)
    by_fixture = OrderedDict()
    for c in cases:
        by_fixture.setdefault(c["fixture"], []).append(c)
    broken = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for fixture, group in by_fixture.items():
            chunks, size = nonzero_chunks(os.path.join(fixtures, fixture))
            for status in pool.map(
                lambda c, chunks=chunks, size=size: judge_one(
                    c, out, chunks, size, args
                ),
                group,
            ):
                # docker's own failures: no container, no such command.
                if status in (125, 126, 127):
                    broken += 1
    if broken:
        print(
            f"differential: the reference checker did not run on {broken} case(s) "
            "(status 125-127: docker or the command itself failed)",
            file=sys.stderr,
        )
        return 2
    print(f"differential: the reference checker judged {len(cases)} cases")
    return 0


# ---------------------------------------------------------------- classify


def fold(line):
    """A line without its numbers: lower-cased, the image's path, numbers
    and positions folded to #, so one kind of line reads the same on every
    image and at every place."""
    s = line.strip().lower()
    s = re.sub(r"\S*\.img\b", "<image>", s)
    s = re.sub(r"\b0x[0-9a-f]+\b", "#", s)
    s = re.sub(r"\b[0-9a-f]{8,}\b", "#", s)
    s = re.sub(r"\d+", "#", s)
    s = re.sub(r"#(?:[:/.]#)+", "#", s)
    return re.sub(r"\s+", " ", s)


# A pass's progress, `name...`, and what was printed after it on its line.
PROGRESS = re.compile(r"^[a-z_]+\.\.\.(.+)$")


def usual(line):
    """One of the reference's usual lines, or a pass's progress broken by
    one: on aged.img seed 3 (exit 0) it printed `check_allocations...` and
    then its own `Running userspace offline fsck` on the same line. A
    finding printed after a pass's progress is still a finding."""
    if any(p.match(line) for p in NORMAL):
        return True
    rest = PROGRESS.match(line)
    return bool(rest) and any(p.match(rest.group(1)) for p in NORMAL)


def findings(text, normal=frozenset()):
    """The lines that are not the reference's usual ones, nor (folded) one
    it printed on the unchanged fixture."""
    return [
        l.rstrip()
        for l in text.splitlines()
        if not usual(l.rstrip()) and fold(l) not in normal
    ]


def signature(lines, status):
    """What kind of thing the reference found, without the numbers: its
    first unindented finding (the first at all when every one is indented),
    folded."""
    if not lines:
        return f"exit {status}"
    top = next((l for l in lines if l[:1] not in (" ", "\t")), lines[0])
    return fold(top)[:100]


def read_ref(out, cid):
    """A case's exit status and output, or None for one never judged; the
    output is None when the reference crashed, hung or faulted."""
    ref = os.path.join(out, "ref", cid)
    try:
        with open(ref + ".status") as f:
            status = int(f.read().strip())
    except FileNotFoundError:
        return None
    if crashed(status):
        return status, None
    with open(ref + ".txt", encoding="utf-8") as f:
        return status, f.read()


def judged(out, case, normal=None):
    """The reference's verdict on a case. `normal` is its fixture's control,
    `{"lines": folded notes, "status": exit status}`: a note it printed there
    is not a finding here, and neither is the same exit status."""
    got = read_ref(out, case["id"])
    if got is None:
        return None
    status, text = got
    if text is None:
        return {"verdict": "crashed", "status": status}
    normal = normal or {"lines": frozenset(), "status": 0}
    lines = findings(text, normal["lines"])
    return {
        "verdict": "found" if lines or status not in (0, normal["status"]) else "clean",
        "status": status,
        "lines": lines,
        "signature": signature(lines, status),
    }


def controls(out, cases):
    """Each fixture's control as `judged` takes it, read from what the
    reference printed on the fixture unchanged. A fixture whose control
    was never judged, or crashed, has none: its control fails the run."""
    normal = {}
    for c in cases:
        if c["structure"] != "control":
            continue
        got = read_ref(out, c["id"])
        if got is None or got[1] is None:
            continue
        status, text = got
        normal[c["fixture"]] = {
            "lines": frozenset(fold(l) for l in findings(text)),
            "notes": findings(text),
            "status": status,
        }
    return normal


def read_baseline(path):
    known = OrderedDict()
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.rstrip("\n")
            if not line.strip() or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 3 or parts[0] not in ("ref-only", "ours-only"):
                raise SystemExit(
                    f"{path}:{n}: not `ref-only|ours-only<TAB>structure<TAB>signature[<TAB>note]`"
                )
            known[(parts[0], parts[1], parts[2])] = parts[3] if len(parts) > 3 else ""
    return known


def baseline_key(known, key):
    """The baseline entry covering `key`: the same, or the same with `*`
    for its structure."""
    if key in known:
        return key
    wild = (key[0], "*", key[2])
    return wild if wild in known else None


def example(case, ref):
    seed = "control" if case["seed"] == "-" else f"seed {case['seed']}"
    out = [f"- `{case['fixture']}` {seed}: {case['what']}"]
    if case["ours"] != "clean":
        out.append(f"  - this crate: `{case['signature']}`: {case['detail']}")
    else:
        out.append("  - this crate: clean")
    if ref["verdict"] == "clean":
        out.append(f"  - the reference: clean (exit {ref['status']})")
    else:
        out.append(f"  - the reference (exit {ref['status']}):")
        out.append("")
        out.append("    ```")
        out.extend("    " + l for l in ref["lines"][:12])
        if len(ref["lines"]) > 12:
            out.append(f"    ... {len(ref['lines']) - 12} more lines")
        out.append("    ```")
    return out


def classify(args):
    out = args.out
    cases = read_cases(out)
    known = read_baseline(args.baseline)
    counts = Counter()
    classes = OrderedDict()  # key -> [cases]
    controls_bad, panics, unjudged, controls_crashed = [], [], 0, 0
    normal = controls(out, cases)
    for case in cases:
        if case["ours"] == "panicked":
            counts["ours-panicked"] += 1
            panics.append(case)
            continue
        ref = judged(out, case, normal.get(case["fixture"]))
        if ref is None:
            unjudged += 1
            continue
        case["ref"] = ref
        if ref["verdict"] == "crashed":
            counts["ref-crashed"] += 1
            # Counted, never named, even for a fixture unchanged.
            controls_crashed += case["structure"] == "control"
            continue
        theirs, ours = ref["verdict"] == "found", case["ours"] == "found"
        cls = {
            (False, False): "both-clean",
            (True, True): "both-found",
            (True, False): "ref-only",
            (False, True): "ours-only",
        }[(theirs, ours)]
        counts[cls] += 1
        if case["structure"] == "control" and cls != "both-clean":
            controls_bad.append(case)
            continue
        if cls in ("ref-only", "ours-only"):
            sig = ref["signature"] if cls == "ref-only" else case["signature"]
            classes.setdefault((cls, case["structure"], sig), []).append(case)

    new = [k for k in classes if baseline_key(known, k) is None]
    seen = {baseline_key(known, k) for k in classes}
    unseen = [k for k in known if k not in seen]
    n_controls = sum(1 for c in cases if c["structure"] == "control")

    report = [f"# Differential fuzzing: {len(cases)} cases", ""]
    report.append(
        f"Reference checker {args.version}; {n_controls} controls (each fixture unchanged), "
        "the rest one change each."
    )
    # Which fixtures the cases were cut from: the same seeds reach different
    # nodes in a rebuilt fixture, so a class is reproducible only with these.
    fixtures_from = os.environ.get("DIFFERENTIAL_FIXTURES_FROM", "")
    report += ["", f"Fixtures: {fixtures_from or 'unrecorded (a local run)'}."]
    report += ["", "| class | cases |", "|---|---|"]
    report += [f"| {label} | {counts[c]} |" for c, label in CLASSES.items()]
    if unjudged:
        report.append(f"| not judged (the judge did not finish) | {unjudged} |")
    noted = [(f, n) for f, n in normal.items() if n["notes"] or n["status"] != 0]
    if noted:
        report += [
            "",
            "## What the reference prints on a fixture unchanged",
            "",
            "Normal for that fixture's cases: not counted as a finding in any of them.",
            "",
        ]
        for f, n in noted:
            report.append(f"- `{f}` (exit {n['status']}):")
            report += (
                ["", "    ```"] + ["    " + l for l in n["notes"][:12]] + ["    ```"]
            )
    if controls_bad:
        report += [
            "",
            "## Controls that are not clean to both",
            "",
            (
                "Each fixture unchanged must be clean to both checkers; one that is not "
                "means the harness, or the fixture, is wrong."
            ),
            "",
        ]
        for c in controls_bad:
            report += example(c, c["ref"])
    if panics:
        report += ["", "## This crate panicked", ""]
        for c in panics:
            report.append(
                f"- `{c['fixture']}` seed {c['seed']}: {c['signature']}: {c['detail']}"
            )
    if new:
        report += [
            "",
            "## New disagreement classes",
            "",
            (
                "Not in `fuzz/differential-baseline.tsv`. Each is this crate's to fix, or, "
                "once understood and tracked by an issue, a line of the baseline."
            ),
            "",
        ]
        for k in new:
            report.append(f"### {k[0]} / {k[1]} / `{k[2]}`: {len(classes[k])} case(s)")
            report.append("")
            report.append(f"Baseline line: `{k[0]}\\t{k[1]}\\t{k[2]}\\t<issue>`")
            report.append("")
            for c in classes[k][:2]:
                report += example(c, c["ref"])
            report.append("")
    if classes:
        report += [
            "",
            "## Every disagreement class",
            "",
            "| direction | structure | signature | cases | baseline |",
            "|---|---|---|---|---|",
        ]
        for k, cs in classes.items():
            b = baseline_key(known, k)
            note = "NEW" if b is None else (known[b] or "known")
            report.append(f"| {k[0]} | {k[1]} | `{k[2]}` | {len(cs)} | {note} |")
        known_with_examples = [k for k in classes if k not in new]
        if known_with_examples:
            report += ["", "## Examples of the known classes", ""]
            for k in known_with_examples:
                report.append(f"### {k[0]} / {k[1]} / `{k[2]}`")
                report.append("")
                report += example(classes[k][0], classes[k][0]["ref"])
                report.append("")
    if unseen:
        report += ["", "## Baseline classes not seen in this run", ""]
        report += [
            f"- {k[0]} / {k[1]} / `{k[2]}` ({known[k] or 'no note'})" for k in unseen
        ]
    with open(args.report, "w", encoding="utf-8") as f:
        f.write("\n".join(report) + "\n")

    print(f"differential: {len(cases)} cases, reference checker {args.version}")
    for c, label in CLASSES.items():
        print(f"  {counts[c]:5d}  {label}")
    print(
        f"  {len(classes)} disagreement classes: {len(classes) - len(new)} known, {len(new)} new"
    )
    for k in new:
        c = classes[k][0]
        print(
            f"  NEW {k[0]} / {k[1]} / {k[2]}  ({len(classes[k])} cases, e.g. {c['id']})"
        )
    failed = []
    if new:
        failed.append(f"{len(new)} new disagreement class(es)")
    if panics:
        failed.append(f"this crate panicked in {len(panics)} case(s)")
    if controls_bad:
        failed.append(
            f"{len(controls_bad)} control(s) not clean to both: "
            + ", ".join(c["fixture"] for c in controls_bad)
        )
    if controls_crashed:
        failed.append(f"the reference crashed or hung on {controls_crashed} control(s)")
    if unjudged:
        failed.append(f"{unjudged} case(s) never judged")
    if failed:
        print(
            "differential: FAILED: " + "; ".join(failed) + f" (report: {args.report})"
        )
        return 1
    print(f"differential: no new disagreement (report: {args.report})")
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = p.add_subparsers(dest="command", required=True)
    j = sub.add_parser("judge")
    j.add_argument("out")
    j.add_argument("fixtures")
    how = j.add_mutually_exclusive_group(required=True)
    how.add_argument(
        "--container", help="run `bcachefs fsck -n` in this running container"
    )
    how.add_argument("--reference", help="a stand-in, given the image's path (tests)")
    j.add_argument("--jobs", type=int, default=4)
    j.add_argument("--timeout", type=int, default=60)
    c = sub.add_parser("classify")
    c.add_argument("out")
    c.add_argument("--baseline", required=True)
    c.add_argument("--report", required=True)
    c.add_argument("--version", default="(unknown version)")
    args = p.parse_args()
    return judge(args) if args.command == "judge" else classify(args)


if __name__ == "__main__":
    sys.exit(main())
