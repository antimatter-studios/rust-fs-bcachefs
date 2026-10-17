#!/usr/bin/env bash
# Shared write help, manual pages and completions come from the parser.
source "$(dirname "$0")/lib.sh"

for verb in write put; do
    fs.bcachefs image.img "$verb" --help >"$SANDBOX/help" 2>"$SANDBOX/err"
    check "$verb --help succeeds without an image" test $? -eq 0
    for option in from mode journal; do
        check "$verb documents --$option" grep -q -- "--$option" "$SANDBOX/help"
    done
    check "$verb has an input-file example" grep -q 'fs.bcachefs disk.img write /hello.txt --from hello.txt' "$SANDBOX/help"
done
rust-fs-bcachefs generate man "$SANDBOX/share" >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'man page generation succeeds' test $? -eq 0
check 'the write man page exists' test -s "$SANDBOX/share/man/man1/fs.bcachefs-write.1"
check 'the write man page documents stdin' grep -q stdin "$SANDBOX/share/man/man1/fs.bcachefs-write.1"
rust-fs-bcachefs generate completions "$SANDBOX/share" >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'completion generation succeeds' test $? -eq 0
for completion in zsh/site-functions/_fs.bcachefs bash-completion/completions/fs.bcachefs fish/vendor_completions.d/fs.bcachefs.fish; do
    check "$completion offers write" grep -qw write "$SANDBOX/share/$completion"
    check "$completion retains put" grep -qw put "$SANDBOX/share/$completion"
done
finish
