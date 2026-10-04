#!/bin/sh
# Fake tasks command: never answers in time; its child must be killed too.
sleep 30 &
wait
