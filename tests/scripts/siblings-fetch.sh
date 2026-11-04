#!/usr/bin/env bash
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
taskfile="${SIBLINGS_GUARD_TASKFILE:-$repo/chores.yml}"

python3 - "$taskfile" <<'PY' || exit 1
import os
from pathlib import Path
import subprocess
import sys
import tempfile

import yaml

source = yaml.safe_load(Path(sys.argv[1]).read_text())
environment = dict(os.environ, GIT_AUTHOR_NAME="Sibling guard",
                   GIT_AUTHOR_EMAIL="siblings@example.invalid",
                   GIT_COMMITTER_NAME="Sibling guard",
                   GIT_COMMITTER_EMAIL="siblings@example.invalid",
                   GIT_TERMINAL_PROMPT="0")
siblings = (("rust-fs-core", "FS_CORE_URL", "FS_CORE_REF"),
            ("fs-linux-test-harness", "LINUX_HARNESS_URL", "LINUX_HARNESS_REF"))
problems = []


def git(directory, *arguments, input=None):
    return subprocess.run(
        ["git", "-C", str(directory), *arguments], env=environment, input=input,
        text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True
    ).stdout.strip()


def check(condition, message):
    if not condition:
        problems.append(message)


def run(checkout):
    result = subprocess.run(
        ["chore", "--dir", str(checkout), "siblings"], env=environment,
        text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
    result.stdout = "\n".join(result.stdout.splitlines()[:2])
    return result


with tempfile.TemporaryDirectory(prefix="siblings-real-git-") as temporary:
    sandbox = Path(temporary)
    remote = sandbox / "remote"
    remote.mkdir()
    git(remote, "init", "--quiet")
    tree = git(remote, "mktree", input="")
    older = git(remote, "commit-tree", tree, input="before the pin\n")
    pinned = git(remote, "commit-tree", tree, "-p", older, input="the pin\n")
    newer = git(remote, "commit-tree", tree, "-p", pinned, input="after the pin\n")
    git(remote, "update-ref", "refs/heads/pinned", pinned)
    for name, url_variable, ref_variable in siblings:
        git(remote, "tag", "--no-sign", str(source["vars"][ref_variable]), pinned)

    for kind in ("tag", "branch", "commit"):
        checkout = sandbox / kind / "rust-fs-bcachefs"
        checkout.mkdir(parents=True)
        git(checkout, "init", "--quiet")
        variables = dict(source["vars"])
        for name, url_variable, ref_variable in siblings:
            variables[url_variable] = str(remote)
            if kind != "tag":
                variables[ref_variable] = "pinned" if kind == "branch" else pinned
            check(not (checkout.parent / name).exists(),
                  f"{kind}: {name} exists before the first run")
        task = {"version": source["version"], "vars": variables,
                "tasks": {"siblings": source["tasks"]["siblings"]}}
        (checkout / "chores.yml").write_text(yaml.safe_dump(task))
        first = run(checkout)
        check(first.returncode == 0, f"{kind}: first run failed:\n{first.stdout}")
        if first.returncode != 0:
            continue
        second = run(checkout)
        check(second.returncode == 0,
              f"{kind}: second run failed after cloning:\n{second.stdout}")

        for name, url_variable, ref_variable in siblings:
            sibling = checkout.parent / name
            reference = variables[ref_variable]
            check(git(sibling, "rev-parse", "HEAD") == pinned,
                  f"{kind}: {name} did not stay at its pin")
            git(sibling, "checkout", "--quiet", older)
            below = run(checkout)
            check(below.returncode != 0
                  and f"{name} is below {reference}" in below.stdout
                  and "this task will not move a shared checkout" in below.stdout,
                  f"{kind}: {name} below-pin refusal is missing:\n{below.stdout}")
            check(git(sibling, "rev-parse", "HEAD") == older,
                  f"{kind}: {name} below-pin checkout was moved")
            git(sibling, "fetch", "--quiet", "origin", newer)
            git(sibling, "checkout", "--quiet", newer)
            ahead = run(checkout)
            check(ahead.returncode == 0
                  and f"{name} at or ahead of {reference}" in ahead.stdout,
                  f"{kind}: {name} ahead-pin checkout was refused:\n{ahead.stdout}")
            check(git(sibling, "rev-parse", "HEAD") == newer,
                  f"{kind}: {name} ahead-pin checkout was moved")
            git(sibling, "checkout", "--quiet", pinned)

        if kind == "tag":
            for name, url_variable, ref_variable in siblings:
                git(checkout.parent / name, "remote", "set-url", "origin",
                    str(sandbox / "unavailable"))
            offline = run(checkout)
            check(offline.returncode == 0,
                  f"tag: cached pins failed without a remote:\n{offline.stdout}")

for problem in problems:
    print("FAIL  " + problem, file=sys.stderr)
if problems:
    sys.exit(1)
PY

echo 'PASS  siblings ref fetches are repeatable'
