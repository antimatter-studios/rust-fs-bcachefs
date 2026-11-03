#!/usr/bin/env python3
"""kernel-oracle-hang.py CONSOLE RECORD -- whose hang a stopped first boot was.

Prints one line for scripts/kernel-oracle.sh to put in its failure (#514), read
only from what the guest wrote: CONSOLE is its console, RECORD the second serial
port. It decides nothing about the build, which fails either way.

The first boot's disks hold only what the reference formatter and the reference
kernel module wrote: this crate's writer first runs after that boot, and its
reader never runs in the guest. So the question is the module or the host under
it. The module's report of a stuck journal
answers that when it gives the journal's write counters:

- every write on disk: the journal is waiting on the module's own reclaim, and
  nothing the disk could still deliver would free it, so the hang is the module's;
- writes outstanding: what a disk that stopped answering looks like, so the runner
  stays a suspect.

Tasks blocked in the module with no such report leave both suspects, because a
stopped disk blocks them too. With no report at all the console accuses nobody,
and the time of its last message and the record's last line say how far the guest
got: a console still printing at the limit is a slow guest, not a hung one.
"""

import re
import sys

STUCK = re.compile(r"bcachefs \((\w+)\): Journal stuck\? .*\berr (\w+)")
SEQ = re.compile(r"\]\s+seq:\s+(\d+)\s*$")
SEQ_ONDISK = re.compile(r"\]\s+seq_ondisk:\s+(\d+)\s*$")
BLOCKED = re.compile(r"INFO: task (.+):\d+ blocked for more than \d+ seconds")
STACK_END = "</TASK>"
MODULE_FRAME = "[bcachefs]"
TIMESTAMP = re.compile(r"^\[\s*(\d+\.\d+)\]")
RECORD_LINE = re.compile(r"ORACLE ([a-z][a-z0-9_.=-]*: .*)$")
RECORD_LIMIT = 80


def lines_of(path):
    """The file's lines without their ends; a file never written has none."""
    try:
        with open(path, errors="replace") as f:
            return [line.rstrip("\r\n") for line in f]
    except FileNotFoundError:
        return []


def last_stuck_report(console):
    """(disk, error, seq, seq_ondisk) of the last report; a counter it lacks is None."""
    report = None
    for line in console:
        stuck = STUCK.search(line)
        if stuck:
            report = [stuck.group(1), stuck.group(2), None, None]
        elif report:
            for index, pattern in ((2, SEQ), (3, SEQ_ONDISK)):
                found = pattern.search(line)
                if found and report[index] is None:
                    report[index] = int(found.group(1))
    return report


def tasks_blocked_in_module(console):
    """The name of each task reported blocked with a module frame on its stack."""
    names = []
    task = None
    for line in console:
        blocked = BLOCKED.search(line)
        if blocked:
            task = blocked.group(1)
        elif task and MODULE_FRAME in line:
            if task not in names:
                names.append(task)
            task = None
        elif STACK_END in line:
            task = None
    return names


def how_far(console, record):
    """How far the guest got, as a clause."""
    times = [m.group(1) for m in map(TIMESTAMP.search, console) if m]
    last_time = (
        f"its last console message was at {float(times[-1]):.1f} s"
        if times
        else "its console has no timed message"
    )
    lines = [m.group(1) for m in map(RECORD_LINE.search, record) if m]
    last_line = (
        f"its record ended at '{lines[-1][:RECORD_LIMIT]}'"
        if lines
        else "its record is empty"
    )
    return f"{last_time}, {last_line}"


def report(console, record):
    if not console:
        return "the guest printed nothing on its console: the runner, or the guest before its kernel ran"
    blocked = tasks_blocked_in_module(console)
    names = ", ".join(blocked)
    stuck = last_stuck_report(console)
    if stuck:
        disk, error, seq, seq_ondisk = stuck
        if seq is None or seq_ondisk is None:
            return (
                f"the reference kernel module reported err {error} on {disk}, without saying "
                "whether its journal writes reached the disk: the module, or the runner under it"
            )
        if seq > seq_ondisk:
            return (
                f"the reference kernel module reported err {error} on {disk} with "
                f"{seq - seq_ondisk} journal writes not on disk: the disk under it, so the runner, "
                "or the module"
            )
        tasks = f", tasks blocked in it: {names}" if blocked else ""
        return (
            f"the reference kernel module hung: err {error} on {disk} with every journal write "
            f"on disk{tasks}; only the reference formatter and the module had written this boot's disks"
        )
    if blocked:
        return (
            f"tasks blocked in the reference kernel module, with no journal report: {names}: "
            "the module, or the disk under it, so the runner"
        )
    return (
        f"no report from the reference kernel module: {how_far(console, record)}: "
        "a slow or stopped guest, on the runner or in its kernel"
    )


if __name__ == "__main__":
    print(report(lines_of(sys.argv[1]), lines_of(sys.argv[2])))
