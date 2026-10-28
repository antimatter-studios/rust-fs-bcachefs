#!/usr/bin/env bash
# fs.bcachefs export --preserve=xattrs (#442), as installed, against the
# extended attributes the reference implementation's mount listed for the
# aged image's /xattr tree, never against this driver's own reads:
#   - a recursive and a single-file export carry every attribute to the
#     host, name and value;
#   - a namespace the host cannot hold (`trusted.` without privilege) is
#     refused for that item, naming the attribute, with a nonzero exit, and
#     never dropped; a host that holds it (root, or macOS, which has no
#     namespaces) stores it like any other;
#   - without --preserve=xattrs nothing is carried, and attributes cannot
#     go to standard output.
source "$(dirname "$0")/lib.sh"
need_fixture aged

aged="$FIXTURES/aged.img"
manifest="$FIXTURES/aged.json"

# xattrs.py list|probe|compare ...: the host's own view of exported files.
#   list PATH              prints PATH's attributes as the manifest spells
#                          them: sorted `name=hex`, joined by `;`
#   probe PATH             exits 0 when the host stores a `trusted.`
#                          attribute on PATH, 1 when it refuses one
#   compare ROOT PREFIX [SKIP...]
#                          every manifest entry below and at PREFIX, exported
#                          at ROOT, carries exactly the manifest's attributes
# Linux reads through os.listxattr; macOS, which Python gives no xattr calls,
# through libc. Neither follows a symlink. macOS's own com.apple.* bookkeeping
# is not the image's and is left out.
cat >"$SANDBOX/xattrs.py" <<'PY'
import ctypes, json, os, sys

if hasattr(os, "listxattr"):
    def names(p):
        return os.listxattr(p, follow_symlinks=False)
    def get(p, n):
        return os.getxattr(p, n, follow_symlinks=False)
    def put(p, n, v):
        os.setxattr(p, n, v, follow_symlinks=False)
else:
    libc = ctypes.CDLL(None, use_errno=True)
    NOFOLLOW = 1
    def fail(p):
        e = ctypes.get_errno()
        raise OSError(e, os.strerror(e), p)
    def names(p):
        b = os.fsencode(p)
        size = libc.listxattr(b, None, 0, NOFOLLOW)
        if size < 0:
            fail(p)
        buf = ctypes.create_string_buffer(size)
        size = libc.listxattr(b, buf, size, NOFOLLOW)
        if size < 0:
            fail(p)
        return [n.decode() for n in buf.raw[:size].split(b"\0") if n]
    def get(p, n):
        b, k = os.fsencode(p), n.encode()
        size = libc.getxattr(b, k, None, 0, 0, NOFOLLOW)
        if size < 0:
            fail(p)
        buf = ctypes.create_string_buffer(max(size, 1))
        size = libc.getxattr(b, k, buf, size, 0, NOFOLLOW)
        if size < 0:
            fail(p)
        return buf.raw[:size]
    def put(p, n, v):
        if libc.setxattr(os.fsencode(p), n.encode(), v, len(v), 0, NOFOLLOW) < 0:
            fail(p)

def listing(p):
    got = sorted(f"{n}={get(p, n).hex()}" for n in names(p) if not n.startswith("com.apple."))
    return ";".join(got) or None

cmd, *args = sys.argv[1:]
if cmd == "list":
    print(listing(args[0]) or "")
elif cmd == "probe":
    try:
        put(args[0], "trusted.probe", b"x")
    except OSError:
        sys.exit(1)
elif cmd == "compare":
    root, prefix, *skip = args
    prefix = prefix.rstrip("/")
    entries = [e for e in json.load(open(os.environ["MANIFEST"]))["entries"]
               if (e["path"] == prefix or e["path"].startswith(prefix + "/"))
               and e["path"] not in skip]
    assert entries, f"no manifest entry at or below {prefix}"
    bad = []
    for e in entries:
        local = os.path.join(root, e["path"][len(prefix) + 1:])
        try:
            got = listing(local)
        except OSError as err:
            bad.append(f"{e['path']}: {err}")
            continue
        if got != e.get("xattrs"):
            bad.append(f"{e['path']}: {got!r}, not {e.get('xattrs')!r}")
    for b in bad[:20]:
        print(b, file=sys.stderr)
    sys.exit(1 if bad else 0)
PY
xattrs() { MANIFEST="$manifest" python3 "$SANDBOX/xattrs.py" "$@" 2>>"$SANDBOX/xattrs.err"; }
want() { jq -r --arg p "$1" '.entries[] | select(.path == $p) | .xattrs // ""' "$manifest"; }

# export_to DEST [FLAG...] PATH: an export, its report in $SANDBOX/report
# and its status in $status.
export_to() {
    local dest="$1"
    shift
    fs.bcachefs "$aged" export "$@" "$dest" >"$SANDBOX/report" 2>"$SANDBOX/err"
    status=$?
}
jreport() { jq -e "$1" "$SANDBOX/report" >/dev/null; }
jitem() { jq -c --arg p "$1" '.items[] | select(.path == $p)' "$SANDBOX/report" | jq -e "$2" >/dev/null; }

check 'the manifest records attributes on five /xattr paths' \
    jq -e '[.entries[] | select((.path | startswith("/xattr")) and .xattrs != null)] | length == 5' "$manifest"
check 'the manifest records a trusted attribute on /xattr/large.txt' \
    jq -e '.entries[] | select(.path == "/xattr/large.txt") | .xattrs | contains("trusted.marker=")' "$manifest"

# Whether this host stores a trusted.* attribute decides what large.txt
# must do: the kernel refuses one without CAP_SYS_ADMIN, macOS takes any
# name. Probed on a file of our own, never assumed.
mkdir -p "$SANDBOX/out.noindex"
: >"$SANDBOX/out.noindex/probe"
if xattrs probe "$SANDBOX/out.noindex/probe"; then trusted=stored; else trusted=refused; fi
echo "this host's trusted namespace: $trusted"

# The tree, every attribute the reference mount listed.
tree="$SANDBOX/out.noindex/tree"
export_to "$tree" --recursive --preserve=xattrs /xattr
if [ "$trusted" = stored ]; then
    check 'the attributed tree is complete' test "$status" -eq 0
    check 'tree counts' jreport '.completed == 5 and .refused == 0 and .failed == 0'
    check 'every exported attribute matches the reference mount' xattrs compare "$tree" /xattr
else
    check 'a refused attribute makes the tree partial' test "$status" -eq 1
    check 'tree counts' jreport '.completed == 4 and .refused == 1 and .failed == 0'
    check 'large.txt is refused naming trusted.marker' \
        jitem /xattr/large.txt '.status == "refused" and (.reason | contains("trusted.marker"))'
    check 'every other attribute matches the reference mount' xattrs compare "$tree" /xattr /xattr/large.txt
    check 'the refused file is still published' \
        test "$(wc -c <"$tree/large.txt" | tr -d ' ')" = "$(jq -r '.entries[] | select(.path == "/xattr/large.txt") | .size' "$manifest")"
fi
for p in /xattr/one.txt /xattr/many.txt /xattr/removed.txt; do
    check "$p is completed" jitem "$p" '.status == "completed"'
done
check 'the directory carries its own attribute' test "$(xattrs list "$tree")" = "$(want /xattr)"

# One file, without --recursive.
export_to "$SANDBOX/out.noindex/many.txt" --preserve=xattrs /xattr/many.txt
check 'a single attributed file is exported' test "$status" -eq 0
check 'the single file carries the reference attributes' \
    test "$(xattrs list "$SANDBOX/out.noindex/many.txt")" = "$(want /xattr/many.txt)"
export_to "$SANDBOX/out.noindex/large.txt" --preserve=xattrs /xattr/large.txt
if [ "$trusted" = stored ]; then
    check 'a single trusted file is exported' test "$status" -eq 0
    check 'the single trusted file carries the reference attributes' \
        test "$(xattrs list "$SANDBOX/out.noindex/large.txt")" = "$(want /xattr/large.txt)"
else
    check 'a single trusted file is refused' test "$status" -eq 1
    check 'the single refusal names trusted.marker' grep -q 'trusted.marker' "$SANDBOX/report"
fi

# Nothing is carried unasked, and attributes have nowhere to go on stdout.
export_to "$SANDBOX/out.noindex/plain.txt" /xattr/one.txt
check 'a plain export succeeds' test "$status" -eq 0
check 'a plain export carries no attribute' test -z "$(xattrs list "$SANDBOX/out.noindex/plain.txt")"
fs.bcachefs "$aged" export --preserve=xattrs /xattr/one.txt - >"$SANDBOX/stdout" 2>"$SANDBOX/err"
check 'attributes to standard output are refused' test $? -ne 0
check 'the refusal writes no bytes' test ! -s "$SANDBOX/stdout"

fs.bcachefs disk.img export --help >"$SANDBOX/help" 2>"$SANDBOX/err"
check 'export --help shows --preserve=xattrs' grep -q 'fs.bcachefs disk.img export --recursive --preserve=xattrs ' "$SANDBOX/help"

if [ -s "$SANDBOX/xattrs.err" ]; then
    echo "attribute mismatches:" >&2
    head -n 40 "$SANDBOX/xattrs.err" >&2
fi
finish
