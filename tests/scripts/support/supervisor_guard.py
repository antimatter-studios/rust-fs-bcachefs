import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time


def trace_progress_into(env, log):
    """Make a supervisor started with env trace to log, not to its stderr."""
    env["BASH_ENV"] = str(Path(__file__).with_name("progress-trace.sh"))
    env["SUPERVISOR_PROGRESS_FD"] = str(log.fileno())


def require_clean_capture():
    """Fail at once, naming the shell, if a supervisor's trace would mislead.

    Run with the trace the guards use, a function's redirected output must keep
    its exact bytes and the trace must still arrive. Otherwise a guard reports a
    defect in fixture output, or a stuck supervisor, that the shell caused.
    """
    with tempfile.TemporaryDirectory(prefix="supervisor-capture-control.") as name:
        root = Path(name)
        sample = root / "sample.sh"
        sample.write_text(
            'sample() { printf "native output\\n"; printf "native stderr\\n" >&2; }\n'
            'sample >"$1" 2>&1\n'
        )
        capture, trace = root / "capture", root / "trace"
        env = dict(os.environ)
        with trace.open("w") as log:
            trace_progress_into(env, log)
            subprocess.run(
                ["bash", str(sample), str(capture)],
                env=env,
                stdout=log,
                stderr=log,
                pass_fds=(log.fileno(),),
                check=False,
            )
        captured = capture.read_text() if capture.exists() else None
        traced = trace.read_text()
    if captured != "native output\nnative stderr\n":
        lacks = f"writes its trace into a native capture: {captured!r}"
    elif "native output" not in traced:
        lacks = "gives no trace, so a guard has no progress to follow"
    else:
        return
    version = subprocess.run(
        ["bash", "-c", 'echo "$BASH_VERSION"'],
        check=False,
        capture_output=True,
        text=True,
    ).stdout.strip()
    sys.exit(
        f"FAIL  shell capability: {shutil.which('bash')} ({version}) {lacks}. "
        "Put a bash first on PATH that traces to a descriptor of its own."
    )


def signals_at_default():
    for sig in (signal.SIGHUP, signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, signal.SIG_DFL)


def events(case):
    path = case / "events"
    return (
        [json.loads(line) for line in path.read_text().splitlines()]
        if path.exists()
        else []
    )


def in_session(pid, sid):
    try:
        return os.getsid(pid) == sid
    except (ProcessLookupError, PermissionError):
        return False


def keep_failed_logs(guard, path, cases):
    """Keep what each case recorded, then name where, for a guard that failed.

    A guard's temporary directory is deleted as its failure unwinds, and with
    it every supervisor log no assertion happened to quote. The tier prints only
    its verdict, so the log goes to a file its job uploads, not to the output.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("wb") as kept:
        for case in cases:
            for name in ("log", "events"):
                part = case / name
                kept.write(f"== {part}\n".encode())
                if part.exists():
                    kept.write(part.read_bytes())
    print(f"{guard}: diagnostics: {path}", file=sys.stderr, flush=True)


class SupervisorProgress:
    def __init__(self, sandbox, idle_limit, case_limit):
        self.sandbox = sandbox
        self.idle_limit = idle_limit
        self.case_limit = case_limit

    def diagnose(self, case):
        lines = [f"--- what {case.name} last traced"]
        lines += (case / "log").read_text(errors="replace").splitlines()[-40:]
        lines += ["--- its last events"] + [
            json.dumps(event) for event in events(case)[-20:]
        ]
        table = subprocess.run(
            ["ps", "-axo", "pid,ppid,pgid,stat,etime,command"],
            check=False,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
        lines += ["--- its processes still running"] + [
            line[:200] for line in table if str(self.sandbox) in line
        ][:40]
        return "\n".join(lines)

    def wait_for(self, case, what, done):
        began = moved = time.monotonic()
        seen = None
        while not done():
            now = time.monotonic()
            mark = [
                path.stat().st_size if path.exists() else 0
                for path in (case / "log", case / "events")
            ]
            if mark != seen:
                seen, moved = mark, now
            if now - moved > self.idle_limit:
                raise AssertionError(
                    f"{case.name}: waited {now - began:.0f}s for {what}, "
                    f"with no progress for {now - moved:.0f}s\n{self.diagnose(case)}"
                )
            if now - began > self.case_limit:
                raise AssertionError(
                    f"{case.name}: waited {now - began:.0f}s for {what}, "
                    f"and the supervisor is still running commands\n{self.diagnose(case)}"
                )
            time.sleep(0.01)

    def await_ready(self, proc, case):
        def ready():
            if (case / "ready").exists():
                return True
            assert proc.poll() is None, (
                f"{case.name}: the supervisor exited with status "
                f"{proc.returncode} before its child was ready\n{self.diagnose(case)}"
            )
            return False

        self.wait_for(case, "the held child's ready file", ready)


if __name__ == "__main__":
    require_clean_capture()
