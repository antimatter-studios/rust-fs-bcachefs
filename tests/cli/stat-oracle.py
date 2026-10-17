"""fs.bcachefs stat, judged by the reference mount: every shared field of
every path below the root against the mount's own lstat, a symlink's target
against its readlink, a non-UTF-8 name resolved as bytes, and the image
unchanged by the reads."""

import json
import os
import stat
import subprocess
import sys
from pathlib import Path

tool, image, mount = sys.argv[1:]
KINDS = {
    stat.S_IFREG: "file",
    stat.S_IFDIR: "dir",
    stat.S_IFLNK: "symlink",
    stat.S_IFCHR: "char",
    stat.S_IFBLK: "block",
    stat.S_IFIFO: "fifo",
    stat.S_IFSOCK: "socket",
}

before = Path(image).read_bytes()
root = os.fsencode(mount)
paths = []
for top, dirs, files in os.walk(root):
    dirs.sort()
    for name in sorted(dirs) + sorted(files):
        paths.append(os.path.join(top, name)[len(root) :])
assert len(paths) > 300 and b"/raw-\xff" in paths, paths[:10]

for p in paths:
    st = os.lstat(root + p)
    out = subprocess.run(
        [tool, "fs", image, "stat", os.fsdecode(p)], capture_output=True, check=False
    )
    assert out.returncode == 0, (p, out.stderr)
    r = json.loads(out.stdout)
    want = {
        "inode": st.st_ino,
        "type": KINDS[stat.S_IFMT(st.st_mode)],
        "mode": f"{stat.S_IMODE(st.st_mode):04o}",
        "uid": st.st_uid,
        "gid": st.st_gid,
        "nlink": st.st_nlink,
        "size": st.st_size,
        "allocated_bytes": st.st_blocks * 512,
    }
    for t in ("atime", "mtime", "ctime"):
        want[t], want[t + "_nsec"] = divmod(getattr(st, "st_" + t + "_ns"), 10**9)
    if stat.S_ISLNK(st.st_mode):
        want["target"] = os.readlink(root + p).decode("utf-8", "replace")
    try:
        p.decode("utf-8")
        want["path_hex"] = None
    except UnicodeDecodeError:
        want["path_hex"] = p.hex()
    got = {k: r.get(k) for k in want}
    assert got == want, (p, {k: (got[k], want[k]) for k in want if got[k] != want[k]})

assert Path(image).read_bytes() == before, "stat changed the image"
