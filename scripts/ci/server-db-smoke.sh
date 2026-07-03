#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONTAINER_NAME="biohazardfs-db-smoke-$$"
POSTGRES_PASSWORD="biohazardfs-db-smoke-password"
SERVER_ENDPOINT="127.0.0.1:48081"
SERVER_LOG="${TMPDIR:-/tmp}/biohazardfs-server-db-smoke.log"
SERVER_PID=""
CONFIG_FILE=""

cd "$ROOT_DIR"

cargo build -p biohazardfs-server --all-features

CONTAINER_ID="$(docker run --rm -d \
  --name "$CONTAINER_NAME" \
  -e POSTGRES_DB=biohazardfs \
  -e POSTGRES_USER=biohazardfs \
  -e POSTGRES_PASSWORD="$POSTGRES_PASSWORD" \
  -p 127.0.0.1::5432/tcp \
  postgres:17-alpine)"

cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$CONFIG_FILE" ]]; then
    rm -f "$CONFIG_FILE"
  fi
  docker rm -f "$CONTAINER_ID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 60); do
  if docker exec "$CONTAINER_ID" pg_isready -U biohazardfs -d biohazardfs >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

HOST_PORT="$(docker port "$CONTAINER_ID" 5432/tcp | awk -F: '{print $NF}' | tail -n 1)"
if [[ -z "$HOST_PORT" ]]; then
  echo "could not determine Postgres smoke host port" >&2
  exit 1
fi

for _ in $(seq 1 60); do
  if python3 - <<PY
import socket
try:
    with socket.create_connection(('127.0.0.1', int('$HOST_PORT')), timeout=1):
        raise SystemExit(0)
except OSError:
    raise SystemExit(1)
PY
  then
    break
  fi
  sleep 1
done

DATABASE_URL="postgres://biohazardfs:${POSTGRES_PASSWORD}@127.0.0.1:${HOST_PORT}/biohazardfs?sslmode=disable"
CONFIG_FILE="$(mktemp "${TMPDIR:-/tmp}/biohazardfs-server-db-smoke.XXXXXX.toml")"
cat >"$CONFIG_FILE" <<EOF_CONFIG
schema_version = "2026-07-config-v1"
profile = "ci"

[profiles.ci.database]
url = "$DATABASE_URL"
EOF_CONFIG

run_migrate_with_retries() {
  local output_path="$1"
  shift
  local attempt
  for attempt in $(seq 1 10); do
    if "$@" >"$output_path"; then
      return 0
    fi
    if [[ "$attempt" == "10" ]]; then
      echo "biohazardfs-server migrate failed after ${attempt} attempts" >&2
      python3 - <<PY
from pathlib import Path
text = Path('$output_path').read_text() if Path('$output_path').exists() else ''
text = text.replace('$POSTGRES_PASSWORD', '[redacted]')
print(text, file=__import__('sys').stderr)
PY
      return 1
    fi
    sleep 1
  done
}

run_migrate_with_retries \
  /tmp/biohazardfs-server-db-migrate-1.json \
  env -u BIOHAZARDFS_DATABASE_URL target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci migrate

BIOHAZARDFS_DATABASE_URL="$DATABASE_URL" run_migrate_with_retries \
  /tmp/biohazardfs-server-db-migrate-2.json \
  target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci migrate

env -u BIOHAZARDFS_DATABASE_URL target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci config \
  >/tmp/biohazardfs-server-db-config.json

python3 - <<'PY'
import json
from pathlib import Path
first_text = Path('/tmp/biohazardfs-server-db-migrate-1.json').read_text()
second_text = Path('/tmp/biohazardfs-server-db-migrate-2.json').read_text()
config_text = Path('/tmp/biohazardfs-server-db-config.json').read_text()
first = json.loads(first_text)
second = json.loads(second_text)
config = json.loads(config_text)
assert first['ok'] is True, first
assert second['ok'] is True, second
assert first['operation'] == 'server.migrate', first
assert second['operation'] == 'server.migrate', second
assert first['data']['status'] == 'applied', first
assert first['data']['current_version'] == '001', first
assert len(first['data']['applied_migrations']) == 1, first
assert second['data']['status'] == 'up_to_date', second
assert len(second['data']['already_applied_migrations']) == 1, second
assert config['ok'] is True, config
assert config['operation'] == 'server.config', config
assert config['data']['database']['url_set'] is True, config
for text in (first_text, second_text, config_text):
    assert 'biohazardfs-db-smoke-password' not in text, text
    assert 'postgres://' not in text, text
PY

TABLES="$(docker exec "$CONTAINER_ID" psql -U biohazardfs -d biohazardfs -Atc "
  SELECT string_agg(table_name, ',' ORDER BY table_name)
  FROM information_schema.tables
  WHERE table_schema = 'public'
    AND table_name IN (
      'schema_migrations',
      'organizations',
      'users',
      'tokens',
      'nodes',
      'content_manifests',
      'file_versions',
      'operations',
      'upload_sessions',
      'audit_events'
    );
")"

python3 - <<PY
actual = set("""$TABLES""".split(',')) if """$TABLES""" else set()
expected = {
    'schema_migrations',
    'organizations',
    'users',
    'tokens',
    'nodes',
    'content_manifests',
    'file_versions',
    'operations',
    'upload_sessions',
    'audit_events',
}
missing = expected - actual
assert not missing, f"missing migration tables: {sorted(missing)}"
PY

env -u BIOHAZARDFS_DATABASE_URL target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci serve --addr "$SERVER_ENDPOINT" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 50); do
  if python3 - <<PY
import json
import urllib.request
endpoint = "http://$SERVER_ENDPOINT/readyz"
try:
    with urllib.request.urlopen(endpoint, timeout=1) as response:
        payload = json.loads(response.read().decode())
    raise SystemExit(0 if response.status == 200 and payload.get('operation') == 'server.ready' else 1)
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
endpoint = "http://$SERVER_ENDPOINT/readyz"
with urllib.request.urlopen(endpoint, timeout=2) as response:
    payload = json.loads(response.read().decode())
assert response.status == 200, payload
assert payload['ok'] is True, payload
assert payload['operation'] == 'server.ready', payload
assert payload['data']['state'] == 'ready', payload
checks = {check['name']: check for check in payload['data']['checks']}
assert checks['database']['ok'] is True, payload
print('server-db-smoke-ok')
PY
