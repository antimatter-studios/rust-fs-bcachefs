#!/usr/bin/env bash
# CI host entry point. Reference tools run only inside the harness guest.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
harness="$here/../fs-linux-test-harness"
[ -x "$harness/scripts/vm.sh" ] || {
    echo "crc128 repro: run 'chore siblings' to provide the harness" >&2
    exit 1
}
# shellcheck source=/dev/null
source "$harness/scripts/vm-session.sh"
"$harness/scripts/vm.sh" run \
    "BCACHEFS_FIXTURE_OUT=/share/crc128-native-repro timeout --signal=TERM --kill-after=20s 240s python3 /repo/scripts/guest-repro-crc128-overwrite.py"
