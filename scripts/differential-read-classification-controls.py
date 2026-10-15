#!/usr/bin/env python3
"""Exercise classification with the live, strictly graded physical-image proof.

Only CI runs this, after the pinned reference job has independently read the
actual control and mutated images. Negative observations deliberately change
that evidence; none manufacture the positive verdict.
"""

import copy
import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def main():
    root = Path(sys.argv[1]).resolve()
    script = Path(__file__).with_name("differential-fuzz.py")
    spec = importlib.util.spec_from_file_location("differential", script)
    d = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(d)
    proof = d.load_read_proof(root, "1.39.7")
    if not proof:
        raise SystemExit("the live independent proof was not accepted")
    case = json.loads((root / "kept/default-74/case.json").read_text())
    control = json.loads((root / "kept/default-control/case.json").read_text())
    control_sha = d.sha256(root / "scratch/default-control.img")
    marker = d.bound_native_read(
        case, root / "scratch/default-74.img", control_sha, proof
    )
    if not marker:
        raise SystemExit(
            "the live physical images do not corroborate the observed finding"
        )

    # The offline checker's own verdict on the mutation, from its raw output
    # alone: the report must carry this, never the corroborated verdict.
    normal = frozenset(
        d.fold(line)
        for line in d.findings(
            (root / "kept/default-control/reference.txt").read_text()
        )
    )
    original = (
        "found"
        if d.findings((root / "kept/default-74/reference.txt").read_text(), normal)
        else "clean"
    )
    # Stands in for the canonical retained-artifact binding's file, which this
    # second channel must neither read as its proof nor overwrite.
    canonical = json.dumps({"canonical": "retained-artifact native74 binding"})

    checks = [("complete live proof", True, {}, {}, {}, None)]
    for field, value in [
        ("fixture", "other.img"),
        ("structure", "key-value"),
        ("signature", "check:node_header"),
        ("detail", case["detail"].replace("2147483904", "2147483905")),
        ("detail", case["detail"].replace("0x11ba7", "0x11ba8")),
    ]:
        checks.append(("wrong finding " + field, False, {field: value}, {}, {}, None))
    for field, value in [("run", "stale"), ("head", "0" * 40)]:
        checks.append(("stale " + field, False, {}, {field: value}, {}, None))
    for name, edits in [
        ("successful read", {"read/default-74.read.status": "0\n"}),
        ("permission denied", {"read/default-74.read.txt": "Permission denied\n"}),
        ("timeout", {"read/default-74.read.status": "124\n"}),
        ("missing output", {"read/default-74.read.txt": ""}),
        ("wrong path", {"read/default-74.path.txt": "/mnt/default-74/other\n"}),
        (
            "wrong inode",
            {
                "read/default-74.stat.txt": "2147483905 10 /mnt/default-74/many/file-0242.txt\n"
            },
        ),
        (
            "wrong listed key",
            {"listing/default-74.key.txt": "u64s 7 type extent 2147483905:1:U32_MAX\n"},
        ),
        (
            "wrong listed checksum",
            {
                "listing/default-74.key.txt": (root / "listing/default-74.key.txt")
                .read_text()
                .replace("0:11ba7", "0:11ba8")
            },
        ),
        ("wrong control bytes", {"read/default-control.read.txt": "0" * 64 + "\n"}),
        ("failed control", {"read/default-control.read.status": "1\n"}),
        (
            "wrong control inode",
            {
                "read/default-control.stat.txt": "2147483905 10 /mnt/default-control/many/file-0242.txt\n"
            },
        ),
        ("failed mount", {"read/default-74.mount.status": "1\n"}),
    ]:
        checks.append((name, False, {}, {}, edits, None))
    checks += [
        ("missing proof", False, {}, {}, {}, "absent"),
        ("wrong physical mutation", False, {}, {}, {}, "image"),
        ("wrong physical control", False, {}, {}, {}, "control"),
        ("wrong reference pin", False, {}, {}, {}, "reference"),
        ("missing reference version", False, {}, {}, {}, "version"),
        ("mismatched manifest digest", False, {}, {}, {}, "digest"),
    ]
    failures = 0
    with tempfile.TemporaryDirectory(prefix="native-classification-") as temporary:
        for index, (
            name,
            accepted,
            case_edits,
            context_edits,
            edits,
            special,
        ) in enumerate(checks):
            out = Path(temporary) / str(index)
            out.mkdir()
            evidence = out / "evidence"
            evidence.mkdir()
            shutil.copyfile(root / "evidence.txt", evidence / "evidence.txt")
            for directory in ("read", "listing", "kept"):
                shutil.copytree(root / directory, evidence / directory)
            manifest = copy.deepcopy(proof)
            manifest["context"].update(context_edits)
            for relative, text in edits.items():
                (evidence / relative).write_text(text)
            # Recompute the envelope so invalid native observations must be
            # refused by the strict grader, not merely by a digest mismatch.
            manifest["observations"] = d.proof_files(evidence)
            if special == "reference":
                manifest["binding"]["reference"] = "1.39.8"
            if special == "digest":
                manifest["observations"]["evidence.txt"] = "0" * 64
            (evidence / "read-proof.json").write_text(json.dumps(manifest))
            candidate = dict(case, **case_edits)
            matched = marker
            if special in ("image", "control"):
                wrong = out / "wrong.img"
                wrong.write_bytes(b"another physical image")
                matched = d.bound_native_read(
                    candidate,
                    wrong if special == "image" else root / "scratch/default-74.img",
                    d.sha256(wrong) if special == "control" else control_sha,
                    proof,
                )
            refs = out / "ref"
            refs.mkdir()
            fields = (
                "id",
                "fixture",
                "seed",
                "structure",
                "what",
                "ours",
                "signature",
                "detail",
            )
            (out / "cases.tsv").write_text(
                "".join(
                    "\t".join(c[k] for k in fields) + "\n" for c in (control, candidate)
                )
            )
            for c in (control, candidate):
                cid = c["id"]
                (refs / (cid + ".status")).write_text("0\n")
                shutil.copyfile(
                    root / "kept" / cid / "reference.txt", refs / (cid + ".txt")
                )
            (refs / (case["id"] + ".native-read-proof.json")).write_text(
                json.dumps(matched or {})
            )
            (refs / (case["id"] + ".read.json")).write_text(canonical)
            (out / "baseline").write_text("")
            result = subprocess.run(
                [
                    sys.executable,
                    str(script),
                    "classify",
                    str(out),
                    "--baseline",
                    str(out / "baseline"),
                    "--report",
                    str(out / "report.md"),
                    "--version",
                    "unknown" if special == "version" else "1.39.7",
                    "--read-evidence",
                    str(out / "absent") if special == "absent" else str(evidence),
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                check=False,
                timeout=40,
            )
            report = (out / "report.md").read_text()
            valid = (result.returncode == 0) == accepted
            valid &= ("Independent native data-read findings" in report) == accepted
            valid &= (refs / (case["id"] + ".read.json")).read_text() == canonical
            if accepted:
                valid &= f"fsck remains {original} (exit 0)" in report
            else:
                valid &= "NEW ours-only" in result.stdout
            if not valid:
                failures += 1
                print(f"FAIL: {name}: {result.stdout}")
            else:
                print(f"ok: {name}")
    print(
        f"native read classification controls: {len(checks)} executed, {failures} failed"
    )
    return int(failures != 0)


if __name__ == "__main__":
    sys.exit(main())
