#!/bin/sh
# Fake tasks command: prints the claude-tasks contract's example list.
[ "$1 $2" = "list --json" ] || { echo "expected: list --json" >&2; exit 2; }
exec cat "$(dirname "$0")/list-example.json"
