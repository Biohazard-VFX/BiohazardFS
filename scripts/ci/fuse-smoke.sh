#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ ! -e /dev/fuse ]]; then
  echo "fuse-smoke-skip: /dev/fuse is not available" >&2
  exit 0
fi

if command -v fusermount3 >/dev/null 2>&1; then
  FUSERMOUNT="fusermount3"
elif command -v fusermount >/dev/null 2>&1; then
  FUSERMOUNT="fusermount"
else
  echo "fuse-smoke-skip: fusermount3/fusermount is not available" >&2
  exit 0
fi

cd "$ROOT_DIR"
cargo build -p biohazardfs-fuse
cargo build -p biohazardfs-daemon

SOURCE_ROOT="$(mktemp -d)"
MOUNTPOINT="$(mktemp -d)"
SMOKE_DIR="$(mktemp -d)"
FUSE_PID=""
DAEMON_PID=""
RW_MOUNTPOINT="$(mktemp -d)"
RW_CACHE_DIR="$(mktemp -d)"

cleanup() {
  "$FUSERMOUNT" -u "$MOUNTPOINT" >/dev/null 2>&1 || true
  "$FUSERMOUNT" -u "$RW_MOUNTPOINT" >/dev/null 2>&1 || true
  if [[ -n "$FUSE_PID" ]]; then
    kill "$FUSE_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$DAEMON_PID" ]]; then
    kill "$DAEMON_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$SOURCE_ROOT" "$MOUNTPOINT" "$SMOKE_DIR" "$RW_MOUNTPOINT" "$RW_CACHE_DIR"
}
trap cleanup EXIT

mkdir -p "$SOURCE_ROOT/plates"
printf 'virtual mount smoke\n' >"$SOURCE_ROOT/plates/shot001.txt"

target/debug/biohazardfs-fuse mount --source "$SOURCE_ROOT" --mountpoint "$MOUNTPOINT" >"$SMOKE_DIR/fuse.log" 2>&1 &
FUSE_PID=$!

for _ in $(seq 1 80); do
  if mountpoint -q "$MOUNTPOINT"; then
    break
  fi
  if ! kill -0 "$FUSE_PID" >/dev/null 2>&1; then
    echo "fuse-smoke-fail: mount process exited" >&2
    cat "$SMOKE_DIR/fuse.log" >&2
    exit 1
  fi
  sleep 0.1
done

if ! mountpoint -q "$MOUNTPOINT"; then
  echo "fuse-smoke-fail: mountpoint did not become active" >&2
  cat "$SMOKE_DIR/fuse.log" >&2
  exit 1
fi

ls "$MOUNTPOINT/plates" >"$SMOKE_DIR/listing.txt"
cat "$MOUNTPOINT/plates/shot001.txt" >"$SMOKE_DIR/content.txt"
python3 - "$SMOKE_DIR" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1])
assert root.joinpath('listing.txt').read_text().strip() == 'shot001.txt'
assert root.joinpath('content.txt').read_text() == 'virtual mount smoke\n'
PY

if python3 - "$MOUNTPOINT" 2>"$SMOKE_DIR/write.err" <<'PY'
from pathlib import Path
import sys
Path(sys.argv[1], 'plates/write.txt').write_text('nope')
PY
then
  echo "fuse-smoke-fail: write unexpectedly succeeded" >&2
  exit 1
fi

"$FUSERMOUNT" -u "$MOUNTPOINT"
wait "$FUSE_PID" || true
FUSE_PID=""

echo "fuse-live-mount-smoke-ok"

# ----- read-write workspace mount: write a file through the mount and read it back -----
# The read-write mount proxies file.write/file.read through the local daemon.
DAEMON_PORT=47666
DAEMON_ADDR="127.0.0.1:${DAEMON_PORT}"
DAEMON_TOKEN="fuse-smoke-local-token"

# The dev-loopback daemon refuses non-loopback addrs and requires a token env.
BIOHAZARDFS_LOCAL_TOKEN="$DAEMON_TOKEN" \
  target/debug/biohazardfsd --dev-loopback-http --addr "$DAEMON_ADDR" \
  >"$SMOKE_DIR/daemon.log" 2>&1 &
DAEMON_PID=$!

DAEMON_HOST="${DAEMON_ADDR%%:*}"
DAEMON_PORT_NUM="${DAEMON_ADDR##*:}"
for _ in $(seq 1 80); do
  if python3 -c "import socket,sys; s=socket.create_connection(('$DAEMON_HOST', $DAEMON_PORT_NUM), timeout=0.5); s.close()" 2>/dev/null; then
    break
  fi
  if ! kill -0 "$DAEMON_PID" >/dev/null 2>&1; then
    echo "fuse-smoke-fail: daemon process exited" >&2
    cat "$SMOKE_DIR/daemon.log" >&2
    exit 1
  fi
  sleep 0.1
done

BIOHAZARDFS_LOCAL_TOKEN="$DAEMON_TOKEN" \
target/debug/biohazardfs-fuse mount-workspace \
  --daemon-endpoint "$DAEMON_ADDR" \
  --cache-dir "$RW_CACHE_DIR" \
  --mountpoint "$RW_MOUNTPOINT" \
  >"$SMOKE_DIR/rw-fuse.log" 2>&1 &
FUSE_PID=$!

for _ in $(seq 1 80); do
  if mountpoint -q "$RW_MOUNTPOINT"; then
    break
  fi
  if ! kill -0 "$FUSE_PID" >/dev/null 2>&1; then
    echo "fuse-smoke-fail: read-write mount process exited" >&2
    cat "$SMOKE_DIR/rw-fuse.log" >&2
    exit 1
  fi
  sleep 0.1
done

if ! mountpoint -q "$RW_MOUNTPOINT"; then
  echo "fuse-smoke-fail: read-write mountpoint did not become active" >&2
  cat "$SMOKE_DIR/rw-fuse.log" >&2
  exit 1
fi

# Write a file through the FUSE mount, then read it back and assert bytes match.
RW_PAYLOAD="read-write workspace payload $(date +%s)"
printf '%s' "$RW_PAYLOAD" >"$RW_MOUNTPOINT/through_mount.txt"
READ_BACK="$(cat "$RW_MOUNTPOINT/through_mount.txt")"
if [[ "$READ_BACK" != "$RW_PAYLOAD" ]]; then
  echo "fuse-smoke-fail: read-write round trip mismatch" >&2
  echo "expected: $RW_PAYLOAD" >&2
  echo "actual:   $READ_BACK" >&2
  exit 1
fi

# Truncate an existing mounted file to zero and assert the truncate takes
# effect end-to-end (round-3 repro: `: > existing` previously no-op'd through
# setattr(size=0) and the mount kept reading the old content).
printf 'abc' >"$RW_MOUNTPOINT/truncate-me.txt"
if [[ "$(cat "$RW_MOUNTPOINT/truncate-me.txt")" != "abc" ]]; then
  echo "fuse-smoke-fail: truncate setup write mismatch" >&2
  exit 1
fi
: >"$RW_MOUNTPOINT/truncate-me.txt"
TRUNC_AFTER="$(cat "$RW_MOUNTPOINT/truncate-me.txt")"
if [[ -n "$TRUNC_AFTER" ]]; then
  echo "fuse-smoke-fail: truncate-to-zero did not take effect (got: $TRUNC_AFTER)" >&2
  exit 1
fi

"$FUSERMOUNT" -u "$RW_MOUNTPOINT"
wait "$FUSE_PID" || true
FUSE_PID=""
kill "$DAEMON_PID" >/dev/null 2>&1 || true
wait "$DAEMON_PID" 2>/dev/null || true
DAEMON_PID=""

echo "fuse-workspace-write-smoke-ok"
