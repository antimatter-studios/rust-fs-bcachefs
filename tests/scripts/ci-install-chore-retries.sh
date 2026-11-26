#!/usr/bin/env bash
# scripts/ci-install-chore.sh against a local release server (#613): HTTP 504
# answers are retried and the install then succeeds; a server that keeps
# answering 504 fails the step naming the download; and bytes that do not
# match the release's checksums are never installed.
#
#   bash tests/scripts/ci-install-chore-retries.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fails=0
fail() { echo "FAIL  $*" >&2; fails=$((fails + 1)); }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/install-chore.XXXXXX")"
server=
trap '[ -z "$server" ] || kill "$server" 2>/dev/null; rm -rf "$tmp"' EXIT

# Serves the directory's release/; the first N requests for each file answer
# 504, N read from its refusals file, and its port is written beside them.
cat >"$tmp/server.py" <<'PY'
import collections
import functools
import http.server
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
asked = collections.Counter()


class Handler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        asked[self.path] += 1
        if asked[self.path] <= int((root / "refusals").read_text()):
            self.send_error(504)
            return
        super().do_GET()

    def log_message(self, *args):
        pass


handler = functools.partial(Handler, directory=str(root / "release"))
httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
(root / "port.tmp").write_text(str(httpd.server_address[1]))
(root / "port.tmp").rename(root / "port")
httpd.serve_forever()
PY

# A release holding a stand-in chore for whatever host this is.
case "$(uname -s)" in Linux) os=linux ;; *) os=darwin ;; esac
case "$(uname -m)" in x86_64 | amd64) arch=x86_64 ;; *) arch=arm64 ;; esac
tarball="chore-9.9.9-${os}-${arch}.tar.gz"
mkdir -p "$tmp/release" "$tmp/pack"
printf '#!/bin/sh\necho "chore 9.9.9 stand-in"\n' >"$tmp/pack/chore"
chmod +x "$tmp/pack/chore"
tar -czf "$tmp/release/$tarball" -C "$tmp/pack" chore
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$tmp/release" && sha256sum "$tarball" >checksums.txt)
else
    (cd "$tmp/release" && shasum -a 256 "$tarball" >checksums.txt)
fi

# install_with REFUSALS: a new server over its own directory, $run, holding a
# new home and runner directory; leaves status and $run/out.
runs=0
install_with() {
    [ -z "$server" ] || { kill "$server" 2>/dev/null; wait "$server" 2>/dev/null; }
    runs=$((runs + 1))
    run="$tmp/run-$runs"
    mkdir -p "$run/home" "$run/runner"
    ln -s "$tmp/release" "$run/release"
    echo "$1" >"$run/refusals"
    python3 "$tmp/server.py" "$run" &
    server=$!
    for _ in $(seq 1 100); do [ -s "$run/port" ] && break; sleep 0.1; done
    env HOME="$run/home" RUNNER_TEMP="$run/runner" GITHUB_PATH="" CHORE_VERSION=9.9.9 \
        CHORE_RETRY_DELAY=1 CHORE_BASE_URL="http://127.0.0.1:$(cat "$run/port")" \
        bash "$REPO/scripts/ci-install-chore.sh" >"$run/out" 2>&1
    status=$?
}

install_with 2
[ "$status" -eq 0 ] || fail "two 504 answers a file, then success: status $status: $(cat "$run/out")"
[ -x "$run/home/.local/bin/chore" ] || fail "the install left no chore"

install_with 99
[ "$status" -ne 0 ] || fail "a server that only answers 504 installed something"
grep -q "ci-install-chore: cannot download .*$tarball" "$run/out" ||
    fail "a persistent 504 does not name the download: $(cat "$run/out")"
[ ! -e "$run/home/.local/bin/chore" ] || fail "a failed download left a chore installed"

echo "tampered" >>"$tmp/release/$tarball"
install_with 0
[ "$status" -ne 0 ] || fail "a tarball that does not match its checksum was installed"
[ ! -e "$run/home/.local/bin/chore" ] || fail "a mismatched tarball left a chore installed"

[ "$fails" -eq 0 ] || exit 1
echo "PASS  the chore install retries a 504, names a download it cannot make and checks its bytes"
