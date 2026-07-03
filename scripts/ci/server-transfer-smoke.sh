#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
POSTGRES_CONTAINER="biohazardfs-transfer-postgres-$$"
RUSTFS_CONTAINER="biohazardfs-transfer-rustfs-$$"
POSTGRES_PASSWORD="biohazardfs-transfer-db-password"
RUSTFS_ACCESS_KEY="bhfstransfersmoke"
RUSTFS_SECRET_KEY="biohazardfs-transfer-object-password"
BUCKET="biohazardfs-transfer-smoke"
SERVER_ENDPOINT="127.0.0.1:48082"
SERVER_LOG="${TMPDIR:-/tmp}/biohazardfs-transfer-smoke.log"
SERVER_PID=""
CONFIG_FILE=""

cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$CONFIG_FILE" ]]; then
    rm -f "$CONFIG_FILE"
  fi
  docker rm -f "$POSTGRES_CONTAINER" >/dev/null 2>&1 || true
  docker rm -f "$RUSTFS_CONTAINER" >/dev/null 2>&1 || true
  rm -f /tmp/biohazardfs-transfer-put.json /tmp/biohazardfs-transfer-get.json \
    /tmp/biohazardfs-transfer-cli-put.json /tmp/biohazardfs-transfer-cli-get.json \
    /tmp/biohazardfs-transfer-cli-input.txt /tmp/biohazardfs-transfer-cli-output.txt \
    /tmp/biohazardfs-transfer-file-put.json /tmp/biohazardfs-transfer-file-get.json \
    /tmp/biohazardfs-transfer-file-output.txt /tmp/biohazardfs-transfer-cli-file-put.json \
    /tmp/biohazardfs-transfer-cli-file-get.json /tmp/biohazardfs-transfer-cli-file-input.txt \
    /tmp/biohazardfs-transfer-cli-file-output.txt
}
trap cleanup EXIT

cd "$ROOT_DIR"

cargo build -p biohazardfs-server -p biohazardfs-cli --all-features

POSTGRES_ID="$(docker run --rm -d \
  --name "$POSTGRES_CONTAINER" \
  -e POSTGRES_DB=biohazardfs \
  -e POSTGRES_USER=biohazardfs \
  -e POSTGRES_PASSWORD="$POSTGRES_PASSWORD" \
  -p 127.0.0.1::5432/tcp \
  postgres:17-alpine)"

docker run --rm -d \
  --name "$RUSTFS_CONTAINER" \
  -e RUSTFS_ADDRESS=:9000 \
  -e RUSTFS_CONSOLE_ENABLE=true \
  -e RUSTFS_ACCESS_KEY="$RUSTFS_ACCESS_KEY" \
  -e RUSTFS_SECRET_KEY="$RUSTFS_SECRET_KEY" \
  -p 127.0.0.1::9000 \
  rustfs/rustfs:1.0.0-beta.8 \
  --address :9000 --console-enable /data >/dev/null

for _ in $(seq 1 60); do
  if docker exec "$POSTGRES_ID" pg_isready -U biohazardfs -d biohazardfs >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

POSTGRES_PORT="$(docker port "$POSTGRES_ID" 5432/tcp | awk -F: '{print $NF}' | tail -n 1)"
RUSTFS_PORT="$(docker port "$RUSTFS_CONTAINER" 9000/tcp | awk -F: '{print $NF}' | tail -n 1)"
if [[ -z "$POSTGRES_PORT" || -z "$RUSTFS_PORT" ]]; then
  echo "could not determine smoke container ports" >&2
  exit 1
fi

python3 - <<PY
import socket
import time
for host, port in [('127.0.0.1', int('$POSTGRES_PORT')), ('127.0.0.1', int('$RUSTFS_PORT'))]:
    last = None
    for _ in range(120):
        try:
            with socket.create_connection((host, port), timeout=0.5):
                break
        except OSError as exc:
            last = exc
            time.sleep(0.5)
    else:
        raise SystemExit(f'{host}:{port} did not accept TCP connections: {last}')
PY

DATABASE_URL="postgres://biohazardfs:${POSTGRES_PASSWORD}@127.0.0.1:${POSTGRES_PORT}/biohazardfs?sslmode=disable"
CONFIG_FILE="$(mktemp "${TMPDIR:-/tmp}/biohazardfs-transfer-smoke.XXXXXX.toml")"
cat >"$CONFIG_FILE" <<EOF_CONFIG
schema_version = "2026-07-config-v1"
profile = "ci"

[profiles.ci.server]
public_url = "http://$SERVER_ENDPOINT"

[profiles.ci.database]
url = "$DATABASE_URL"

[profiles.ci.object_store]
provider = "rustfs"
endpoint = "http://127.0.0.1:$RUSTFS_PORT"
bucket = "$BUCKET"
region = "us-east-1"
access_key_id = "$RUSTFS_ACCESS_KEY"
secret_access_key = "$RUSTFS_SECRET_KEY"
EOF_CONFIG

env -u BIOHAZARDFS_DATABASE_URL target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci migrate >/tmp/biohazardfs-transfer-migrate.json

object_store_ready=0
for _ in $(seq 1 60); do
  if target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci object-store ensure-bucket \
    >/tmp/biohazardfs-transfer-object-ready.json; then
    object_store_ready=1
    break
  fi
  sleep 0.5
done
if [ "$object_store_ready" -ne 1 ]; then
  echo "RustFS object-store API did not become ready for transfer smoke" >&2
  cat /tmp/biohazardfs-transfer-object-ready.json >&2 || true
  exit 1
fi

SMOKE_TOKEN="biohazardfs-transfer-smoke-token"
TOKEN_HASH="$(python3 - <<PY
import hashlib
print('sha256:' + hashlib.sha256('$SMOKE_TOKEN'.encode()).hexdigest())
PY
)"

docker exec -i "$POSTGRES_ID" psql -v token_hash="$TOKEN_HASH" -U biohazardfs -d biohazardfs >/dev/null <<'SQL'
INSERT INTO organizations (org_id, slug, display_name, status)
VALUES ('org_transfer', 'transfer', 'Transfer Org', 'active');

INSERT INTO users (org_id, user_id, display_name, email, status)
VALUES ('org_transfer', 'user_transfer', 'Transfer User', 'transfer@example.invalid', 'active');

INSERT INTO tokens (org_id, token_id, user_id, kind, scopes, status, secret_hash)
VALUES ('org_transfer', 'token_transfer', 'user_transfer', 'api', '["object:read", "object:write", "file:read", "file:write", "namespace:read"]'::jsonb, 'active', :'token_hash');
SQL

env -u BIOHAZARDFS_DATABASE_URL target/debug/biohazardfs-server --config "$CONFIG_FILE" --profile ci serve --addr "$SERVER_ENDPOINT" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 80); do
  if python3 - <<PY
import json
import urllib.request
try:
    with urllib.request.urlopen('http://$SERVER_ENDPOINT/readyz', timeout=1) as response:
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
import hashlib
import json
from pathlib import Path
import urllib.error
import urllib.parse
import urllib.request

base = 'http://$SERVER_ENDPOINT'
token = '$SMOKE_TOKEN'
content = b'BiohazardFS transfer smoke payload\nframe=0001\n'
expected_hash = hashlib.sha256(content).hexdigest()

put_request = urllib.request.Request(
    base + '/api/v1/objects/content',
    data=content,
    method='PUT',
    headers={'Authorization': f'Bearer {token}', 'Content-Type': 'application/octet-stream'},
)
with urllib.request.urlopen(put_request, timeout=5) as response:
    put_text = response.read().decode()
put_payload = json.loads(put_text)
Path('/tmp/biohazardfs-transfer-put.json').write_text(put_text)
assert put_payload['ok'] is True, put_payload
assert put_payload['operation'] == 'server.objects.content.put', put_payload
assert put_payload['data']['content_hash'] == expected_hash, put_payload
assert put_payload['data']['size_bytes'] == len(content), put_payload
assert put_payload['data']['storage_provider'] == 'rustfs', put_payload
assert put_payload['data']['object_key'].endswith('/' + expected_hash), put_payload

get_request = urllib.request.Request(
    base + '/api/v1/objects/content?sha256=' + expected_hash,
    headers={'Authorization': f'Bearer {token}'},
)
with urllib.request.urlopen(get_request, timeout=5) as response:
    get_text = response.read().decode()
get_payload = json.loads(get_text)
Path('/tmp/biohazardfs-transfer-get.json').write_text(get_text)
assert get_payload['ok'] is True, get_payload
assert get_payload['operation'] == 'server.objects.content.get', get_payload
assert get_payload['data']['content_hash'] == expected_hash, get_payload
assert get_payload['data']['size_bytes'] == len(content), get_payload
assert bytes.fromhex(get_payload['data']['content_hex']) == content, get_payload

file_content = b'BiohazardFS metadata file smoke payload\nframe=0003\n'
file_hash = hashlib.sha256(file_content).hexdigest()
file_name = urllib.parse.quote('shot001_plate.txt')
file_put_request = urllib.request.Request(
    base + f'/api/v1/files/content?name={file_name}&source=cli',
    data=file_content,
    method='PUT',
    headers={'Authorization': f'Bearer {token}', 'Content-Type': 'application/octet-stream'},
)
try:
    with urllib.request.urlopen(file_put_request, timeout=5) as response:
        file_put_text = response.read().decode()
except urllib.error.HTTPError as error:
    file_put_text = error.read().decode()
    Path('/tmp/biohazardfs-transfer-file-put.json').write_text(file_put_text)
    raise AssertionError(file_put_text) from error
file_put_payload = json.loads(file_put_text)
Path('/tmp/biohazardfs-transfer-file-put.json').write_text(file_put_text)
assert file_put_payload['ok'] is True, file_put_payload
assert file_put_payload['operation'] == 'server.files.content.put', file_put_payload
assert file_put_payload['data']['name'] == 'shot001_plate.txt', file_put_payload
assert file_put_payload['data']['content_hash'] == file_hash, file_put_payload
assert file_put_payload['data']['size_bytes'] == len(file_content), file_put_payload
node_id = file_put_payload['data']['node_id']

file_get_request = urllib.request.Request(
    base + '/api/v1/files/content?node_id=' + urllib.parse.quote(node_id),
    headers={'Authorization': f'Bearer {token}'},
)
try:
    with urllib.request.urlopen(file_get_request, timeout=5) as response:
        file_get_text = response.read().decode()
except urllib.error.HTTPError as error:
    file_get_text = error.read().decode()
    Path('/tmp/biohazardfs-transfer-file-get.json').write_text(file_get_text)
    raise AssertionError(file_get_text) from error
file_get_payload = json.loads(file_get_text)
Path('/tmp/biohazardfs-transfer-file-get.json').write_text(file_get_text)
assert file_get_payload['ok'] is True, file_get_payload
assert file_get_payload['operation'] == 'server.files.content.get', file_get_payload
assert file_get_payload['data']['node_id'] == node_id, file_get_payload
assert file_get_payload['data']['content_hash'] == file_hash, file_get_payload
assert bytes.fromhex(file_get_payload['data']['content_hex']) == file_content, file_get_payload

for text in (put_text, get_text, file_put_text, file_get_text):
    assert token not in text, text
    assert '$POSTGRES_PASSWORD' not in text, text
    assert '$RUSTFS_ACCESS_KEY' not in text, text
    assert '$RUSTFS_SECRET_KEY' not in text, text

try:
    urllib.request.urlopen(base + '/api/v1/objects/content?sha256=' + expected_hash, timeout=2)
    raise AssertionError('object get without auth unexpectedly succeeded')
except urllib.error.HTTPError as error:
    text = error.read().decode()
    payload = json.loads(text)
    assert error.code == 401, payload
    assert payload['error']['code'] == 'auth_required', payload

PY

cat >/tmp/biohazardfs-transfer-cli-input.txt <<'EOF_CLI_INPUT'
BiohazardFS CLI transfer smoke payload
frame=0002
EOF_CLI_INPUT

BIOHAZARDFS_SERVER_TOKEN="$SMOKE_TOKEN" \
  env -u BIOHAZARDFS_DATABASE_URL -u BIOHAZARDFS_SERVER_PUBLIC_URL \
  target/debug/biohazardfs --config "$CONFIG_FILE" --profile ci object put /tmp/biohazardfs-transfer-cli-input.txt \
  >/tmp/biohazardfs-transfer-cli-put.json

CLI_HASH="$(python3 - <<'PY'
import json
from pathlib import Path
payload = json.loads(Path('/tmp/biohazardfs-transfer-cli-put.json').read_text())
assert payload['ok'] is True, payload
assert payload['command'] == 'object.put', payload
print(payload['data']['content_hash'])
PY
)"

BIOHAZARDFS_SERVER_TOKEN="$SMOKE_TOKEN" \
  env -u BIOHAZARDFS_DATABASE_URL -u BIOHAZARDFS_SERVER_PUBLIC_URL \
  target/debug/biohazardfs --config "$CONFIG_FILE" --profile ci object get --sha256 "$CLI_HASH" --output /tmp/biohazardfs-transfer-cli-output.txt \
  >/tmp/biohazardfs-transfer-cli-get.json

cat >/tmp/biohazardfs-transfer-cli-file-input.txt <<'EOF_CLI_FILE_INPUT'
BiohazardFS CLI file workflow smoke payload
frame=0004
EOF_CLI_FILE_INPUT

BIOHAZARDFS_SERVER_TOKEN="$SMOKE_TOKEN" \
  env -u BIOHAZARDFS_DATABASE_URL -u BIOHAZARDFS_SERVER_PUBLIC_URL \
  target/debug/biohazardfs --config "$CONFIG_FILE" --profile ci file put /tmp/biohazardfs-transfer-cli-file-input.txt --name cli-shot.txt \
  >/tmp/biohazardfs-transfer-cli-file-put.json

CLI_NODE_ID="$(python3 - <<'PY'
import json
from pathlib import Path
payload = json.loads(Path('/tmp/biohazardfs-transfer-cli-file-put.json').read_text())
assert payload['ok'] is True, payload
assert payload['command'] == 'file.put', payload
assert payload['data']['name'] == 'cli-shot.txt', payload
print(payload['data']['node_id'])
PY
)"

BIOHAZARDFS_SERVER_TOKEN="$SMOKE_TOKEN" \
  env -u BIOHAZARDFS_DATABASE_URL -u BIOHAZARDFS_SERVER_PUBLIC_URL \
  target/debug/biohazardfs --config "$CONFIG_FILE" --profile ci file get --node "$CLI_NODE_ID" --output /tmp/biohazardfs-transfer-cli-file-output.txt \
  >/tmp/biohazardfs-transfer-cli-file-get.json

python3 - <<PY
import json
from pathlib import Path
put_text = Path('/tmp/biohazardfs-transfer-cli-put.json').read_text()
get_text = Path('/tmp/biohazardfs-transfer-cli-get.json').read_text()
file_put_text = Path('/tmp/biohazardfs-transfer-cli-file-put.json').read_text()
file_get_text = Path('/tmp/biohazardfs-transfer-cli-file-get.json').read_text()
put_payload = json.loads(put_text)
get_payload = json.loads(get_text)
file_put_payload = json.loads(file_put_text)
file_get_payload = json.loads(file_get_text)
assert put_payload['ok'] is True, put_payload
assert get_payload['ok'] is True, get_payload
assert file_put_payload['ok'] is True, file_put_payload
assert file_get_payload['ok'] is True, file_get_payload
assert put_payload['command'] == 'object.put', put_payload
assert get_payload['command'] == 'object.get', get_payload
assert file_put_payload['command'] == 'file.put', file_put_payload
assert file_get_payload['command'] == 'file.get', file_get_payload
assert put_payload['data']['content_hash'] == '$CLI_HASH', put_payload
assert get_payload['data']['content_hash'] == '$CLI_HASH', get_payload
assert file_get_payload['data']['node_id'] == '$CLI_NODE_ID', file_get_payload
assert 'content_hex' not in get_text, get_text
assert 'content_hex' not in file_get_text, file_get_text
assert Path('/tmp/biohazardfs-transfer-cli-output.txt').read_bytes() == Path('/tmp/biohazardfs-transfer-cli-input.txt').read_bytes()
assert Path('/tmp/biohazardfs-transfer-cli-file-output.txt').read_bytes() == Path('/tmp/biohazardfs-transfer-cli-file-input.txt').read_bytes()
for text in (put_text, get_text, file_put_text, file_get_text):
    assert '$SMOKE_TOKEN' not in text, text
    assert '$POSTGRES_PASSWORD' not in text, text
    assert '$RUSTFS_ACCESS_KEY' not in text, text
    assert '$RUSTFS_SECRET_KEY' not in text, text
print('server-transfer-smoke-ok')
PY
