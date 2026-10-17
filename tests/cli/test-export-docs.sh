#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

fs.bcachefs image.img export --help >"$SANDBOX/help" 2>"$SANDBOX/err"
check 'export help succeeds without opening an image' test $? -eq 0
check 'help documents replacement' grep -q -- '--replace' "$SANDBOX/help"
check 'help has an export example' grep -q 'fs.bcachefs disk.img export /hello.txt hello.txt' "$SANDBOX/help"
check 'help documents SHA-256' grep -q 'SHA-256' "$SANDBOX/help"
check 'help explains non-file refusal' grep -qi 'symlinks' "$SANDBOX/help"
rust-fs-bcachefs generate man "$SANDBOX/share" >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'man generation succeeds' test $? -eq 0
check 'export man page exists' test -s "$SANDBOX/share/man/man1/fs.bcachefs-export.1"
rust-fs-bcachefs generate completions "$SANDBOX/share" >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'completion generation succeeds' test $? -eq 0
for completion in zsh/site-functions/_fs.bcachefs bash-completion/completions/fs.bcachefs fish/vendor_completions.d/fs.bcachefs.fish; do
    check 'completion offers export' grep -qw export "$SANDBOX/share/$completion"
done
finish
