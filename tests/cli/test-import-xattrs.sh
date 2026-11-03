#!/usr/bin/env bash
# fs.bcachefs import --preserve=xattrs (#443), as installed, on copies of the
# write-study base and the aged image: the extended attributes of a host file
# or directory are set on what is imported, and a namespace bcachefs cannot
# hold refuses its item by name after the contents are published. Attributes
# are read back here through export, which only shows the verb's contract;
# the reference mount's listing of the same imports is tests/write_oracle.rs,
# in the guest.
source "$(dirname "$0")/lib.sh"
need_image write-study/base
need_fixture aged

# xattrs.py list|put|acl: the host's own attribute calls. Linux goes through
# os.setxattr; macOS, which Python gives no xattr calls, through libc. Neither
# follows a symlink.
#   list PATH            sorted `name=hex` joined by `;`, com.apple.* left out
#   put PATH NAME HEX    sets NAME to the bytes HEX spells
#   acl PATH             sets an access ACL, which Linux keeps as the attribute
#                        system.posix_acl_access
#   stamped PATH         exits 0 when PATH carries com.apple.provenance
cat >"$SANDBOX/xattrs.py" <<'PY'
import ctypes, os, struct, sys

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

cmd, *args = sys.argv[1:]
if cmd == "list":
    print(";".join(sorted(f"{n}={get(args[0], n).hex()}" for n in names(args[0]) if not n.startswith("com.apple."))))
elif cmd == "put":
    put(args[0], args[1], bytes.fromhex(args[2]))
elif cmd == "acl":
    undefined = 0xffffffff
    entries = [(1, 6, undefined), (2, 4, 1), (4, 4, undefined), (0x10, 4, undefined), (0x20, 4, undefined)]
    put(args[0], "system.posix_acl_access",
        struct.pack("<I", 2) + b"".join(struct.pack("<HHI", *e) for e in entries))
elif cmd == "stamped":
    sys.exit(0 if "com.apple.provenance" in names(args[0]) else 1)
PY
xattrs() { python3 "$SANDBOX/xattrs.py" "$@"; }
# run ARG...: an import on $img, its report in $SANDBOX/report and its status
# in $status.
run() {
    fs.bcachefs "$img" import "$@" >"$SANDBOX/report" 2>"$SANDBOX/err"
    status=$?
}
jreport() { jq -e "$1" "$SANDBOX/report" >/dev/null; }
jitem() { jq -c --arg d "$1" '.items[] | select(.destination == $d)' "$SANDBOX/report" | jq -e "$2" >/dev/null; }
# carried IMAGE PATH: PATH's attributes in the image, spelled as `list` spells
# them, through an export to a directory of its own.
carried() {
    local out
    out="$(mktemp -d "$SANDBOX/carried.XXXXXX")/item"
    fs.bcachefs "$1" export --recursive --preserve=xattrs "$2" "$out" >/dev/null 2>&1
    xattrs list "$out"
}
reads() { [ "$(fs.bcachefs "$img" read "$1" 2>/dev/null)" = "$2" ]; }

fs.bcachefs image.img import --help >"$SANDBOX/help" 2>"$SANDBOX/err"
check 'help has an attributes example' grep -q 'fs.bcachefs disk.img import --preserve=xattrs ' "$SANDBOX/help"

# Attributes on a file: text, an empty value, bytes with a NUL in them, a long
# name, and a value of 1500 bytes.
long_name="user.$(python3 -c 'print("n" * 100, end="")')"
long_value="$(python3 -c 'print("ab" * 1500, end="")')"
one="$SANDBOX/src.noindex/one"
mkdir -p "$(dirname "$one")"
printf 'attributed\n' >"$one"
xattrs put "$one" user.colour "$(printf 'blue' | od -An -tx1 | tr -d ' \n')"
xattrs put "$one" user.empty ""
xattrs put "$one" user.binary 00ff0001
xattrs put "$one" "$long_name" 6c6f6e67
xattrs put "$one" user.big "$long_value"
want_one="$(xattrs list "$one")"
check 'the source carries five attributes' test "$(printf '%s\n' "$want_one" | tr ';' '\n' | wc -l | tr -d ' ')" -eq 5

# Whether a new file here is stamped decides what every item must do. macOS
# stamps what a tool with a provenance of its own creates with
# com.apple.provenance, which no call removes, and bcachefs cannot hold that
# name. A stamped item is refused by it, and the host's own attribute order
# means nothing else of that item is set, so a stamped host checks the refusal
# and leaves the values to an unstamped one. Probed on a file of our own, never
# assumed.
# An empty file proves nothing: the stamp arrives with the first write.
printf 'probe\n' >"$SANDBOX/src.noindex/probe"
if xattrs stamped "$SANDBOX/src.noindex/probe"; then stamped=yes; else stamped=no; fi
echo "this host stamps new files: $stamped"

attributes_are_carried() {
    # One file, no --recursive, onto an image with and one without attributes
    # already: the first attribute of a filesystem makes its btree's root.
    for image in write-study/base aged; do
        img="$SANDBOX/$(basename "$image").img"
        cp "$FIXTURES/$image.img" "$img"
        run --preserve=xattrs "$one" /imported-one
        check "$image: one file imports with its attributes ($(cat "$SANDBOX/err"))" test "$status" -eq 0
        check "$image: it is one completed item" jreport '.completed == 1 and .refused == 0 and .failed == 0'
        check "$image: its contents arrive" reads /imported-one attributed
        check "$image: its attributes arrive, names and values" test "$(carried "$img" /imported-one)" = "$want_one"
    done
    img="$SANDBOX/base.img"

    # Without the flag no attribute is read or set, on one file or on a tree.
    run "$one" /no-flag
    check "without the flag it imports ($(cat "$SANDBOX/err"))" test "$status" -eq 0
    check 'without the flag nothing is carried' test -z "$(carried "$img" /no-flag)"
    mkdir -p "$SANDBOX/src.noindex/plain-tree"
    cp "$one" "$SANDBOX/src.noindex/plain-tree/file"
    xattrs put "$SANDBOX/src.noindex/plain-tree/file" user.colour 626c7565
    run --recursive "$SANDBOX/src.noindex/plain-tree" /no-flag-tree
    check "a tree without the flag imports ($(cat "$SANDBOX/err"))" test "$status" -eq 0
    check 'a tree without the flag carries nothing' test -z "$(carried "$img" /no-flag-tree/file)"

    # A tree: a file's and a directory's own, and a directory already in the image
    # that the tree merges into.
    tree="$SANDBOX/src.noindex/tree"
    mkdir -p "$tree/sub"
    printf 'in a tree\n' >"$tree/sub/file"
    xattrs put "$tree/sub/file" user.where 66696c65
    xattrs put "$tree/sub" user.where 646972
    xattrs put "$tree" user.where 726f6f74
    run --recursive --preserve=xattrs "$tree" /d
    check "a tree imports with its attributes ($(cat "$SANDBOX/err"))" test "$status" -eq 0
    check 'the tree is complete' jreport '.completed == 3 and .refused == 0 and .failed == 0 and .merged_directories == 1'
    check 'a file in the tree carries its attribute' test "$(carried "$img" /d/sub/file)" = 'user.where=66696c65'
    check 'a directory in the tree carries its own' test "$(carried "$img" /d/sub)" = 'user.where=646972'
    check 'a merged directory takes the source root attribute' test "$(carried "$img" /d)" = 'user.where=726f6f74'

    # A second import of the same names replaces a value and keeps the others.
    xattrs put "$tree/sub/file" user.where 6e6577
    xattrs put "$tree/sub" user.second 32
    run --recursive --replace --preserve=xattrs "$tree" /d
    check "a second tree imports ($(cat "$SANDBOX/err"))" test "$status" -eq 0
    check 'a replaced file has the new value' test "$(carried "$img" /d/sub/file)" = 'user.where=6e6577'
    check 'a merged directory has the old and the new' test "$(carried "$img" /d/sub)" = 'user.second=32;user.where=646972'

    # A namespace bcachefs cannot hold refuses its item, naming the attribute,
    # with a nonzero exit; the contents are published and the sibling goes in.
    # macOS lets any user set a name with no namespace; Linux lets a file's owner
    # set the ACL it keeps in the system namespace.
    held="$SANDBOX/src.noindex/held"
    mkdir -p "$held"
    printf 'cannot keep\n' >"$held/refused"
    printf 'can keep\n' >"$held/zz-sibling"
    xattrs put "$held/zz-sibling" user.fine 6f6b
    case "$(uname -s)" in
        Darwin)
            xattrs put "$held/refused" net.example.note 6e6f7465
            attribute=net.example.note
            ;;
        *)
            xattrs acl "$held/refused"
            attribute=system.posix_acl_access
            ;;
    esac
    check "the source carries $attribute" grep -q "$attribute" <(xattrs list "$held/refused")
    run --recursive --preserve=xattrs "$held" /held
    check 'a refused namespace makes the tree partial' test "$status" -ne 0
    check 'one item is refused' jreport '.refused == 1 and .failed == 0 and .completed == 2'
    check 'the refusal names the attribute' jitem /held/refused ".status == \"refused\" and (.reason | contains(\"$attribute\"))"
    check 'the refusal names the namespaces' jitem /held/refused '.reason | contains("user and trusted")'
    check 'its contents are published' reads /held/refused 'cannot keep'
    check 'the sibling is imported' reads /held/zz-sibling 'can keep'
    check 'the sibling carries its attribute' test "$(carried "$img" /held/zz-sibling)" = 'user.fine=6f6b'
    run --preserve=xattrs "$held/refused" /held-one
    check 'one file with a refused namespace exits nonzero' test "$status" -ne 0
    check 'it is refused by name' jitem /held-one ".status == \"refused\" and (.reason | contains(\"$attribute\"))"

    # A value one key cannot hold (about 2 KiB) is refused too, never truncated.
    huge="$SANDBOX/src.noindex/huge"
    printf 'huge\n' >"$huge"
    xattrs put "$huge" user.huge "$(python3 -c 'print("ab" * 3000, end="")')"
    run --preserve=xattrs "$huge" /huge-attr
    check 'a value one key cannot hold exits nonzero' test "$status" -ne 0
    check 'it is refused by name' jitem /huge-attr '.status == "refused" and (.reason | contains("user.huge"))'
    check 'nothing of it is set' test -z "$(carried "$img" /huge-attr)"
}

attributes_are_refused_by_the_stamp() {
    for image in write-study/base aged; do
        img="$SANDBOX/$(basename "$image").img"
        cp "$FIXTURES/$image.img" "$img"
        run --preserve=xattrs "$one" /imported-one
        check "$image: a stamped file exits nonzero" test "$status" -ne 0
        check "$image: it is refused naming the stamp" jitem /imported-one '.status == "refused" and (.reason | contains("com.apple.provenance"))'
        check "$image: its contents are published" reads /imported-one attributed
    done
    img="$SANDBOX/base.img"
    mkdir -p "$SANDBOX/src.noindex/stamped-tree"
    printf 'stamped\n' >"$SANDBOX/src.noindex/stamped-tree/file"
    run --recursive --preserve=xattrs "$SANDBOX/src.noindex/stamped-tree" /stamped-tree
    check 'a stamped tree exits nonzero' test "$status" -ne 0
    check 'its directory and its file are both refused' jreport '.refused == 2 and .failed == 0'
    check 'its file is published all the same' reads /stamped-tree/file stamped
    run --recursive "$SANDBOX/src.noindex/stamped-tree" /unattributed-tree
    check "without the flag the stamp is not read ($(cat "$SANDBOX/err"))" test "$status" -eq 0
}

if [ "$stamped" = yes ]; then
    attributes_are_refused_by_the_stamp
else
    attributes_are_carried
fi

# What attributes cannot apply to is an error before the image is changed.
cp "$img" "$SANDBOX/before.img"
printf 'x' | fs.bcachefs "$img" import --preserve=xattrs - /stdin-attrs >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'stdin has no attributes to carry' test $? -eq 2
fs.bcachefs "$img" import --preserve=xattrs --verify "$one" /verified >"$SANDBOX/out" 2>"$SANDBOX/err"
check '--verify does not combine with --preserve' test $? -eq 2
check 'the refusal names --verify' grep -q -- '--verify' "$SANDBOX/err"
fs.bcachefs "$img" import --recursive --preserve=xattrs,sparse "$one" /both >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'holes and attributes together are refused whole' test $? -eq 3
check 'the refusals change no byte of the image' cmp -s "$img" "$SANDBOX/before.img"

finish
