#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONTAINER_NAME="biohazardfs-object-store-smoke-$$"
CONFIG_FILE=""
HOST_PORT=""
ACCESS_KEY_ID="bhfsobjectsmoke"
SECRET_ACCESS_KEY="biohazardfs-object-smoke-password"
BUCKET="biohazardfs-smoke"

cleanup() {
  if [ -n "$CONTAINER_NAME" ]; then
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
  fi
  if [ -n "$CONFIG_FILE" ]; then
    rm -f "$CONFIG_FILE"
  fi
  rm -f /tmp/biohazardfs-object-store-missing.json \
    /tmp/biohazardfs-object-store-ensure.json \
    /tmp/biohazardfs-object-store-check.json
}
trap cleanup EXIT

cd "$ROOT_DIR"

cargo build -p biohazardfs-server --all-features

docker run --rm -d \
  --name "$CONTAINER_NAME" \
  -e RUSTFS_ADDRESS=:9000 \
  -e RUSTFS_CONSOLE_ENABLE=true \
  -e RUSTFS_ACCESS_KEY="$ACCESS_KEY_ID" \
  -e RUSTFS_SECRET_KEY="$SECRET_ACCESS_KEY" \
  -p 127.0.0.1::9000 \
  rustfs/rustfs:1.0.0-beta.8 \
  --address :9000 --console-enable /data >/dev/null

HOST_PORT="$(docker port "$CONTAINER_NAME" 9000/tcp | awk -F: '{print $NF}')"
if [ -z "$HOST_PORT" ]; then
  echo "could not discover RustFS host port" >&2
  exit 1
fi

python3 - <<PY
import socket
import sys
import time
host = '127.0.0.1'
port = int('$HOST_PORT')
last = None
for _ in range(120):
    try:
        with socket.create_connection((host, port), timeout=0.5):
            sys.exit(0)
    except OSError as exc:
        last = exc
        time.sleep(0.5)
raise SystemExit(f'RustFS did not accept TCP connections: {last}')
PY

CONFIG_FILE="$(mktemp)"
cat >"$CONFIG_FILE" <<EOF_CONFIG
schema_version = "2026-07-config-v1"
profile = "ci"

[profiles.ci.object_store]
provider = "rustfs"
endpoint = "http://127.0.0.1:$HOST_PORT"
bucket = "$BUCKET"
region = "us-east-1"
access_key_id = "$ACCESS_KEY_ID"
secret_access_key = "$SECRET_ACCESS_KEY"
EOF_CONFIG

initial_status=1
object_store_ready=0
for _ in $(seq 1 60); do
  set +e
  target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci object-store check \
    >/tmp/biohazardfs-object-store-missing.json
  initial_status=$?
  set -e
  if python3 - <<PY
import json
from pathlib import Path
text = Path('/tmp/biohazardfs-object-store-missing.json').read_text()
payload = json.loads(text)
if payload.get('operation') != 'server.object_store':
    raise SystemExit(1)
if $initial_status == 0 and payload.get('ok') is True and payload.get('data', {}).get('status') == 'bucket_available':
    raise SystemExit(0)
if payload.get('ok') is False and payload.get('error', {}).get('code') == 'object_store_bucket_missing':
    raise SystemExit(0)
raise SystemExit(1)
PY
  then
    object_store_ready=1
    break
  fi
  sleep 0.5
done
if [ "$object_store_ready" -ne 1 ]; then
  echo "RustFS object-store API did not become ready" >&2
  cat /tmp/biohazardfs-object-store-missing.json >&2 || true
  exit 1
fi

python3 - <<PY
import json
from pathlib import Path
text = Path('/tmp/biohazardfs-object-store-missing.json').read_text()
payload = json.loads(text)
assert payload['operation'] == 'server.object_store', payload
if $initial_status == 0:
    assert payload['ok'] is True, payload
    assert payload['data']['status'] == 'bucket_available', payload
else:
    assert payload['ok'] is False, payload
    assert payload['error']['code'] == 'object_store_bucket_missing', payload
assert '$SECRET_ACCESS_KEY' not in text, text
assert '$ACCESS_KEY_ID' not in text, text
PY

target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci object-store ensure-bucket \
  >/tmp/biohazardfs-object-store-ensure.json

target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci object-store check \
  >/tmp/biohazardfs-object-store-check.json

python3 - <<PY
import json
from pathlib import Path
ensure_text = Path('/tmp/biohazardfs-object-store-ensure.json').read_text()
check_text = Path('/tmp/biohazardfs-object-store-check.json').read_text()
ensure = json.loads(ensure_text)
check = json.loads(check_text)
for payload in (ensure, check):
    assert payload['ok'] is True, payload
    assert payload['operation'] == 'server.object_store', payload
    assert payload['data']['provider'] == 'rustfs', payload
    assert payload['data']['bucket'] == '$BUCKET', payload
    assert payload['data']['credentials_configured'] is True, payload
    assert payload['data']['status'] == 'bucket_available', payload
for text in (ensure_text, check_text):
    assert '$SECRET_ACCESS_KEY' not in text, text
    assert '$ACCESS_KEY_ID' not in text, text
print('object-store-smoke-ok')
PY
