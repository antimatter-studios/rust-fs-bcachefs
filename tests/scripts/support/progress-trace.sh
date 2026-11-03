# Read through BASH_ENV by a supervisor under guard, before its first command.
# It writes each command the supervisor runs to descriptor
# $SUPERVISOR_PROGRESS_FD and nowhere else, so a guard can follow its progress
# while every native capture keeps its exact bytes.
#
# Which way depends on the shell, and neither way does for both:
# - Bash 4.1 and newer has BASH_XTRACEFD. The DEBUG trap would not do there:
#   Bash 5 does not run it inside a trap handler, so cleanup would look stuck.
# - Bash 3.2, the macOS system shell, has no BASH_XTRACEFD. Its xtrace follows
#   stderr, and so enters `sample >capture 2>&1` when sample is a function. Its
#   DEBUG trap does run inside a trap handler, and writes where it is told.
unset BASH_ENV
if [ "${BASH_VERSINFO[0]}" -gt 4 ] || { [ "${BASH_VERSINFO[0]}" -eq 4 ] && [ "${BASH_VERSINFO[1]}" -ge 1 ]; }; then
    BASH_XTRACEFD="$SUPERVISOR_PROGRESS_FD"
    set -x
else
    set -T
    trap 'printf "+ %s\n" "$BASH_COMMAND" >&"$SUPERVISOR_PROGRESS_FD"' DEBUG
fi
