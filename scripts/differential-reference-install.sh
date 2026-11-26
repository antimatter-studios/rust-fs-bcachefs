#!/usr/bin/env bash
# Install the reference tools in the differential run's container.
#
#   bash scripts/differential-reference-install.sh CONTAINER VERSION LOG
#
# The install fetches from the Debian and reference package mirrors, and one
# failed attempt used to fail the whole nightly run (#717). So it is tried
# DIFFERENTIAL_INSTALL_ATTEMPTS times (3), DIFFERENTIAL_INSTALL_PAUSE seconds
# apart (30). Every attempt's output goes to LOG; when the last one fails, the
# end of LOG is printed, because the log stays on the runner.
set -uo pipefail

if [ "$#" -ne 3 ]; then
    echo "usage: $0 CONTAINER VERSION LOG" >&2
    exit 2
fi
ctr="$1"
version="$2"
log="$3"
attempts="${DIFFERENTIAL_INSTALL_ATTEMPTS:-3}"
pause="${DIFFERENTIAL_INSTALL_PAUSE:-30}"

: >"$log"
attempt=1
while :; do
    echo "== attempt $attempt of $attempts" >>"$log"
    docker exec -e REF_VERSION="$version" "$ctr" bash -euc '
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y -qq --no-install-recommends ca-certificates curl >/dev/null
        install -d -m 0755 /etc/apt/keyrings
        curl -fsSL -o /etc/apt/keyrings/apt.bcachefs.org.asc https://apt.bcachefs.org/apt.bcachefs.org.asc
        echo "deb [signed-by=/etc/apt/keyrings/apt.bcachefs.org.asc] https://apt.bcachefs.org/trixie bcachefs-tools-release main" \
            >/etc/apt/sources.list.d/apt.bcachefs.org.list
        apt-get update -qq
        apt-get install -y -qq --no-install-recommends "bcachefs-tools=1:$REF_VERSION" >/dev/null
        bcachefs version
    ' >>"$log" 2>&1
    status=$?
    [ "$status" -eq 0 ] && exit 0
    [ "$attempt" -lt "$attempts" ] || break
    echo "differential: the reference tools did not install (attempt $attempt of $attempts, status $status); trying again in $pause s" >&2
    sleep "$pause"
    attempt=$((attempt + 1))
done

echo "differential: the reference tools did not install in $attempts attempts (status $status); the end of $log:" >&2
tail -n 30 "$log" >&2
exit 1
