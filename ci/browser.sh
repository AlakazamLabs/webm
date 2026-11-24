#!/usr/bin/env bash
# Browser playback check (macOS or Linux with browser libraries): installs
# Playwright's Chromium and Firefox (cached in $K2_JOB_CACHE) and plays
# every file in ci/browser/files. The files come from ci/oracles.sh's
# out/media.
set -euo pipefail
cd ci/browser
export PLAYWRIGHT_BROWSERS_PATH="${K2_JOB_CACHE:-$PWD/.cache}/pw-browsers"
npm install --no-audit --no-fund --silent
npx playwright install chromium firefox
ls files | wc -l | xargs echo "files:"
node play.mjs
