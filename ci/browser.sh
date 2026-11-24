#!/usr/bin/env bash
# Browser playback check (macOS or Linux with browser libraries): installs
# Playwright's Chromium and Firefox (cached in $WEBM_CI_CACHE, default
# .ci-cache in the repo) and plays
# every file in ci/browser/files. The files come from ci/oracles.sh's
# out/media.
set -euo pipefail
cache="${WEBM_CI_CACHE:-$PWD/.ci-cache}"
cd ci/browser
export PLAYWRIGHT_BROWSERS_PATH="$cache/pw-browsers"
npm install --no-audit --no-fund --silent
list="${BROWSERS:-chromium,firefox}"
# shellcheck disable=SC2046
npx playwright install $(echo "$list" | tr , ' ')
ls files | wc -l | xargs echo "files:"
node play.mjs
