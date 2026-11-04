#!/usr/bin/env bash
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

sandbox="$(mktemp -d)"
trap 'rm -r -- "$sandbox"' EXIT
mkdir -p "$sandbox/tests/scripts/support"
cp "$REPO/tests/scripts/host-tools-are-not-assumed.sh" "$sandbox/tests/scripts/"
cp "$REPO/tests/scripts/support/setsid" "$sandbox/tests/scripts/support/"
cat >"$sandbox/tests/scripts/kernel-oracle-boot-is-bounded.sh" <<'GUARD'
#!/usr/bin/env bash
if [ "$NESTED_STATUS" = success ]; then
    echo 'PASS  the nested stand-in completed'
    exit 0
fi
echo 'nested stdout begins'
python3 -c 'import sys; print("FAIL  nested check: " + "x" * 360 + " uncut diagnostic", file=sys.stderr)'
printf '%s\n' 'progress 1' 'progress 2' 'progress 3' 'progress 4'
echo 'nested stdout ends'
exit "$NESTED_STATUS"
GUARD

for nested_status in 7 0 success; do
    out="$(NESTED_STATUS="$nested_status" bash "$sandbox/tests/scripts/host-tools-are-not-assumed.sh" 2>&1)"
    status=$?
    if [ "$nested_status" = success ]; then
        [ "$status" -eq 0 ] || fail "a completed nested guard was rejected: $out"
        [ "${out#PASS}" != "$out" ] || fail 'a completed outer guard lost its PASS line'
        continue
    fi
    [ "$status" -ne 0 ] || fail "a nested guard with status $nested_status was accepted"
    case "$nested_status" in
        7) expected='kernel-oracle-boot-is-bounded exited 7' ;;
        0) expected='kernel-oracle-boot-is-bounded exited 0 without its PASS line' ;;
    esac
    [[ "$out" == *"$expected"* ]] || fail "the nested status $nested_status was not named"
    diagnostic="$(python3 -c 'print("FAIL  nested check: " + "x" * 360 + " uncut diagnostic")')"
    for line in 'nested stdout begins' "$diagnostic" 'progress 1' 'progress 2' 'progress 3' 'progress 4' 'nested stdout ends'; do
        [[ "$out" == *"$line"* ]] || fail "status $nested_status lost a nested output line: $line"
    done
done

[ "$fails" -eq 0 ] || exit 1
echo 'PASS  nested guard failures retain their status and complete stdout and stderr'
