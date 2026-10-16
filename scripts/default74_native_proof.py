"""Correlate one physical image with retained independent reference reads.

The archived reference FUSE read, not fsck's exit status, proves this exact
checksum defect. No other case or rebuilt fixture inherits that verdict.
"""

import hashlib
import json
import os
import zipfile

ARCHIVE = "ae6ea81e1866f4be19d6ad40c9b1753004db949e12c1948e49fff9a54ea326b0"
CONTROL = "70e75cfbe82c927f6914689a0142c0e37e99cae289819d86fbacf62f6ee78342"
MUTATED = "3b58375372070ca854c7600c54cca29d64a51921d68facea8eabaf4285e433eb"
PATCH = "ee572579a8b995cbe6a8950a759560baec5297638f12dcc95f0981ab84c68c67"
CASE = {
    "id": "default-74",
    "fixture": "default.img",
    "seed": "74",
    "structure": "node",
    "what": "node at byte 3014656 (32768 bytes), re-stamped",
    "ours": "found",
    "signature": "check:extent_data",
    "detail": "extent 2147483904:1:U32_MAX: extent data: checksum mismatch (stored 0x11ba7, computed 0xa2f1ba7)",
}
BINDING = {
    "control": CONTROL,
    "mutated": MUTATED,
    "patch": PATCH,
    "archive": ARCHIVE,
    "reference": "1.39.7",
}
SOURCE = (
    "https://github.com/antimatter-studios/rust-fs-bcachefs/actions/runs/38818320145"
)


def digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def grade(observations):
    """Require both native reads, their inode/path, and both extent listings."""
    expected = {"read/fusemount-help.status": "0\n"}
    for cid, checksum in (("default-control", "a2f1ba7"), ("default-74", "11ba7")):
        path = f"/mnt/{cid}/many/file-0242.txt"
        expected.update(
            {
                f"listing/{cid}.status": "0\n",
                f"read/{cid}.mount.status": "0\n",
                f"read/{cid}.path.txt": path + "\n",
                f"read/{cid}.stat.txt": f"2147483904 10 {path}\n",
                f"read/{cid}.read.status": "0\n" if cid == "default-control" else "1\n",
                f"read/{cid}.read.txt": (
                    f"fd7023d87ea3679ca779d9e8d671ac04a58df4c91ce26b2774cd06623f1f6a9d  {path}\n"
                    if cid == "default-control"
                    else f"sha256sum: {path}: Input/output error\n"
                ),
            }
        )
        key = observations.get(f"listing/{cid}.key.txt", "")
        if (
            key.count("type extent 2147483904:1:U32_MAX len 1") != 1
            or key.count(f"csum crc32c 0:{checksum}  compress none") != 1
            or f"ptr: {cid}.img 0:237:30 gen 0" not in key
        ):
            raise ValueError(cid + ": reference extent differs")
    for name, value in expected.items():
        if observations.get(name) != value:
            raise ValueError("native observation differs: " + name)


def proof(archive):
    if digest(archive) != ARCHIVE:
        raise ValueError("default74-reference artifact 12011667570 digest differs")
    with zipfile.ZipFile(archive) as z:
        observations = {
            n: z.read(n).decode()
            for n in z.namelist()
            if n.startswith(("read/", "listing/")) and not n.endswith("/")
        }
        if (
            hashlib.sha256(z.read("kept/default-74/mutation.patch")).hexdigest()
            != PATCH
        ):
            raise ValueError("reference read used another patch")
        if json.loads(z.read("kept/default-74/case.json")) != dict(
            CASE, reference_status=0
        ):
            raise ValueError("reference read used another case")
    grade(observations)
    return observations


def matches(case, binding, archive):
    if (
        not archive
        or binding != BINDING
        or any(case.get(k) != v for k, v in CASE.items())
    ):
        return False
    proof(archive)
    return True


def bind(case, control, mutated, patch, version):
    archive = os.environ.get("DIFFERENTIAL_NATIVE74_ARCHIVE")
    if not archive or any(case.get(k) != v for k, v in CASE.items()):
        return None
    binding = {
        "control": digest(control),
        "mutated": digest(mutated),
        "patch": digest(patch),
        "archive": ARCHIVE,
        "reference": version,
    }
    return binding if matches(case, binding, archive) else None
