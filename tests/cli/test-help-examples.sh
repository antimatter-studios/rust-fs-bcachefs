#!/usr/bin/env bash
# Every subcommand's help, and the man page generated from it, carries an
# example of that subcommand. The verbs come from the parser's own table, so
# a verb added later is held to the same rule without a new row here.
source "$(dirname "$0")/lib.sh"

fs.bcachefs --help >"$SANDBOX/help" 2>"$SANDBOX/err"
check 'the command help is available' test $? -eq 0
awk '/^Commands:/{commands=1; next} /^Options:/{commands=0} commands && /^  [a-z]/{print $1}' \
    "$SANDBOX/help" | grep -vx help >"$SANDBOX/verbs"
check 'the verb table is not empty' test -s "$SANDBOX/verbs"

rust-fs-bcachefs generate man "$SANDBOX/share" >"$SANDBOX/out" 2>"$SANDBOX/err"
check 'man page generation succeeds' test $? -eq 0

while read -r verb; do
    fs.bcachefs disk.img "$verb" --help >"$SANDBOX/verb-help" 2>"$SANDBOX/err"
    check "$verb --help succeeds without an image" test $? -eq 0
    check "$verb help has an example section" grep -Eq '^Examples?:$' "$SANDBOX/verb-help"
    check "$verb help has an example of $verb" grep -q "fs.bcachefs disk.img $verb " "$SANDBOX/verb-help"
    check "$verb man page carries its example" \
        grep -q "fs.bcachefs disk.img $verb " "$SANDBOX/share/man/man1/fs.bcachefs-$verb.1"
done <"$SANDBOX/verbs"

finish
