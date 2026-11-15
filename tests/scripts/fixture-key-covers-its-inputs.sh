#!/usr/bin/env bash
# What a kept fixtures build answers for (#386). A merge queue run reuses a
# build while its fixture key is unchanged (scripts/fixture-key.sh), so a key
# that misses an input would test a change against fixtures it did not make.
#
# No VM, no network, no real release:
#   - the key's FILES hold everything the fixture build reaches: every script
#     from scripts/build-fixtures.sh on, the reference pin, the harness config,
#     and every source file reachable from the write verb. Each entry must be
#     needed, and removing any one fails this guard;
#   - a change to each input changes the key, and a change to a reader-only
#     file, a test or a doc does not;
#   - kept-fixtures.sh builds exactly when it says it does, and refuses bytes
#     that are not the recorded digest;
#   - the workflows ask for what the key cannot see: a tag, a release and a
#     nightly build fresh, and the nightly compares;
#   - fixtures-compare.py fails on what a reference change would look like.
#
#   bash tests/scripts/fixture-key-covers-its-inputs.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 -c 'import yaml' 2>/dev/null || {
    echo "FAIL  the python3 yaml module is required (pip install pyyaml)" >&2
    exit 1
}

python3 - "$REPO" <<'PY' || exit 1
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

import yaml

repo = Path(sys.argv[1])
fails = []


def check(condition, message):
    if not condition:
        fails.append(message)


def run(*args, cwd=None, env=None):
    return subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True)


# --- What the fixture build reaches, found from the tree itself. -------------

# The file's own scripts, as its code runs them. Comments are not references,
# and $harness/scripts/ is the harness, whose ref the key holds.
SCRIPT = re.compile(r'(?:\$here|\$\{here\}|\$REPO|/repo)/(scripts/[\w.-]+)')


def script_closure(root, roots):
    seen, todo = set(), list(roots)
    while todo:
        name = todo.pop()
        if name in seen:
            continue
        seen.add(name)
        path = root / name
        if path.suffix not in (".sh", ".py"):
            continue
        for line in path.read_text().splitlines():
            if line.lstrip().startswith("#"):
                continue
            todo.extend(SCRIPT.findall(line))
    return seen


# The source the write verb reaches: `crate::x` and `fs_bcachefs::x` name a
# module of the library, `super::x` in the tools a module of the tools, and
# write.rs brings its three halves in by #[path].
WRITER_ROOTS = ["src/cli/main.rs", "src/cli/fs.rs", "src/cli/fs_write.rs", "src/lib.rs", "src/write.rs"]


def source_closure(root):
    seen, todo = set(), list(WRITER_ROOTS)
    while todo:
        name = todo.pop()
        if name in seen:
            continue
        seen.add(name)
        path = root / name
        if not path.is_file():
            continue
        text = "\n".join(l.split("//")[0] for l in path.read_text().splitlines())
        found = [f"src/{m}.rs" for m in re.findall(r"\b(?:crate|fs_bcachefs)::([a-z_]+)", text)]
        found += [f"src/{m}" for m in re.findall(r'#\[path = "([\w.]+)"\]', text)]
        if name.startswith("src/cli/"):
            found += [f"src/cli/{m}.rs" for m in re.findall(r"\bsuper::([a-z_]+)", text)]
        todo.extend(f for f in found if (root / f).is_file())
    return seen


def harness_setup(root):
    return re.findall(r'^script = "(scripts/[\w.-]+)"', (root / "fs-linux-test-harness.toml").read_text(), re.M)


def required(root):
    """Each input, with the reason it is one."""
    needed = {}
    for name in sorted(script_closure(root, ["scripts/build-fixtures.sh"])):
        needed[name] = "the fixture build runs it"
    for name in harness_setup(root):
        needed[name] = "the harness's [setup] builds the reference tools with it"
    needed["fs-linux-test-harness.toml"] = "it configures the VM the fixtures are built in"
    for name in sorted(source_closure(root)):
        needed[name] = "the write verb reaches it"
    needed["Cargo.toml"] = "the writer's features and dependencies"
    needed["Cargo.lock"] = "the writer's dependency versions"
    needed["rust-toolchain.toml"] = "the compiler that builds the writer"
    return needed


def judge(files, needed):
    """Every disagreement between the key's FILES and what is needed."""
    problems = [f"{n} is not in the key: {why}" for n, why in needed.items() if n not in files]
    problems += [f"{n} is in the key and nothing reaches it" for n in files if n not in needed]
    return problems


files = run("bash", "scripts/fixture-key.sh", "--files", cwd=repo).stdout.split()
check(files, "fixture-key.sh --files printed nothing")
needed = required(repo)
for problem in judge(files, needed):
    check(False, problem)

# The controls: this guard fails when the key misses any one input.
for name in files:
    check(judge([f for f in files if f != name], needed),
          f"removing {name} from the key was not noticed")
check(judge(files + ["scripts/an-extra.sh"], needed), "an unneeded input was not noticed")

# Writer reach that must stay out, or every checker fix would rebuild: these are
# what the key leaves out on purpose, and they hold no writer code.
for name in ["src/check.rs", "src/capi.rs", "src/capi_write.rs", "src/cli/fsck.rs"]:
    check(name not in needed, f"{name} now reaches the writer's path, or the writer reaches it: decide if it belongs in the key")


with tempfile.TemporaryDirectory(prefix="fixture-key-guard-") as tmp:
    tree = Path(tmp) / "tree"
    tree.mkdir()
    extra = [
        "chores.yml", ".github/workflows/ci.yml", "scripts/fixture-key.sh", "scripts/kept-fixtures.sh",
        "scripts/fixtures-compare.py", "src/check.rs", "src/capi.rs", "README.md",
        "tests/oracle_kernel.rs", "docs/clean-room.md",
    ]
    for name in files + extra:
        target = tree / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(repo / name, target)
    scratch = Path(tmp) / "bin"
    scratch.mkdir()

    def git(*args):
        result = run("git", "-c", "user.name=guard", "-c", "user.email=guard@example.invalid",
                     "-c", "commit.gpgsign=false", *args, cwd=tree)
        assert result.returncode == 0, result.stdout + result.stderr
        return result.stdout.strip()

    def commit(message):
        # Plumbing: the guard's tree is scratch and must not run anyone's hooks.
        git("add", "-A")
        parent = run("git", "rev-parse", "--verify", "--quiet", "HEAD", cwd=tree).stdout.strip()
        made = git("commit-tree", git("write-tree"), "-m", message, *(["-p", parent] if parent else []))
        git("update-ref", "HEAD", made)
        return made

    git("init", "-q")
    base = commit("base")

    def key(*args):
        result = run("bash", "scripts/fixture-key.sh", *args, cwd=tree)
        assert result.returncode == 0, result.stderr
        return result.stdout.strip()

    original = key()
    check(re.fullmatch(r"[0-9a-f]{64}", original), f"the key is not a sha256: {original}")

    def mutated(name, change, ask=True):
        """The key with `name` changed and committed; then back to base."""
        path = tree / name
        before = path.read_bytes()
        path.write_text(change(path.read_text()))
        commit("change")
        result = key()
        changed = run("bash", "scripts/fixture-key.sh", "--changed-from", base, cwd=tree).stdout.strip() if ask else None
        path.write_bytes(before)
        git("reset", "-q", "--hard", base)
        return result, changed

    # Each input moves the key, and the pull request step sees it.
    for name in files:
        result, _ = mutated(name, lambda t: t + "\n# changed\n", ask=False)
        check(result != original, f"changing {name} leaves the key alone")

    # The pins and sections held by content, not by whole file.
    pin_cases = {
        "the core pin": ("chores.yml", lambda t: re.sub(r"FS_CORE_REF: \S+", "FS_CORE_REF: v9.9.9", t)),
        "the harness pin": ("chores.yml", lambda t: re.sub(r"LINUX_HARNESS_REF: \S+", "LINUX_HARNESS_REF: v9.9.9", t)),
        "the fixtures task": ("chores.yml", lambda t: t.replace("cmds: ['scripts/build-fixtures.sh']", "cmds: ['scripts/other.sh']")),
        "the fixtures job": (".github/workflows/ci.yml", lambda t: t.replace("toolchain: 1.95.0\n      - name: chore fixtures", "toolchain: 1.96.0\n      - name: chore fixtures")),
        "the generation": ("scripts/fixture-key.sh", lambda t: re.sub(r"^GENERATION=\d+", "GENERATION=999", t, flags=re.M)),
    }
    for label, (name, change) in pin_cases.items():
        text = (tree / name).read_text()
        check(change(text) != text, f"the {label} control changed nothing: the guard no longer finds it")
        result, changed = mutated(name, change)
        check(result != original, f"changing {label} leaves the key alone")
        check(changed == "changed=true", f"changing {label}: {changed}")

    # What the key leaves out on purpose reads the same images.
    outside = {
        "src/check.rs": lambda t: t + "\n// changed\n",
        "src/capi.rs": lambda t: t + "\n// changed\n",
        "tests/oracle_kernel.rs": lambda t: t + "\n// changed\n",
        "docs/clean-room.md": lambda t: t + "\nchanged\n",
        "README.md": lambda t: t + "\nchanged\n",
        # A floor or budget of another task, and another job of ci.yml.
        "chores.yml": lambda t: t + "\n# changed\n",
        ".github/workflows/ci.yml": lambda t: t.replace("name: chore test:oracle\n", "name: chore test:oracle again\n"),
    }
    for name, change in outside.items():
        text = (tree / name).read_text()
        check(change(text) != text, f"the {name} control changed nothing")
        result, changed = mutated(name, change)
        check(result == original, f"changing {name} moved the key, so it would rebuild for nothing")
        check(changed == "changed=false", f"changing {name}: {changed}")

    # A removed input fails rather than quietly leaving the key.
    (tree / "scripts/prealloc.c").unlink()
    commit("removed")
    result = run("bash", "scripts/fixture-key.sh", cwd=tree)
    check(result.returncode != 0 and "scripts/prealloc.c" in result.stderr,
          "a key input that is gone did not fail, naming it")
    check(run("bash", "scripts/fixture-key.sh", "--changed-from", base, cwd=tree).returncode != 0,
          "--changed-from accepted a head missing an input")
    git("reset", "-q", "--hard", base)
    # A new input a base does not have is a change.
    newer = tree / "scripts/fixture-key.sh"
    saved = newer.read_text()
    (tree / "scripts/a-new-input.sh").write_text("#!/bin/sh\n")
    newer.write_text(saved.replace("FILES=(\n", "FILES=(\n    scripts/a-new-input.sh\n"))
    commit("new input")
    check(run("bash", "scripts/fixture-key.sh", "--changed-from", base, cwd=tree).stdout.strip() == "changed=true",
          "a new input the base lacks did not read as a change")
    git("reset", "-q", "--hard", base)

    # --- kept-fixtures.sh, against a fake repository. -------------------------
    fake = scratch / "gh"
    fake.write_text(r'''#!/usr/bin/env bash
# The releases and runs of a repository that does not exist. Logs each call.
echo "$*" >>"$FAKE/calls"
case "$1 $2" in
    "api repos/"*)
        case "${FAKE_API:-miss}" in
            hit) echo "sha256:$FAKE_DIGEST" ;;
            miss) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
            *) echo "gh: Bad Gateway (HTTP 502)" >&2; exit 1 ;;
        esac
        ;;
    "release download")
        while [ "$#" -gt 0 ]; do
            if [ "$1" = --dir ]; then dir="$2"; fi
            shift
        done
        cp "$FAKE/asset" "$dir/fixtures.tar.gz"
        ;;
    "release create") [ "${FAKE_CREATE:-ok}" = ok ] || exit 1 ;;
    "run list") printf '%s\n' "${FAKE_RUN:-}" ;;
    "run download")
        [ "${FAKE_ARTIFACT:-yes}" = yes ] || { echo "no artifact" >&2; exit 1; }
        while [ "$#" -gt 0 ]; do
            if [ "$1" = --dir ]; then dir="$2"; fi
            shift
        done
        cp "$FAKE/asset" "$dir/fixtures.tar.gz"
        ;;
    *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
esac
''')
    fake.chmod(0o755)
    (scratch / "asset").write_bytes(b"the kept bytes")
    digest = subprocess.run(
        ["python3", "-c", "import hashlib,sys;print(hashlib.sha256(open(sys.argv[1],'rb').read()).hexdigest())",
         str(scratch / "asset")], capture_output=True, text=True).stdout.strip()
    tag = "fixtures-" + original[:16]

    def kept(*args, **extra):
        calls = scratch / "calls"
        if calls.exists():
            calls.unlink()
        env = dict(os.environ, GH=str(fake), FAKE=str(scratch), FAKE_DIGEST=digest,
                   KEPT_FIXTURES_TRIES="2", KEPT_FIXTURES_PAUSE="0",
                   GITHUB_REPOSITORY="owner/name", **extra)
        env.pop("FRESH", None) if "FRESH" not in extra else None
        result = run("bash", "scripts/kept-fixtures.sh", *args, cwd=tree, env=env)
        return result, calls.read_text() if calls.exists() else ""

    r, _ = kept("select", FAKE_API="hit")
    check(r.returncode == 0 and r.stdout.split() == ["kept=true", f"tag={tag}", f"sha256={digest}"],
          f"a key with a release did not read as kept: {r.stdout!r} {r.stderr!r}")
    r, _ = kept("select", FAKE_API="miss")
    check(r.returncode == 0 and r.stdout.split() == ["kept=false"], f"a key with no release did not build: {r.stdout!r}")
    r, calls = kept("select", FAKE_API="hit", FRESH="true")
    check(r.returncode == 0 and r.stdout.split() == ["kept=false"], f"FRESH did not build: {r.stdout!r}")
    check(calls == "", "FRESH still asked for a release")
    r, _ = kept("select", FAKE_API="down")
    check(r.returncode != 0 and "kept=" not in r.stdout,
          f"a release API failure was read as an answer: {r.stdout!r}")

    out = scratch / "out"
    r, _ = kept("fetch", tag, digest, str(out))
    check(r.returncode == 0 and (out / "fixtures.tar.gz").read_bytes() == b"the kept bytes",
          f"the recorded digest was refused: {r.stderr}")
    shutil.rmtree(out)
    r, _ = kept("fetch", tag, "0" * 64, str(out))
    check(r.returncode != 0 and "no fallback" in r.stderr, "other bytes than the recorded digest were accepted")
    check(not (out / "fixtures.tar.gz").exists(), "refused bytes were left behind")

    r, calls = kept("publish", str(scratch / "asset"), "123", FAKE_API="miss")
    check(r.returncode == 0, f"publish failed: {r.stderr}")
    for needle in ["release create " + tag, "--prerelease", "--latest=false", "/fixtures.tar.gz", "fixture-key-manifest.txt"]:
        check(needle in calls, f"publish did not pass {needle}: {calls}")
    check("--clobber" not in calls, "publish may replace a kept build")
    r, calls = kept("publish", str(scratch / "asset"), "123", FAKE_API="hit")
    check(r.returncode == 0 and "release create" not in calls, "a key that has a build was published again")
    r, _ = kept("publish", str(scratch / "asset"), "123", FAKE_API="miss", FAKE_CREATE="fail")
    check(r.returncode != 0, "a failed create with no release left was not a failure")
    r, _ = kept("publish", str(scratch / "absent"), "123", FAKE_API="miss")
    check(r.returncode != 0, "an absent tarball was published")

    r, calls = kept("promote", base, FAKE_API="hit")
    check(r.returncode == 0 and "run list" not in calls, "a kept key was promoted again")
    r, calls = kept("promote", base, FAKE_API="miss", FAKE_RUN="555")
    check(r.returncode == 0, f"promote failed: {r.stderr}")
    check("--event merge_group" in calls and f"--commit {base}" in calls and "--status success" in calls,
          f"promote did not ask for this commit's green queue run: {calls}")
    check("run download 555" in calls and "release create" in calls, f"promote did not keep that run's build: {calls}")
    r, calls = kept("promote", base, FAKE_API="miss", FAKE_RUN="")
    check(r.returncode == 0 and "::warning::" in r.stdout and "release create" not in calls,
          "a commit with no queue run did not warn and go on")
    r, _ = kept("promote", base, FAKE_API="miss", FAKE_RUN="555", FAKE_ARTIFACT="no")
    check(r.returncode != 0, "a green queue run with no artifact was not a failure")

# --- The workflows ask for what the key cannot see. --------------------------


def workflow(name):
    return yaml.safe_load((repo / ".github/workflows" / name).read_text())


def triggers(doc):
    return doc.get("on", doc.get(True)) or {}


ci = workflow("ci.yml")
job = ci["jobs"]["fixtures"]
steps = job["steps"]
source = [s for s in steps if s.get("id") == "source"]
check(len(source) == 1 and "kept-fixtures.sh select" in source[0]["run"], "the fixtures job does not ask select")
fresh = source[0].get("env", {}).get("FRESH", "") if source else ""
check("inputs.fresh_fixtures" in fresh and "refs/tags/" in fresh, f"a tag or a dispatch does not build fresh: {fresh}")
names = [s.get("name", s.get("uses", "")) for s in steps]
for wanted in ["chore fixtures", "Stop the shared guest (no run is still using it)", "Pack the fixtures, keeping their holes",
               "KVM, QEMU and Vagrant", "Install chore", "chore siblings", "Install Rust", "Cache the Vagrant box"]:
    match = [s for s in steps if s.get("name") == wanted]
    check(len(match) == 1 and match[0].get("if") == "steps.source.outputs.kept != 'true'",
          f"{wanted} does not run exactly when no build is kept")
fetch = [s for s in steps if "kept-fixtures.sh fetch" in s.get("run", "")]
check(len(fetch) == 1 and fetch[0].get("if") == "steps.source.outputs.kept == 'true'", "the kept build is not fetched when kept")
upload = [s for s in steps if str(s.get("uses", "")).startswith("actions/upload-artifact") and s["with"]["name"] == "fixtures"]
check(len(upload) == 1 and "if" not in upload[0], "the fixtures artifact is not uploaded on both paths")
for name, consumer in ci["jobs"].items():
    needs = consumer.get("needs", [])
    if "fixtures" in ([needs] if isinstance(needs, str) else needs) and name != "ci-ok":
        text = json.dumps(consumer["steps"])
        check("actions/download-artifact" in text, f"{name} needs fixtures and does not read the artifact")
        check("kept-fixtures" not in text and "chore fixtures" not in text, f"{name} gets fixtures its own way")

changes = ci["jobs"]["changes"]
check("steps.k.outputs.changed == 'true'" in changes["outputs"]["code"], "a key change does not run the full pipeline")
k = [s for s in changes["steps"] if s.get("id") == "k"]
check(len(k) == 1 and k[0].get("if") == "github.event_name == 'pull_request'" and "--changed-from HEAD^1" in k[0]["run"],
      "the pull request step does not compare with its base")
check(changes["steps"][0].get("with", {}).get("fetch-depth") == 0, "the changes job lacks the base commit")

gate = workflow("release.yml")["jobs"]["gate"]
check(gate.get("with", {}).get("fresh_fixtures") is True, "a release does not build its fixtures fresh")

nightly = workflow("fixtures-fresh.yml")
check("schedule" in triggers(nightly), "the fresh build has no schedule")
check(nightly["jobs"]["gate"]["with"]["fresh_fixtures"] is True, "the nightly does not build fresh")
compare_steps = nightly["jobs"]["compare"]["steps"]
check(any("fixtures-compare.py" in s.get("run", "") and s.get("if") == "steps.source.outputs.kept == 'true'" for s in compare_steps),
      "the nightly does not compare with the kept build")
check(any("kept-fixtures.sh publish" in s.get("run", "") and s.get("if") == "steps.source.outputs.kept != 'true'" for s in compare_steps),
      "a night with nothing kept for its key does not keep its build")
check("failure()" in nightly["jobs"]["file"]["if"], "a failed night is not filed")

keep = workflow("fixtures-keep.yml")
check(triggers(keep)["push"]["branches"] == ["main"], "the landed build is not kept on a push to main")
check("promote" in keep["jobs"]["keep"]["steps"][-1]["run"], "a landing does not promote")
check(keep["jobs"]["keep"]["permissions"].get("contents") == "write", "the keep job cannot create a release")
check(keep["permissions"] == {"contents": "read"}, "the keep workflow's default token should stay read-only")

# --- fixtures-compare.py fails on what a reference change looks like. --------

with tempfile.TemporaryDirectory(prefix="fixtures-compare-guard-") as tmp:
    root = Path(tmp)
    base_files = {
        "fixtures/default.img": b"\0" * 4096,
        "fixtures/default.json": b'{"uuid": "03b23b04-23a8-4d1b-b166-6a41cf6962af", "seq": 12}\n',
        "fixtures/default.format.txt": b"version 1.39.7\nb\na\n",
        "fixtures/kernel.txt": b"kernel: 7.2.11+deb13-amd64\nmodule: loaded\nwritten: ok\n",
        "fixtures/default.super.txt": b"Created: Wed Nov  4 09:33:32 2026\nName: default\n",
        "fixtures/default.freespace.txt": b"u64s 5 type set 0:1 len 3\n",
    }

    def pack(name, changes=None, removed=()):
        members = dict(base_files, **(changes or {}))
        path = root / name
        with tarfile.open(path, "w:gz") as tar:
            for member, data in members.items():
                if member in removed:
                    continue
                source = root / "member"
                source.write_bytes(data)
                tar.add(source, arcname=member)
        return str(path)

    def verdict(other):
        return run("python3", "-I", str(repo / "scripts/fixtures-compare.py"), str(kept_tar), other)

    kept_tar = pack("kept.tar.gz")
    r = verdict(pack("same.tar.gz"))
    check(r.returncode == 0 and "0 differences" in r.stdout, f"the same build differs: {r.stdout}")
    r = verdict(pack("noise.tar.gz", {
        "fixtures/default.json": b'{"uuid": "1fc9f28a-0bc6-4d24-8140-9a3231e2a544", "seq": 99}\n',
        "fixtures/default.format.txt": b"version 1.39.7\na\nb\n",
        "fixtures/default.super.txt": b"Created: Tue Mar 10 21:01:02 2026\nName: default\n",
        "fixtures/default.freespace.txt": b"u64s 5 type set 0:1 len 3\nu64s 5 type set 0:9 len 1\n",
    }))
    check(r.returncode == 0, f"what differs between any two builds failed the compare: {r.stdout}")
    for label, changes, removed in [
        ("the kernel the module runs on", {"fixtures/kernel.txt": b"kernel: 7.3.1+deb13-amd64\nmodule: loaded\nwritten: ok\n"}, ()),
        ("a stage the module records", {"fixtures/kernel.txt": b"kernel: 7.2.11+deb13-amd64\nmodule: loaded\nwritten: failed\n"}, ()),
        ("what the reference prints", {"fixtures/default.format.txt": b"version 1.39.7\na\nb\nc\n"}, ()),
        ("the reference's version", {"fixtures/default.format.txt": b"version 1.40.0\nb\na\n"}, ()),
        ("an image's size", {"fixtures/default.img": b"\0" * 8192}, ()),
        ("a new record", {"fixtures/extra.txt": b"x\n"}, ()),
        ("a record gone", {}, ("fixtures/default.json",)),
    ]:
        r = verdict(pack("changed.tar.gz", changes, removed))
        check(r.returncode == 1 and "fixtures-compare: fixtures/" in r.stdout, f"{label} changed and the compare passed: {r.stdout}")

if fails:
    for line in fails:
        print(f"FAIL  {line}", file=sys.stderr)
    sys.exit(1)
PY
echo "fixture-key-covers-its-inputs: all checks passed"
