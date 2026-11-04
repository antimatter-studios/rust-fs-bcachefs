#!/usr/bin/env bash
# The crc128-overwrite step either produces a crc128 entry with a nonzero
# offset or fails the fixture build, naming the entry and saying why (#577).
#
# Without this, a build with no such entry reports success, and every later
# job that reads the fixtures fails on the same missing precondition.
#
# This runs the real step, selected from scripts/guest-build-fixtures.sh with
# the real supervisor and mount helpers, over stand-ins for the reference tools
# and the mount. It needs no VM. The stand-ins' listings are what the reference
# lister printed for a good and a bad build (merge-queue runs 41789838751 and
# 41795639102). This checks the step's logic, not the format.
#
#   bash tests/scripts/crc128-overwrite-is-judged-at-the-fixture-step.sh
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d "${TMPDIR:-/tmp}/crc128-overwrite-step.XXXXXX")"
trap 'rm -rf -- "$sandbox"' EXIT

# The real step, with the anchors the reproducer selects it by.
step_script="$sandbox/step.sh"
python3 -B - "$repo" "$step_script" <<'PY' || { echo "FAIL  the step could not be selected" >&2; exit 1; }
import importlib.util, sys
repo, dest = sys.argv[1], sys.argv[2]
spec = importlib.util.spec_from_file_location("repro", repo + "/scripts/guest-repro-crc128-overwrite.py")
repro = importlib.util.module_from_spec(spec)
spec.loader.exec_module(repro)
source = open(repo + "/scripts/guest-build-fixtures.sh").read()
parts = [
    repro.section(source, "#!/usr/bin/env bash\n", "# name | formatter options\n"),
    repro.section(source, "fuse_mount() { # IMAGE OPTIONS LOG\n",
                  'step "aged (mounted and aged by the reference implementation)"\n'),
    repro.section(source,
                  'echo "== crc128-overwrite (a crc128 extent partly overwritten, through the mount)"\n',
                  "# POISONED EXTENTS."),
]
open(dest, "w").write("".join(parts))
PY
printf 'step() { :; }\n' >"$sandbox/watchdog.sh"

# The stand-ins: one script, named for each tool it replaces.
tools="$sandbox/bin"
mkdir "$tools"
cat >"$tools/stand-in" <<'STANDIN'
#!/usr/bin/env bash
set -u
tool="$(basename "$0")"
mounts() { grep -c '^mount ' "$FIXTURE_CASE/events" 2>/dev/null || true; }
case "$tool" in
    bcachefs-ref)
        op="$1"
        shift
        case "$op" in
            format) ;;
            fusemount)
                # fusemount -f -o OPTIONS IMAGE MOUNTPOINT
                options="$3" image="$4" mountpoint="$5"
                number=$(($(mounts) + 1))
                echo "mount $image $options" >>"$FIXTURE_CASE/events"
                if [ "$FIXTURE_SCENARIO" = second-mount-refused ] && [ "$number" = 2 ]; then
                    echo "stand-in refusal of the second mount" >&2
                    exit 42
                fi
                printf '%s\n' "$image" >"$BCACHEFS_REF_ROOT$mountpoint/.mounted-from"
                if [ "$FIXTURE_SCENARIO" = other-bytes ] && [ "$number" = 3 ]; then
                    printf 'x' >>"$BCACHEFS_REF_ROOT$mountpoint/f"
                fi
                : >"$FIXTURE_CASE/mounted"
                exec /bin/sleep 300
                ;;
            fsck) echo "stand-in checker" ;;
            list)
                # list -b BTREE IMAGE
                [ "$2" = extents ] || { echo "stand-in listing"; exit 0; }
                written='  crc128: c_size 1024 size 1024 offset 0 nonce 0 csum crc32c 0:f1c6c44  compress none'
                if [ "$(mounts)" = 1 ]; then
                    [ "$FIXTURE_SCENARIO" = no-crc128-written ] &&
                        written='  crc64: c_size 512 size 512 offset 0 nonce 0 csum crc32c 0:f1c6c44  compress none'
                    echo "$written"
                    exit 0
                fi
                case "$FIXTURE_SCENARIO" in
                    narrowed)
                        echo '  crc128: c_size 8 size 8 offset 0 nonce 0 csum crc32c 0:26bafc  compress none'
                        echo '  crc128: c_size 1008 size 1008 offset 0 nonce 0 csum crc32c 0:6630f855  compress none'
                        ;;
                    no-crc128-written)
                        echo '  crc32: c_size 8 size 8 offset 0 nonce 0 csum crc32c 0:ad6a78d3  compress none'
                        ;;
                    *)
                        echo '  crc128: c_size 1024 size 1024 offset 0 nonce 0 csum crc32c 0:f1c6c44  compress none'
                        echo '  crc128: c_size 1024 size 1024 offset 16 nonce 0 csum crc32c 0:f1c6c44  compress none'
                        ;;
                esac
                ;;
            *) echo "stand-in $op" ;;
        esac
        ;;
    findmnt) [ -f "$FIXTURE_CASE/mounted" ] && echo 1000 ;;
    mountpoint) [ -f "$FIXTURE_CASE/mounted" ] ;;
    fusermount3 | umount) rm -f "$FIXTURE_CASE/mounted" ;;
    cp)
        # cp --sparse=always SOURCE DESTINATION
        /bin/cp "$2" "$3"
        ;;
    truncate)
        # truncate -s SIZE PATH
        printf 'stand-in image\n' >"$3"
        ;;
    sleep | sync) ;;
    python3)
        # The real interpreter, with what the manifest calls and macOS lacks.
        exec "$FIXTURE_PYTHON" -B -c '
import os, sys
os.listxattr = getattr(os, "listxattr", lambda *args, **kwargs: [])
sys.argv = sys.argv[1:]
exec(compile(sys.stdin.read(), "<stdin>", "exec"))' "$@"
        ;;
esac
STANDIN
chmod +x "$tools/stand-in"
for tool in bcachefs-ref findmnt mountpoint fusermount3 umount cp truncate sleep sync python3; do
    ln -s stand-in "$tools/$tool"
done

FIXTURE_PYTHON="$(command -v python3)"
export FIXTURE_PYTHON
status=0 output='' transcript='' case_dir=''
# run_case NAME: the step over stand-ins, bounded. setsid and timeout come from
# the support directory only on a host that has neither.
run_case() {
    case_dir="$sandbox/$1"
    mkdir -p "$case_dir/ref" "$case_dir/out"
    status=0
    output="$(
        PATH="$tools:$PATH:$repo/tests/scripts/support" \
            FIXTURE_CASE="$case_dir" FIXTURE_SCENARIO="$1" \
            BCACHEFS_REF_ROOT="$case_dir/ref" BCACHEFS_FIXTURE_OUT="$case_dir/out" \
            BCACHEFS_FIXTURE_WATCHDOG="$sandbox/watchdog.sh" \
            timeout --kill-after=5s 120s bash "$step_script" 2>&1
    )" || status=$?
    transcript="$(cat "$case_dir/out/crc128-overwrite.txt" 2>/dev/null)"
}

# refused NAME TEXT...: the step fails, naming the entry in its output, and the
# transcript holds each TEXT.
refused() {
    local name="$1" text
    shift
    run_case "$name"
    if [ "$status" = 0 ]; then
        fail "$name: the step exited 0, so the build goes on without the entry"
        return
    fi
    case "$output" in
        *"crc128-overwrite"*"no crc128 entry with a nonzero offset"*) ;;
        *) fail "$name: the build's output does not name the entry: $output" ;;
    esac
    for text in "$@"; do
        case "$transcript" in
            *"$text"*) ;;
            *) fail "$name: crc128-overwrite.txt does not say: $text"$'\n'"$transcript" ;;
        esac
    done
}

# A build that holds the entry goes on, and the fixture stays as listed: only
# the two writes mount it, and the manifest is read through a mount of a copy.
# A read through a writable mount of the fixture is what removes the offset.
run_case good
[ "$status" = 0 ] || fail "good: the step failed with the entry present (status $status): $output"
case "$transcript" in
    *"offset 16"*) ;;
    *) fail "good: crc128-overwrite.txt does not show the entry: $transcript" ;;
esac
[ -f "$case_dir/out/crc128-overwrite.img" ] || fail "good: the image was not published"
python3 -B - "$case_dir" <<'PY' || fail "good: the fixture is exposed to the manifest's read"
import hashlib, json, sys
case = sys.argv[1]
mounts = [l.split() for l in open(case + "/events") if l.startswith("mount ")]
fixture = [m for m in mounts if m[1].endswith("/crc128-overwrite.img")]
if len(fixture) != 2:
    sys.exit(f"the fixture image was mounted {len(fixture)} times, not once per write: {mounts}")
entries = json.load(open(case + "/out/crc128-overwrite.json"))["entries"]
read = next(e["sha256"] for e in entries if e["path"] == "/.mounted-from")
source = [m for m in mounts if hashlib.sha256((m[1] + "\n").encode()).hexdigest() == read]
if not source:
    sys.exit("the manifest was read through no mount this run made")
if source[-1][1].endswith("/crc128-overwrite.img"):
    sys.exit("the manifest was read through a mount of the fixture image itself")
if "ro" not in source[-1][2].split(","):
    sys.exit(f"the manifest was read through a writable mount: {source[-1]}")
PY

# The listing after the overwrite holds crc128 entries, all at offset 0: what
# merge-queue run 41795639102 built.
refused narrowed "no crc128 entry with a nonzero offset" "all at offset 0" \
    "c_size 8 size 8 offset 0" "c_size 1008 size 1008 offset 0" "c_size 1024 size 1024 offset 0"

# The first write gave the overwrite nothing to offset.
refused no-crc128-written "no crc128 entry with a nonzero offset" "the first write left no crc128 entry"

# The steps stopped, so there is no listing to judge: the reason is the refusal.
refused second-mount-refused "no crc128 entry with a nonzero offset" "stopped before the last listing" \
    "stand-in refusal of the second mount"

# The manifest no longer sees the writes themselves, so it is held to them.
run_case other-bytes
if [ "$status" = 0 ]; then
    fail "other-bytes: the step exited 0 on a read-back that differs from the two writes"
else
    case "$transcript" in
        *"other bytes than the two writes"*) ;;
        *) fail "other-bytes: crc128-overwrite.txt does not say so: $transcript" ;;
    esac
fi

# A failed build packs no fixtures, so the record has to be kept apart.
python3 -B - "$repo/.github/workflows/ci.yml" <<'PY' || fail "ci.yml does not keep crc128-overwrite's record when the fixtures job fails"
import re, sys
text = open(sys.argv[1]).read()
job = text[text.index("\n  fixtures:\n"):text.index("\n  oracle:\n")]
kept = [s for s in re.split(r"\n      - ", job) if "crc128-overwrite" in s]
sys.exit(not any("failure()" in s and "upload-artifact" in s and ".vm-share/fixtures/crc128-overwrite" in s for s in kept))
PY

[ "$fails" = 0 ] || exit 1
echo "crc128-overwrite-is-judged-at-the-fixture-step: all checks passed"
