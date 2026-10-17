#!/usr/bin/env bash
# The shared read verb is documented by the same command that parses it.
source "$(dirname "$0")/lib.sh"

for verb in read cat; do
    fs.bcachefs image.img "$verb" --help >"$SANDBOX/help" 2>"$SANDBOX/err"
    check "$verb --help succeeds without opening an image" test $? -eq 0
    check "$verb help documents -o" grep -q -- '-o, --output' "$SANDBOX/help"
    check "$verb help explains symlink refusal" grep -qi 'symlink' "$SANDBOX/help"
    check "$verb help has an extraction example" grep -q 'fs.bcachefs disk.img read /hello.txt -o hello.txt' "$SANDBOX/help"
done

rust-fs-bcachefs generate man "$SANDBOX/share" >"$SANDBOX/out" 2>"$SANDBOX/err"
check "man page generation succeeds" test $? -eq 0
check "the read man page exists" test -s "$SANDBOX/share/man/man1/fs.bcachefs-read.1"
check "the read man page documents partial output" grep -q 'FILE.partial' "$SANDBOX/share/man/man1/fs.bcachefs-read.1"
check "the read man page carries its example" grep -q 'hello.txt' "$SANDBOX/share/man/man1/fs.bcachefs-read.1"

rust-fs-bcachefs generate completions "$SANDBOX/share" >"$SANDBOX/out" 2>"$SANDBOX/err"
check "completion generation succeeds" test $? -eq 0
for completion in zsh/site-functions/_fs.bcachefs bash-completion/completions/fs.bcachefs fish/vendor_completions.d/fs.bcachefs.fish; do
    check "$completion offers read" grep -qw read "$SANDBOX/share/$completion"
done

finish
