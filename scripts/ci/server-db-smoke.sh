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
assert first['data']['current_version'] == '002', first
assert len(first['data']['applied_migrations']) == 2, first
assert second['data']['status'] == 'up_to_date', second
assert len(second['data']['already_applied_migrations']) == 2, second
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

SMOKE_TOKEN="biohazardfs-server-db-smoke-token"
TOKEN_HASH="$(python3 - <<PY
import hashlib
print('sha256:' + hashlib.sha256('$SMOKE_TOKEN'.encode()).hexdigest())
PY
)"

docker exec -i "$CONTAINER_ID" psql -v token_hash="$TOKEN_HASH" -U biohazardfs -d biohazardfs >/dev/null <<'SQL'
INSERT INTO organizations (org_id, slug, display_name, status)
VALUES ('org_smoke', 'smoke', 'Smoke Org', 'active'),
       ('org_hidden', 'hidden', 'Hidden Org', 'active');

INSERT INTO users (org_id, user_id, display_name, email, status)
VALUES ('org_smoke', 'user_smoke', 'Smoke User', 'smoke@example.invalid', 'active'),
       ('org_hidden', 'user_hidden', 'Hidden User', 'hidden@example.invalid', 'active');

INSERT INTO tokens (org_id, token_id, user_id, kind, scopes, status, secret_hash)
VALUES ('org_smoke', 'token_smoke', 'user_smoke', 'api', '["namespace:read"]'::jsonb, 'active', :'token_hash');

INSERT INTO nodes (org_id, node_id, parent_node_id, name, kind, created_by, path_cache)
VALUES ('org_smoke', 'node_root_dir', NULL, 'Shots', 'directory', 'user_smoke', '/Shots'),
       ('org_smoke', 'node_deleted', NULL, 'Deleted', 'directory', 'user_smoke', '/Deleted'),
       ('org_smoke', 'node_child_file', 'node_root_dir', 'plate.exr', 'file', 'user_smoke', '/Shots/plate.exr'),
       ('org_hidden', 'node_hidden', NULL, 'Hidden', 'directory', 'user_hidden', '/Hidden');

UPDATE nodes
SET deleted_at = now(), deleted_by = 'user_smoke'
WHERE org_id = 'org_smoke' AND node_id = 'node_deleted';
SQL

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
import urllib.error
import urllib.request
base = "http://$SERVER_ENDPOINT"

with urllib.request.urlopen(base + "/readyz", timeout=2) as response:
    payload = json.loads(response.read().decode())
assert response.status == 200, payload
assert payload['ok'] is True, payload
assert payload['operation'] == 'server.ready', payload
assert payload['data']['state'] == 'ready', payload
checks = {check['name']: check for check in payload['data']['checks']}
assert checks['database']['ok'] is True, payload

try:
    urllib.request.urlopen(base + "/api/v1/namespace/children", timeout=2)
    raise AssertionError('namespace request without auth unexpectedly succeeded')
except urllib.error.HTTPError as error:
    text = error.read().decode()
    payload = json.loads(text)
    assert error.code == 401, payload
    assert payload['ok'] is False, payload
    assert payload['operation'] == 'server.namespace.children', payload
    assert payload['error']['code'] == 'auth_required', payload

bad_request = urllib.request.Request(
    base + "/api/v1/namespace/children",
    headers={"Authorization": "Bearer do-not-echo-bad-token"},
)
try:
    urllib.request.urlopen(bad_request, timeout=2)
    raise AssertionError('namespace request with bad auth unexpectedly succeeded')
except urllib.error.HTTPError as error:
    text = error.read().decode()
    payload = json.loads(text)
    assert error.code == 401, payload
    assert payload['error']['code'] == 'auth_invalid', payload
    assert 'do-not-echo-bad-token' not in text, text

request = urllib.request.Request(
    base + "/api/v1/namespace/children",
    headers={"Authorization": "Bearer $SMOKE_TOKEN"},
)
with urllib.request.urlopen(request, timeout=2) as response:
    text = response.read().decode()
    payload = json.loads(text)
assert response.status == 200, payload
assert payload['ok'] is True, payload
assert payload['operation'] == 'server.namespace.children', payload
assert payload['data']['parent_node_id'] is None, payload
assert [node['node_id'] for node in payload['data']['nodes']] == ['node_root_dir'], payload
assert payload['data']['nodes'][0]['name'] == 'Shots', payload
assert 'node_deleted' not in text, payload
assert 'node_hidden' not in text, payload
assert '$SMOKE_TOKEN' not in text, text

child_request = urllib.request.Request(
    base + "/api/v1/namespace/children?parent=node_root_dir&limit=10",
    headers={"Authorization": "Bearer $SMOKE_TOKEN"},
)
with urllib.request.urlopen(child_request, timeout=2) as response:
    child_payload = json.loads(response.read().decode())
assert child_payload['data']['parent_node_id'] == 'node_root_dir', child_payload
assert [node['node_id'] for node in child_payload['data']['nodes']] == ['node_child_file'], child_payload
print('server-db-smoke-ok')
PY
