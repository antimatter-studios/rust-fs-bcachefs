#!/usr/bin/env bash
# The nightly differential run reads nothing that expires (#228, #386).
#
# The differential job once `needs:` the pinned default-74 jobs, and both
# fetched CI artifacts by number. Artifacts always expire: when fixtures
# artifact 11977252185 did, on 2026-10-22, the pinned job failed at its fetch,
# the differential job was skipped behind it, and its "File a failed scheduled
# run" step never ran. Every night from then on was dead without one report.
#
# So, for a scheduled run and for a dispatch left at its defaults (both judge
# the newest green main run's fixtures), with each `if:` evaluated as GitHub
# evaluates it:
#   - the differential job runs;
#   - no job or step that runs names a CI run or artifact by number, in its
#     script or in its environment, since a numbered artifact is a fetch that
#     fails once its retention runs out;
#   - a failed scheduled run still reaches its issue.
# The pinned regressions keep their numbers; they run only where their pinned
# fixtures are judged.
#
#   bash tests/scripts/differential-nightly-has-no-pinned-input.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fail() { echo "FAIL  $*" >&2; exit 1; }

command -v python3 >/dev/null 2>&1 || fail "python3 is required"
python3 -c 'import yaml' 2>/dev/null ||
    fail "the python3 yaml module is required (pip install pyyaml)"

python3 - "$REPO/.github/workflows/differential.yml" <<'PY' || exit 1
import re, sys, yaml

doc = yaml.safe_load(open(sys.argv[1])) or {}
# PyYAML reads the bare key `on` as True.
triggers = doc.get("on", doc.get(True)) or {}
jobs = doc.get("jobs") or {}
if "differential" not in jobs:
    sys.exit("FAIL  differential.yml has no differential job")
if "schedule" not in triggers:
    sys.exit("FAIL  differential.yml has no schedule")

STATUS = ("success", "failure", "cancelled", "always")
NUMBERED = re.compile(r"(?<![\w.])\d{9,}(?![\w.])")


def evaluate(expr, ctx, status):
    """An `if:` or `${{ }}` expression, as GitHub evaluates it: && and ||
    return an operand, ! negates, a missing value reads as '', and a status
    function answers from `status`."""
    expr = expr.strip()
    if expr.startswith("${{") and expr.endswith("}}"):
        expr = expr[3:-2]
    py = re.sub(r"\bneeds\.\*\.result\b", "_all", expr)
    py = re.sub(r"\bneeds\.([\w-]+)\.result\b", lambda m: "_need(%r)" % m.group(1), py)
    py = re.sub(r"\b(%s)\(\)" % "|".join(STATUS), lambda m: "_st(%r)" % m.group(1), py)
    py = re.sub(r"\bcontains\(", "_contains(", py)
    py = re.sub(r"\bformat\(", "_fmt(", py)
    py = re.sub(
        r"\b(github|inputs|env|runner)((?:\.[\w-]+)+)",
        lambda m: "_ctx(%r)" % (m.group(1) + m.group(2)),
        py,
    )
    py = py.replace("&&", " and ").replace("||", " or ")
    py = re.sub(r"!(?!=)", " not ", py)
    py = py.replace("true", "True").replace("false", "False")

    def lookup(path):
        value = ctx
        for key in path.split("."):
            value = value.get(key) if isinstance(value, dict) else None
        return "" if value is None else value

    def contains(haystack, needle):
        if isinstance(haystack, list):
            return needle in haystack
        return str(needle).lower() in str(haystack).lower()

    names = {
        "__builtins__": {},
        "_all": list(ctx["needs"].values()),
        "_need": lambda job: ctx["needs"].get(job, ""),
        "_st": lambda name: status[name],
        "_contains": contains,
        "_fmt": lambda t, *a: re.sub(r"\{(\d+)\}", lambda m: str(a[int(m.group(1))]), t),
        "_ctx": lookup,
        "True": True,
        "False": False,
    }
    return eval(py, names)


def runs(expr, ctx, status):
    """An `if:` with no status function is ANDed with success()."""
    if expr is None:
        return status["success"]
    text = str(expr)
    if not re.search(r"\b(%s)\(\)" % "|".join(STATUS), text):
        if not status["success"]:
            return False
    return bool(evaluate(text, ctx, status))


def values(env, ctx):
    for key, value in (env or {}).items():
        text = str(value)
        if "${{" in text:
            text = re.sub(r"\$\{\{(.*?)\}\}", lambda m: str(evaluate(m.group(1), ctx, OK)), text)
        yield key, text


OK = {"success": True, "failure": False, "cancelled": False, "always": True}
FAILED = {"success": False, "failure": True, "cancelled": False, "always": True}

defaults = {
    name: str(spec.get("default", ""))
    for name, spec in ((triggers.get("workflow_dispatch") or {}).get("inputs") or {}).items()
}
events = {
    "schedule": {},
    "workflow_dispatch at its defaults": {
        k: (v.lower() == "true" if v.lower() in ("true", "false") else v)
        for k, v in defaults.items()
    },
}

problems = []
for event, inputs in events.items():
    event_name = event.split()[0]
    results, reported = {}, False
    # Jobs in order of their needs, each one's result as GitHub records it.
    pending = dict(jobs)
    while pending:
        ready = [
            n for n, j in pending.items()
            if all(d in results for d in ([j.get("needs")] if isinstance(j.get("needs"), str) else j.get("needs") or []))
        ]
        if not ready:
            sys.exit("FAIL  the jobs' needs form a cycle")
        for name in ready:
            job = pending.pop(name)
            needs = job.get("needs") or []
            needs = [needs] if isinstance(needs, str) else needs
            ctx = {
                "github": {"event_name": event_name, "ref": "refs/heads/main", "repository": "o/r"},
                "inputs": inputs,
                "env": {k: str(v) for k, v in (doc.get("env") or {}).items()},
                "runner": {"temp": "/tmp"},
                "needs": {d: results[d] for d in needs},
            }
            status = dict(OK, success=all(results[d] == "success" for d in needs),
                          failure=any(results[d] == "failure" for d in needs))
            if not runs(job.get("if"), ctx, status):
                results[name] = "skipped"
                continue
            before = len(problems)
            ctx["env"].update(values(job.get("env"), ctx))
            for key, text in values(job.get("env"), ctx):
                if NUMBERED.search(text):
                    problems.append("%s: job %s's env %s names a numbered run or artifact: %s" % (event, name, key, text))
            for step in job.get("steps") or []:
                label = step.get("name") or step.get("uses") or "a step"
                ran = [s for s in (OK, FAILED) if runs(step.get("if"), ctx, s)]
                if not ran:
                    continue
                if FAILED in ran and "gh issue" in str(step.get("run", "")):
                    reported = True
                texts = [("run", str(step.get("run", "")))]
                texts += [("env " + k, t) for k, t in values(step.get("env"), ctx)]
                texts += [("with " + k, str(v)) for k, v in (step.get("with") or {}).items()]
                for where, text in texts:
                    hit = NUMBERED.search(text)
                    if hit:
                        problems.append("%s: job %s, step %r, %s names run or artifact %s" % (event, name, label, where, hit.group(0)))
            # A numbered artifact is a fetch that fails once it expires.
            results[name] = "failure" if len(problems) > before else "success"
    if results["differential"] == "skipped":
        problems.append("%s: the differential job is skipped (results: %s)" % (
            event, ", ".join("%s=%s" % kv for kv in results.items())))
    elif event_name == "schedule" and not reported:
        problems.append("schedule: a failed differential job files no issue")

if problems:
    sys.exit("\n".join("FAIL  " + p for p in problems))
PY

echo "PASS  the nightly and a default dispatch run the differential over no numbered artifact"
