#!/bin/sh
# Fake tasks command: fails with a stderr line.
echo "reading notes" >&2
echo "claude-tasks: cannot read resume pointers" >&2
exit 1
