#!/usr/bin/env bash
set -euo pipefail
# This runs inside a fresh CI D-Bus session. Never unlock a personal keyring.
export XDG_CONFIG_HOME="$RUNNER_TEMP/aidash-wdio-config"
export XDG_DATA_HOME="$RUNNER_TEMP/aidash-wdio-data"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"
printf 'fixture-keyring-password' | gnome-keyring-daemon --unlock --components=secrets
npm run test:e2e --prefix desktop
