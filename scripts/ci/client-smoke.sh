#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ENDPOINT="${BIOHAZARDFS_DAEMON_ENDPOINT:-127.0.0.1:47666}"
LOCAL_TOKEN="${BIOHAZARDFS_LOCAL_TOKEN:-local_scaffold_smoke_token}"
DAEMON_LOG="${TMPDIR:-/tmp}/biohazardfsd-smoke.log"
WORKSPACE_ROOT="$(mktemp -d)"

cd "$ROOT_DIR"

cargo build --workspace --all-features

mkdir -p "$WORKSPACE_ROOT/plates"
printf 'workspace smoke\n' >"$WORKSPACE_ROOT/plates/shot001.txt"

BIOHAZARDFS_LOCAL_TOKEN="$LOCAL_TOKEN" BIOHAZARDFS_WORKSPACE_ROOT="$WORKSPACE_ROOT" target/debug/biohazardfsd --dev-loopback-http --addr "$ENDPOINT" >"$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!
cleanup() {
  kill "$DAEMON_PID" >/dev/null 2>&1 || true
  rm -rf "$WORKSPACE_ROOT"
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

BIOHAZARDFS_LOCAL_TOKEN="$LOCAL_TOKEN" target/debug/biohazardfs --daemon-endpoint "$ENDPOINT" daemon workspace-status >/tmp/biohazardfs-workspace-status.json
BIOHAZARDFS_LOCAL_TOKEN="$LOCAL_TOKEN" target/debug/biohazardfs --daemon-endpoint "$ENDPOINT" daemon workspace-list --path plates >/tmp/biohazardfs-workspace-list.json
python3 - <<'PY'
import json
from pathlib import Path
status = json.loads(Path('/tmp/biohazardfs-workspace-status.json').read_text())
listing = json.loads(Path('/tmp/biohazardfs-workspace-list.json').read_text())
assert status['ok'] is True, status
assert status['command'] == 'daemon.workspace.status', status
assert status['data']['root_configured'] is True, status
assert status['data']['root_exists'] is True, status
assert listing['ok'] is True, listing
assert listing['command'] == 'daemon.workspace.list', listing
names = [entry['name'] for entry in listing['data']['entries']]
assert 'shot001.txt' in names, listing
print('daemon-workspace-smoke-ok')
PY

CONFIG_SMOKE_DIR="$(mktemp -d)"
cat >"$CONFIG_SMOKE_DIR/config.toml" <<'TOML'
schema_version = "2026-07-config-v1"
profile = "smoke"

[profiles.smoke.server]
bind = "127.0.0.1:49999"

[profiles.smoke.object_store]
provider = "rustfs"
endpoint = "http://object-store:9000"
bucket = "biohazardfs-smoke"
access_key_id = "biohazardfs"
secret_access_key = "do-not-print"
TOML

target/debug/biohazardfs --config "$CONFIG_SMOKE_DIR/config.toml" config path >/tmp/biohazardfs-config-path.json
target/debug/biohazardfs --config "$CONFIG_SMOKE_DIR/config.toml" config show --redacted >/tmp/biohazardfs-config-show.json
target/debug/biohazardfs --config "$CONFIG_SMOKE_DIR/config.toml" config validate >/tmp/biohazardfs-config-validate.json
python3 - <<'PY'
import json
from pathlib import Path
for path in ['/tmp/biohazardfs-config-path.json', '/tmp/biohazardfs-config-show.json', '/tmp/biohazardfs-config-validate.json']:
    text = Path(path).read_text()
    payload = json.loads(text)
    assert payload['ok'] is True, payload
    assert 'do-not-print' not in text, text
show = json.loads(Path('/tmp/biohazardfs-config-show.json').read_text())
assert show['command'] == 'config.show', show
assert show['data']['config']['server']['bind'] == '127.0.0.1:49999', show
assert show['data']['config']['object_store']['secret_access_key'] == '***REDACTED***', show
print('config-cli-smoke-ok')
PY

if [[ "${BIOHAZARDFS_SKIP_ELECTRON_BUILD:-0}" != "1" ]]; then
  pnpm --dir apps/workspace-electron install --frozen-lockfile
  pnpm --dir apps/workspace-electron run build
fi

if [[ "${BIOHAZARDFS_SKIP_ELECTRON_LAUNCH:-0}" == "1" ]]; then
  echo "BIOHAZARDFS_SKIP_ELECTRON_LAUNCH=1; skipping Electron launch smoke" >&2
  exit 0
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
