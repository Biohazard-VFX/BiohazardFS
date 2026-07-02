#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ENDPOINT="${BIOHAZARDFS_SERVER_SMOKE_ENDPOINT:-127.0.0.1:48080}"
SERVER_LOG="${TMPDIR:-/tmp}/biohazardfs-server-smoke.log"

cd "$ROOT_DIR"

cargo build -p biohazardfs-server --all-features

target/debug/biohazardfs-server serve --addr "$ENDPOINT" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
cleanup() {
  kill "$SERVER_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 50); do
  if python3 - <<PY
import json
import urllib.request
endpoint = "http://$ENDPOINT/healthz"
try:
    with urllib.request.urlopen(endpoint, timeout=1) as response:
        payload = json.loads(response.read().decode())
    raise SystemExit(0 if payload.get('ok') is True else 1)
except Exception:
    raise SystemExit(1)
PY
  then
    break
  fi
  sleep 0.1
done

python3 - <<PY
import json
import urllib.request
base = "http://$ENDPOINT"
expectations = {
    "/healthz": "server.health",
    "/readyz": "server.ready",
    "/version": "server.version",
    "/api/v1/status": "server.status",
}
for path, operation in expectations.items():
    with urllib.request.urlopen(base + path, timeout=2) as response:
        payload = json.loads(response.read().decode())
    assert payload["ok"] is True, payload
    assert payload["operation"] == operation, payload
    assert payload["meta"]["schema_version"] == "2026-07-server-v1", payload
    assert payload["meta"]["api_version"] == "v1", payload
print("server-smoke-ok")
PY

target/debug/biohazardfs-server health >/tmp/biohazardfs-server-health.json
python3 - <<'PY'
import json
from pathlib import Path
payload = json.loads(Path('/tmp/biohazardfs-server-health.json').read_text())
assert payload['ok'] is True, payload
assert payload['operation'] == 'server.health', payload
PY

target/debug/biohazardfs-server version >/tmp/biohazardfs-server-version.json
python3 - <<'PY'
import json
from pathlib import Path
payload = json.loads(Path('/tmp/biohazardfs-server-version.json').read_text())
assert payload['ok'] is True, payload
assert payload['operation'] == 'server.version', payload
PY

BIOHAZARDFS_OBJECT_STORE_SECRET_ACCESS_KEY=do-not-print target/debug/biohazardfs-server config >/tmp/biohazardfs-server-config.json
python3 - <<'PY'
import json
from pathlib import Path
text = Path('/tmp/biohazardfs-server-config.json').read_text()
payload = json.loads(text)
assert payload['ok'] is True, payload
assert payload['operation'] == 'server.config', payload
assert payload['data']['schema_version'] == '2026-07-config-v1', payload
assert payload['data']['object_store']['provider'] == 'rustfs', payload
assert 'do-not-print' not in text, payload
PY

target/debug/biohazardfs-server migrate >/tmp/biohazardfs-server-migrate.json
python3 - <<'PY'
import json
from pathlib import Path
payload = json.loads(Path('/tmp/biohazardfs-server-migrate.json').read_text())
assert payload['ok'] is True, payload
assert payload['operation'] == 'server.migrate', payload
PY

target/debug/biohazardfs-server worker >/tmp/biohazardfs-server-worker.json
python3 - <<'PY'
import json
from pathlib import Path
payload = json.loads(Path('/tmp/biohazardfs-server-worker.json').read_text())
assert payload['ok'] is True, payload
assert payload['operation'] == 'server.worker', payload
PY
