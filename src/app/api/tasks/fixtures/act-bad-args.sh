#!/bin/sh
# Fake tasks command for an action: writes its arguments, one per line, to `args` in the
# folder of its first --file=, prints the reader's reply line and exits 2.
for arg; do
    case $arg in
        --file=*) dir=$(dirname "${arg#--file=}"); break ;;
    esac
done
[ -n "$dir" ] && printf '%s\n' "$@" > "$dir/args"
echo '{"ok": false, "reason": "bad arguments: one --row for each --file"}'
exit 2
