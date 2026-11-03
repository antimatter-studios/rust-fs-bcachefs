import json
import os
from pathlib import Path
import signal
import subprocess
import time


def trace_progress_into(env, log):
    """Make a supervisor started with env trace to log, not to its stderr."""
    env["BASH_ENV"] = str(Path(__file__).with_name("progress-trace.sh"))
    env["SUPERVISOR_PROGRESS_FD"] = str(log.fileno())


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
