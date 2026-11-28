#!/usr/bin/env bash
# The reference tools' containers on a podman runner (#745).
#
# The self-hosted runners answer `docker` with podman, whose default seccomp
# profile refuses io_pgetevents with EPERM. The reference tool then fails with
# "io_getevents() error: Operation not permitted", `bcachefs version` included,
# so the install fails on every attempt. Docker's default profile, on a
# GitHub-hosted runner, allows the call.
#
# So scripts/differential-fuzz.sh must start its container so the tool runs
# there, and every other script that starts a long-lived reference container
# must too.
#
#   bash tests/scripts/reference-containers-run-without-seccomp.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/reference-seccomp.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/repo/scripts" "$tmp/fixtures"
# A copy: the script clears tmp/differential under the tree it lives in.
cp "$REPO/scripts/differential-fuzz.sh" "$REPO/scripts/differential-reference-install.sh" "$tmp/repo/scripts/"

# docker as podman answers it: the reference tool fails inside a container
# that was started under the default seccomp profile.
cat >"$tmp/bin/docker" <<'STAND_IN'
#!/usr/bin/env bash
case "$1" in
    run)
        case " $* " in
            *" --security-opt seccomp=unconfined "*) : >"$STATE/unconfined" ;;
        esac
        ;;
    exec)
        if [ ! -f "$STATE/unconfined" ]; then
            echo "io_getevents() error: Operation not permitted"
            exit 1
        fi
        echo "installed"
        ;;
esac
STAND_IN
# The emitter's build, its run, and the judge: nothing to do here.
for tool in cargo timeout python3; do
    printf '#!/usr/bin/env bash\nexit 0\n' >"$tmp/bin/$tool"
done
chmod +x "$tmp/bin/"*

PATH="$tmp/bin:$PATH" STATE="$tmp" BCACHEFS_FIXTURES="$tmp/fixtures" \
    DIFFERENTIAL_KEEP='' DIFFERENTIAL_LISTINGS='' DIFFERENTIAL_READ_EVIDENCE='' \
    DIFFERENTIAL_INSTALL_PAUSE=0 \
    bash "$tmp/repo/scripts/differential-fuzz.sh" 1 0 >"$tmp/out" 2>&1
status=$?
[ "$status" -eq 0 ] ||
    fail "the differential run did not install on a podman runner (status $status): $(tail -n 1 "$tmp/out")"

# Every `docker run -d` in scripts/, its continuation lines joined.
started=0
for script in "$REPO"/scripts/*.sh; do
    while IFS= read -r command; do
        started=$((started + 1))
        case "$command" in
            *"--security-opt seccomp=unconfined "*) ;;
            *) fail "${script#"$REPO"/} starts a reference container under the default seccomp profile: $command" ;;
        esac
    done < <(awk '{ if (sub(/\\$/, "")) { line = line $0; next } print line $0; line = "" }' "$script" |
        grep -E '^[[:space:]]*docker run -d ')
done
# differential-fuzz.sh, pinned-snapshot-tree-subvolume.sh and the two in
# pinned-default74-reference.sh: fewer means the search above stopped matching.
[ "$started" -ge 4 ] || fail "found $started reference container start(s), expected at least 4"

[ "$fails" -eq 0 ] || exit 1
echo "PASS  every long-lived reference container runs the tool on a podman runner"
