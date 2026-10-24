#!/usr/bin/env bash
# fs.bcachefs export --recursive (#342), as installed, against what the
# reference implementation's guest recorded, never against this driver's
# own reads:
#   deep  formatted by the reference formatter from a tree 33 directories
#         deep: every file's bytes, every symlink target, the depth bound,
#         merging, replacement, the device offset, and a damaged file that
#         fails alone and is never published;
#   aged  mounted and written by the reference implementation: hard links,
#         and the access and modification times its mount reported.
source "$(dirname "$0")/lib.sh"
source "$(dirname "$0")/offset-lib.sh"
need_fixture deep
need_fixture aged

# tree.py MANIFEST PREFIX ROOT CHECKS [SKIP...]: the exported tree at ROOT
# against every manifest entry below PREFIX. CHECKS is a comma list:
#   content  files hold the manifest's bytes, directories exist, and
#            symlinks are absent (they were refused);
#   links    symlinks exist and hold the manifest's targets instead;
#   hard     entries sharing a manifest inode share one exported inode,
#            with the manifest's link count;
#   times    files and directories carry the manifest's access and
#            modification times, to the nanosecond.
# SKIP names manifest paths to leave out. Every stat is taken before any
# file is read, so reading cannot move an access time.
cat >"$SANDBOX/tree.py" <<'PY'
import hashlib, json, os, sys
manifest, prefix, root, checks, *skip = sys.argv[1:]
checks = set(checks.split(","))
prefix = prefix.rstrip("/")
entries = [e for e in json.load(open(manifest))["entries"]
           if e["path"].startswith(prefix + "/") and e["path"] not in skip]
assert entries, f"no manifest entry below {prefix}"
bad = []
local = {e["path"]: os.path.join(root, e["path"][len(prefix) + 1:]) for e in entries}
st = {}
for e in entries:
    try:
        st[e["path"]] = os.lstat(local[e["path"]])
    except FileNotFoundError:
        st[e["path"]] = None
for e in entries:
    p, s = e["path"], st[e["path"]]
    if e["type"] == "symlink":
        if "links" in checks:
            if s is None or not os.path.islink(local[p]):
                bad.append(f"{p}: no symlink")
            elif os.readlink(local[p]) != e["target"]:
                bad.append(f"{p}: target {os.readlink(local[p])!r}, not {e['target']!r}")
        elif s is not None:
            bad.append(f"{p}: a refused symlink was created")
        continue
    if s is None:
        bad.append(f"{p}: missing")
        continue
    if "times" in checks:
        for t in ("atime", "mtime"):
            want = e[t] * 10**9 + e[t + "_nsec"]
            got = getattr(s, "st_" + t + "_ns")
            if got != want:
                bad.append(f"{p}: {t} {got}, not {want}")
    if e["type"] == "dir":
        if not os.path.isdir(local[p]):
            bad.append(f"{p}: not a directory")
    elif "content" in checks:
        got = hashlib.sha256(open(local[p], "rb").read()).hexdigest()
        if got != e["sha256"]:
            bad.append(f"{p}: sha256 {got}, not {e['sha256']}")
if "hard" in checks:
    groups = {}
    for e in entries:
        if e["type"] == "file":
            groups.setdefault(e["ino"], []).append(e)
    linked = [g for g in groups.values() if len(g) > 1]
    if not linked:
        bad.append("the manifest holds no hard link to check")
    for g in groups.values():
        inos = {st[e["path"]].st_ino for e in g if st[e["path"]]}
        if len(g) > 1 and len(inos) != 1:
            bad.append(f"{[e['path'] for e in g]}: {len(inos)} exported inodes, not one")
        for e in g:
            if st[e["path"]] and st[e["path"]].st_nlink != e["nlink"]:
                bad.append(f"{e['path']}: {st[e['path']].st_nlink} links, not {e['nlink']}")
for b in bad[:20]:
    print(b, file=sys.stderr)
sys.exit(1 if bad else 0)
PY
tree() { python3 "$SANDBOX/tree.py" "$@" 2>>"$SANDBOX/tree.err"; }

# export_tree IMAGE PATH DEST [FLAG...]: a recursive export, its report in
# $SANDBOX/report and its status in $status.
export_tree() {
    local img="$1" path="$2" dest="$3"
    shift 3
    fs.bcachefs "$img" export --recursive "$@" "$path" "$dest" >"$SANDBOX/report" 2>"$SANDBOX/err"
    status=$?
}
# item PATH: the report's item for PATH.
item() { jq -c --arg p "$1" '.items[] | select(.path == $p)' "$SANDBOX/report"; }
# jqfile FILTER FILE: FILE satisfies FILTER.
jqfile() { jq -e "$1" "$2" >/dev/null; }
# jreport FILTER: the report satisfies FILTER. jitem PATH FILTER: so does
# PATH's item. Quiet: jq's verdict is not test output.
jreport() { jq -e "$1" "$SANDBOX/report" >/dev/null; }
jitem() { item "$1" | jq -e "$2" >/dev/null; }

deep="$FIXTURES/deep.img"
manifest="$FIXTURES/deep.json"
check 'the deep tree is 33 directories deep' \
    jqfile '[.entries[] | select(.type == "dir")] | length == 34 and any(.path == "/tree/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19/d20/d21/d22/d23/d24/d25/d26/d27/d28/d29/d30/d31/d32/d33")' "$manifest"
cp "$deep" "$SANDBOX/before.img"

# Without --recursive a directory is still refused.
fs.bcachefs "$deep" export /tree "$SANDBOX/plain" >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'a directory without --recursive is refused' test $? -eq 1
check 'the refusal publishes nothing' test ! -e "$SANDBOX/plain"

# Every file and directory, each reported; the three symlinks refused
# without --preserve=links.
export_tree "$deep" /tree "$SANDBOX/refused"
check 'refused symlinks make the tree partial' test "$status" -eq 1
check 'deep export counts' jreport '.completed == 69 and .refused == 3 and .skipped == 0 and .failed == 0'
check 'deep export matches the formatter source' tree "$manifest" /tree "$SANDBOX/refused" content
for p in /tree/link /tree/dangling /tree/d1/up; do
    check "$p is refused naming --preserve=links" jitem "$p" '.status == "refused" and (.reason | contains("--preserve=links"))'
done
check 'the deepest file is completed' jitem /tree/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19/d20/d21/d22/d23/d24/d25/d26/d27/d28/d29/d30/d31/d32/d33/f33 '.status == "completed"'

# With --preserve=links every symlink is recreated, never followed.
export_tree "$deep" /tree "$SANDBOX/linked" --preserve=links
check 'a linked export is complete' test "$status" -eq 0
check 'linked export counts' jreport '.completed == 72 and .refused == 0 and .failed == 0'
check 'linked export matches the formatter source' tree "$manifest" /tree "$SANDBOX/linked" content,links

# The depth bound: the deepest entry, /tree/d1/.../d33/f33, is at depth 34.
export_tree "$deep" /tree "$SANDBOX/fit" --preserve=links --max-depth 34
check 'an exact depth fit is complete' test "$status" -eq 0
check 'exact fit counts' jreport '.completed == 72 and .failed == 0'
export_tree "$deep" /tree "$SANDBOX/over" --preserve=links --max-depth 33
check 'one level less is partial' test "$status" -eq 1
check 'only the deepest file fails' jreport '.completed == 71 and .failed == 1'
check 'the deepest file names the bound' jitem /tree/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19/d20/d21/d22/d23/d24/d25/d26/d27/d28/d29/d30/d31/d32/d33/f33 '.status == "failed" and (.reason | contains("maximum depth exceeded"))'
check 'everything above the bound is exported' tree "$manifest" /tree "$SANDBOX/over" content,links \
    /tree/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19/d20/d21/d22/d23/d24/d25/d26/d27/d28/d29/d30/d31/d32/d33/f33
check 'the file past the bound is not published' \
    test ! -e "$SANDBOX/over/d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19/d20/d21/d22/d23/d24/d25/d26/d27/d28/d29/d30/d31/d32/d33/f33"

# Existing directories are merged into, and what else they hold survives.
mkdir -p "$SANDBOX/merged/d1"
printf 'not from the image\n' >"$SANDBOX/merged/d1/local-only"
export_tree "$deep" /tree "$SANDBOX/merged" --preserve=links
check 'a merged export is complete' test "$status" -eq 0
check 'two directories are merged into' jreport '.completed == 72 and .merged_directories == 2'
check 'merged export matches the formatter source' tree "$manifest" /tree "$SANDBOX/merged" content,links
check 'the merged directory keeps its own file' grep -qx 'not from the image' "$SANDBOX/merged/d1/local-only"

# An existing file is refused and kept without --replace, replaced with it.
printf 'keep me\n' >"$SANDBOX/merged/big"
export_tree "$deep" /tree "$SANDBOX/merged" --preserve=links
check 'existing files make the tree partial' test "$status" -eq 1
check 'an existing file is refused' jitem /tree/big '.status == "refused"'
check 'the refused file is kept' grep -qx 'keep me' "$SANDBOX/merged/big"
export_tree "$deep" /tree "$SANDBOX/merged" --preserve=links --replace
check 'replace export counts' jreport '.failed == 0 and .completed > 0'
check 'an existing file is replaced' jitem /tree/big '.status == "completed"'
check 'the replaced tree matches the formatter source' tree "$manifest" /tree "$SANDBOX/merged" content,links

# The device window applies to the tree as to every verb.
offset_embed "$deep" "$SANDBOX/embedded.img" 1048576
export_tree "$SANDBOX/embedded.img" /tree "$SANDBOX/offset" --preserve=links --offset 1048576
check 'an offset export is complete' test "$status" -eq 0
check 'offset export matches the formatter source' tree "$manifest" /tree "$SANDBOX/offset" content,links

# A damaged data span in /tree/big, past the first transfer chunk, found by
# the source's deterministic bytes rather than by this driver's extents.
python3 - "$deep" "$SANDBOX/damaged.img" <<'PY'
import mmap, random, shutil, sys
rng = random.Random(342)
data = bytes(rng.getrandbits(8) for _ in range(3 * 1024 * 1024 + 1))
needle = data[2 * 1024 * 1024:2 * 1024 * 1024 + 4096]
shutil.copyfile(sys.argv[1], sys.argv[2])
with open(sys.argv[2], 'r+b') as f, mmap.mmap(f.fileno(), 0) as image:
    pos = image.find(needle)
    assert pos >= 0, 'the deep fixture has no raw big data; run chore fixtures'
    assert image.find(needle, pos + 1) == -1, 'the raw source span is not unique'
    image[pos + 111] ^= 1
PY
check 'the independent source span was damaged' test $? -eq 0
for replace in no yes; do
    dest="$SANDBOX/damaged-$replace"
    if [ "$replace" = yes ]; then
        mkdir -p "$dest"
        printf 'original destination\n' >"$dest/big"
        export_tree "$SANDBOX/damaged.img" /tree "$dest" --preserve=links --replace
    else
        export_tree "$SANDBOX/damaged.img" /tree "$dest" --preserve=links
    fi
    check "damaged ($replace replace): the tree is partial" test "$status" -eq 1
    check "damaged ($replace replace): only big fails" jreport '.failed == 1 and .completed == 71'
    check "damaged ($replace replace): big is failed" jitem /tree/big '.status == "failed"'
    check "damaged ($replace replace): its siblings are exported" tree "$manifest" /tree "$dest" content,links /tree/big
    if [ "$replace" = yes ]; then
        check 'a failed replacement keeps the original bytes' grep -qx 'original destination' "$dest/big"
    else
        check 'a failed file is never published' test ! -e "$dest/big"
    fi
    leftovers="$(find "$dest" -name '.*.partial' -print)"
    check "damaged ($replace replace): no staging file is left" test -z "$leftovers"
done
check 'export changed no image' cmp -s "$deep" "$SANDBOX/before.img"

# Hard links and times, judged by the reference mount's own lstat. The
# export goes under a `*.noindex` directory, which Spotlight never indexes:
# on the macOS runner its indexer read freshly exported files before the
# check's lstat, moving their access times to the time of the run (#405).
aged="$FIXTURES/aged.img"
mkdir -p "$SANDBOX/aged.noindex"
export_tree "$aged" / "$SANDBOX/aged.noindex/aged" --preserve=links,times
check 'the aged export is complete' test "$status" -eq 0
check 'aged export fails nothing' jreport '.failed == 0 and .refused == 0'
check 'aged export matches the reference mount' tree "$FIXTURES/aged.json" / "$SANDBOX/aged.noindex/aged" content,links,hard,times

if [ -s "$SANDBOX/tree.err" ]; then
    echo "tree mismatches:" >&2
    head -n 40 "$SANDBOX/tree.err" >&2
fi
finish
