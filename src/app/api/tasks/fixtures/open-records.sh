#!/bin/sh
# Fake editor for tasks.open: writes the folder it runs in, then its arguments one per line,
# to `opened` in that folder.
{ pwd -P; printf '%s\n' "$@"; } > opened.part && mv opened.part opened
