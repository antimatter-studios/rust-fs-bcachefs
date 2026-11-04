#!/usr/bin/env python3
"""Grade recorded independent reads, then reject deliberately damaged observations.

The workflow verifies the kept observations' digest before extracting them.
They record reference 1.39.7 reading the pinned physical images:
fsck returns zero for both, while the mutated file read returns EIO. These
controls exercise the live script's verdict, not the reference tools or CRC.
"""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def main():
    recorded = Path(sys.argv[1]).resolve()
    grader = Path(__file__).with_name("pinned-default74-reference.sh")
    control_path = "/mnt/default-control/big/transfer.bin"
    mutated_path = "/mnt/default-206/big/transfer.bin"
    digest = "e34dc68601ef38d9ddf0af2478b18241650ff47e512720d3d7d481101659c5fa"
    # Values come from the pinned independent read, never from this reader.
    cases = [
        ("recorded EIO", True, {}),
        (
            "successful identical read is not observable",
            False,
            {
                "read/default-206.read.status": "0\n",
                "read/default-206.read.txt": f"{digest}  {mutated_path}\n",
            },
        ),
        (
            "successful different read is not an integrity refusal",
            False,
            {
                "read/default-206.read.status": "0\n",
                "read/default-206.read.txt": f"{'0' * 64}  {mutated_path}\n",
            },
        ),
        (
            "permission failure is not EIO",
            False,
            {
                "read/default-206.read.txt": f"sha256sum: {mutated_path}: Permission denied\n",
            },
        ),
        (
            "timeout is not EIO",
            False,
            {
                "read/default-206.read.status": "124\n",
            },
        ),
        (
            "failed mount is not a data read",
            False,
            {
                "read/default-206.mount.status": "1\n",
            },
        ),
        (
            "missing read output is not EIO",
            False,
            {
                "read/default-206.read.txt": "",
            },
        ),
        (
            "missing read status is refused",
            False,
            {
                "read/default-206.read.status": "",
            },
        ),
        (
            "missing inode observation is refused",
            False,
            {
                "read/default-206.stat.txt": "",
            },
        ),
        (
            "wrong inode is refused",
            False,
            {
                "read/default-206.stat.txt": f"2147483654 3145729 {mutated_path}\n",
            },
        ),
        (
            "wrong file is refused",
            False,
            {
                "read/default-206.path.txt": "/mnt/default-206/big/odd.bin\n",
            },
        ),
        (
            "wrong control bytes are refused",
            False,
            {
                "read/default-control.read.txt": f"{'0' * 64}  {control_path}\n",
            },
        ),
        (
            "failed control read is refused",
            False,
            {
                "read/default-control.read.status": "1\n",
            },
        ),
        (
            "wrong control inode is refused",
            False,
            {
                "read/default-control.stat.txt": f"2147483654 3145729 {control_path}\n",
            },
        ),
    ]
    evidence = (recorded / "evidence.txt").read_text()
    for name, before, after in [
        (
            "wrong physical image",
            "d162eb63af5d9f0a2ca7cc1753c8c7d9169ad87dbb163766f42520a5f349156f",
            "0" * 64,
        ),
        (
            "wrong extent checksum tuple",
            "default-206 0x0, default-control 0x25134e43",
            "default-206 0x1, default-control 0x25134e43",
        ),
        (
            "wrong fixture pin",
            "6edbb7c0675ce7735a49ec173c0d371d707847a4885946d2010c1ebad51f36d7",
            "0" * 64,
        ),
        (
            "wrong reference fsck status",
            "default-206 reference fsck -n status 0 (expected 0)",
            "default-206 reference fsck -n status 1 (expected 0)",
        ),
    ]:
        if evidence.count(before) != 1:
            raise SystemExit(f"recorded evidence does not uniquely bind {name}")
        cases.append((name, False, {"evidence.txt": evidence.replace(before, after)}))
    failures = 0
    with tempfile.TemporaryDirectory(prefix="default74-controls-") as temporary:
        for index, (name, accepted, edits) in enumerate(cases):
            case = Path(temporary) / str(index)
            shutil.copytree(recorded, case)
            for relative, value in edits.items():
                (case / relative).write_text(value)
            result = subprocess.run(
                ["bash", str(grader), "--grade-read-evidence", str(case), "2147483653"],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                timeout=20,
                check=False,
            )
            if (result.returncode == 0) != accepted:
                failures += 1
                print(
                    f"FAIL: {name}: status {result.returncode}, expected {'accept' if accepted else 'reject'}"
                )
                print(result.stdout)
            else:
                print(f"ok: {name}")
    print(f"default74 read controls: {len(cases)} executed, {failures} failed")
    return int(failures != 0)


if __name__ == "__main__":
    sys.exit(main())
