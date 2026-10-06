#!/usr/bin/env bash
# build-fixtures.sh -- build .vm-share/fixtures with the reference tools,
# inside the fs-linux-test-harness VM.
#
# This runs on the host. It is a harness SESSION: the VM it boots comes down
# and the machine-wide slot is released when this script exits, however it
# exits. The work itself is scripts/guest-build-fixtures.sh, run as root in
# the guest, where the reference tools live (scripts/vm-setup.sh).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
harness="$here/../fs-linux-test-harness"
[ -x "$harness/scripts/vm.sh" ] || {
    echo "build-fixtures: ../fs-linux-test-harness is not checked out -- run 'chore siblings'." >&2
    exit 1
}
# shellcheck source=/dev/null
[ -n "${KEEP_VM:-}" ] || source "$harness/scripts/vm-session.sh"
"$harness/scripts/vm.sh" run "bash /repo/scripts/guest-build-fixtures.sh"
share="$("$harness/scripts/vm.sh" share)"
n="$(find "$share/fixtures" -name '*.img' | wc -l | tr -d ' ')"
[ "$n" -gt 0 ] || { echo "build-fixtures: the guest produced no images" >&2; exit 1; }
echo "build-fixtures: $n images in $share/fixtures"
