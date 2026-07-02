#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ENDPOINT="${BIOHAZARDFS_DAEMON_ENDPOINT:-127.0.0.1:47666}"
LOCAL_TOKEN="${BIOHAZARDFS_LOCAL_TOKEN:-local_scaffold_smoke_token}"
DAEMON_LOG="${TMPDIR:-/tmp}/biohazardfsd-smoke.log"

cd "$ROOT_DIR"

cargo build --workspace --all-features

BIOHAZARDFS_LOCAL_TOKEN="$LOCAL_TOKEN" target/debug/biohazardfsd --dev-loopback-http --addr "$ENDPOINT" >"$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!
cleanup() {
  kill "$DAEMON_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 50); do
  if BIOHAZARDFS_LOCAL_TOKEN="$LOCAL_TOKEN" target/debug/biohazardfs --daemon-endpoint "$ENDPOINT" daemon status >/tmp/biohazardfs-daemon-status.json 2>/tmp/biohazardfs-daemon-status.err; then
    if python3 - <<'PY'
import json
from pathlib import Path
status = json.loads(Path('/tmp/biohazardfs-daemon-status.json').read_text())
raise SystemExit(0 if status.get('ok') is True else 1)
PY
    then
      break
    fi
  fi
  sleep 0.1
done

python3 - <<'PY'
import json
from pathlib import Path
status = json.loads(Path('/tmp/biohazardfs-daemon-status.json').read_text())
assert status['ok'] is True, status
assert status['data']['state'] == 'ready', status
print('daemon-cli-smoke-ok')
PY

if [[ "${BIOHAZARDFS_SKIP_ELECTRON_BUILD:-0}" != "1" ]]; then
  pnpm --dir apps/workspace-electron install --frozen-lockfile
  pnpm --dir apps/workspace-electron run build
fi

if command -v xvfb-run >/dev/null 2>&1; then
  # GitHub-hosted Linux runners do not configure Electron's chrome-sandbox helper.
  # Keep renderer sandbox enabled in app code, but disable Chromium's host sandbox for this smoke launch.
  BIOHAZARDFS_DESKTOP_SMOKE=1 \
  BIOHAZARDFS_DAEMON_ENDPOINT="$ENDPOINT" \
  BIOHAZARDFS_LOCAL_TOKEN="$LOCAL_TOKEN" \
  xvfb-run -a pnpm --dir apps/workspace-electron exec electron --no-sandbox dist/electron/main.js
else
  echo "xvfb-run not available; skipping Electron launch smoke" >&2
fi
