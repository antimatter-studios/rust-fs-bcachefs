"""Black-box CLI comparison with the guest's reference lister and FUSE find."""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

tool, image, mode, *rest = sys.argv[1:]


def walk(*args):
    return subprocess.run(
        [tool, "fs", image, "ls", "-R", *args], capture_output=True, check=False
    )


before = Path(image).read_bytes()
out = walk("/")
rows = json.loads(out.stdout)
if mode == "mount":
    assert out.returncode == 0, out.stderr
    mount = rest[0]
    found = subprocess.run(
        ["find", ".", "-mindepth", "1", "-printf", r"/%P\0%y\0%i\0%s\0"],
        cwd=mount,
        capture_output=True,
        check=True,
    ).stdout.split(b"\0")
    assert found.pop() == b""
    kinds = {b"d": "dir", b"f": "reg", b"l": "lnk"}
    expected = {
        p: (kinds[k], int(i), int(s))
        for p, k, i, s in zip(found[::4], found[1::4], found[2::4], found[3::4])
    }
    paths = [
        bytes.fromhex(r["path_hex"]) if "path_hex" in r else r["path"].encode()
        for r in rows
    ]
    actual = {p: (r["type"], r["ino"], r["size"]) for p, r in zip(paths, rows)}
    assert actual == expected, (actual.keys() ^ expected.keys(), actual, expected)
    assert len(paths) == len(expected) and paths == sorted(
        paths, key=lambda p: p.split(b"/")
    )
    assert all(
        r["depth"] == len(p.split(b"/")) - 1 and "revisited" not in r
        for p, r in zip(paths, rows)
    )
    by_path = {r["path"]: r for r in rows}
    for p in ["/sv", "/snap"]:
        assert by_path[p]["subvolume"] > 1 and by_path[p]["snapshot"] > 0
    assert by_path["/sv"]["ino"] == by_path["/snap"]["ino"]
    assert by_path["/sv"]["snapshot"] != by_path["/snap"]["snapshot"]
    for p in ["/sv/new.txt", "/snap/gone.txt", "/snap/inner/file.txt"]:
        assert p in by_path
    assert not any(p.startswith((b"/dirlink/", b"/ancestor/")) for p in paths)
    raw = walk(os.fsdecode(b"/raw-\xff"))
    assert raw.returncode == 0 and json.loads(raw.stdout)[0]["depth"] == 0, raw.stderr
    listed = subprocess.run(
        ["bcachefs-ref", "list", "-b", "subvolumes", image],
        capture_output=True,
        check=True,
    ).stdout
    for p in ["/sv", "/snap"]:
        assert ("root " + str(by_path[p]["ino"])).encode() in listed
        assert re.search(
            rb"snapshot(?: id)? " + str(by_path[p]["snapshot"]).encode() + rb"\b",
            listed,
        )
    inodes = subprocess.run(
        ["bcachefs-ref", "list", "-b", "inodes", image], capture_output=True, check=True
    ).stdout
    assert all(
        re.search(rb"\b0:" + str(r["ino"]).encode() + rb":", inodes) for r in rows
    )
elif mode in ("cycle", "twice"):
    assert out.returncode == 0, out.stderr
    alias = "/dir" if mode == "cycle" else "/many"
    assert [r["path"] for r in rows if r.get("revisited")] == [alias]
    assert not any(r["path"].startswith(alias + "/") for r in rows)
elif mode == "unreadable":
    assert out.returncode == 1, out.stderr
    assert rows[-1]["path"] == "/dir"
    error = json.loads(out.stderr)
    assert error["code"] == 1 and "/dir" in error["error"]
else:
    raise AssertionError(mode)
assert Path(image).read_bytes() == before, "listing changed the image"
