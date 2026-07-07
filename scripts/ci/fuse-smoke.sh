#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLATFORM="$(uname -s)"

case "$PLATFORM" in
  Linux)
    if [[ ! -e /dev/fuse ]]; then
      echo "fuse-smoke-skip: /dev/fuse is not available" >&2
      exit 0
    fi
    if command -v fusermount3 >/dev/null 2>&1; then
      UNMOUNT=(fusermount3 -u)
    elif command -v fusermount >/dev/null 2>&1; then
      UNMOUNT=(fusermount -u)
    else
      echo "fuse-smoke-skip: fusermount3/fusermount is not available" >&2
      exit 0
    fi
    ;;
  Darwin)
    if [[ ! -d /Library/Filesystems/macfuse.fs ]]; then
      echo "fuse-smoke-skip: macFUSE is not installed" >&2
      exit 0
    fi
    if ! pkg-config --exists fuse >/dev/null 2>&1; then
      echo "fuse-smoke-skip: macFUSE pkg-config metadata is not available" >&2
      exit 0
    fi
    UNMOUNT=(/sbin/umount)
    ;;
  *)
    echo "fuse-smoke-skip: unsupported platform $PLATFORM" >&2
    exit 0
    ;;
esac

is_mounted() {
  local path="$1"
  case "$PLATFORM" in
    Linux) mountpoint -q "$path" ;;
    Darwin) mount | grep -Fq " on $path " ;;
    *) return 1 ;;
  esac
}

wait_for_mount() {
  local mountpoint="$1"
  local pid="$2"
  local log="$3"
  for _ in $(seq 1 80); do
    if is_mounted "$mountpoint"; then
      return 0
    fi
    if ! kill -0 "$pid" >/dev/null 2>&1; then
      if [[ "$PLATFORM" == "Darwin" ]] && grep -Eq 'not available|Operation not permitted|Privacy & Security|approve macFUSE' "$log"; then
        echo "fuse-smoke-skip: macFUSE is installed but not approved/available to the kernel" >&2
        cat "$log" >&2
        exit 0
      fi
      echo "fuse-smoke-fail: mount process exited" >&2
      cat "$log" >&2
      exit 1
    fi
    sleep 0.1
  done
  if [[ "$PLATFORM" == "Darwin" ]] && grep -Eq 'not available|Operation not permitted|Privacy & Security|approve macFUSE' "$log"; then
    echo "fuse-smoke-skip: macFUSE is installed but not approved/available to the kernel" >&2
    cat "$log" >&2
    exit 0
  fi
  echo "fuse-smoke-fail: mountpoint did not become active" >&2
  cat "$log" >&2
  exit 1
}

cd "$ROOT_DIR"
cargo build -p biohazardfs-fuse
cargo build -p biohazardfs-daemon

real_path() {
  python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$1"
}

SOURCE_ROOT="$(real_path "$(mktemp -d)")"
MOUNTPOINT="$(real_path "$(mktemp -d)")"
SMOKE_DIR="$(real_path "$(mktemp -d)")"
FUSE_PID=""
DAEMON_PID=""
RW_MOUNTPOINT="$(real_path "$(mktemp -d)")"
RW_CACHE_DIR="$(real_path "$(mktemp -d)")"

cleanup() {
  "${UNMOUNT[@]}" "$MOUNTPOINT" >/dev/null 2>&1 || true
  "${UNMOUNT[@]}" "$RW_MOUNTPOINT" >/dev/null 2>&1 || true
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

wait_for_mount "$MOUNTPOINT" "$FUSE_PID" "$SMOKE_DIR/fuse.log"

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

"${UNMOUNT[@]}" "$MOUNTPOINT"
wait "$FUSE_PID" || true
FUSE_PID=""

echo "fuse-live-mount-smoke-ok"

# ----- read-write workspace mount: write a file through the mount and read it back -----
# The read-write mount proxies file.write/file.read through the local daemon.
DAEMON_TOKEN="fuse-smoke-local-token"

choose_loopback_port() {
  python3 - <<'PY'
import socket
with socket.socket() as s:
    s.bind(('127.0.0.1', 0))
    print(s.getsockname()[1])
PY
}

start_daemon() {
  for _attempt in $(seq 1 5); do
    DAEMON_PORT="$(choose_loopback_port)"
    DAEMON_ADDR="127.0.0.1:${DAEMON_PORT}"
    DAEMON_HOST="${DAEMON_ADDR%%:*}"
    DAEMON_PORT_NUM="${DAEMON_ADDR##*:}"
    : >"$SMOKE_DIR/daemon.log"
    BIOHAZARDFS_LOCAL_TOKEN="$DAEMON_TOKEN" \
      target/debug/biohazardfsd --dev-loopback-http --addr "$DAEMON_ADDR" \
      >"$SMOKE_DIR/daemon.log" 2>&1 &
    DAEMON_PID=$!

    for _ in $(seq 1 80); do
      if python3 -c "import socket; s=socket.create_connection(('$DAEMON_HOST', $DAEMON_PORT_NUM), timeout=0.5); s.close()" 2>/dev/null; then
        return 0
      fi
      if ! kill -0 "$DAEMON_PID" >/dev/null 2>&1; then
        if grep -Eqi 'address.*in use|addr.*in use|os error 48|os error 98' "$SMOKE_DIR/daemon.log"; then
          wait "$DAEMON_PID" 2>/dev/null || true
          DAEMON_PID=""
          break
        fi
        echo "fuse-smoke-fail: daemon process exited" >&2
        cat "$SMOKE_DIR/daemon.log" >&2
        exit 1
      fi
      sleep 0.1
    done

    if [[ -n "$DAEMON_PID" ]]; then
      echo "fuse-smoke-fail: daemon did not accept loopback connections" >&2
      cat "$SMOKE_DIR/daemon.log" >&2
      exit 1
    fi
  done
  echo "fuse-smoke-fail: daemon could not bind an ephemeral loopback port" >&2
  cat "$SMOKE_DIR/daemon.log" >&2
  exit 1
}

start_daemon

BIOHAZARDFS_LOCAL_TOKEN="$DAEMON_TOKEN" \
target/debug/biohazardfs-fuse mount-workspace \
  --daemon-endpoint "$DAEMON_ADDR" \
  --cache-dir "$RW_CACHE_DIR" \
  --mountpoint "$RW_MOUNTPOINT" \
  >"$SMOKE_DIR/rw-fuse.log" 2>&1 &
FUSE_PID=$!

wait_for_mount "$RW_MOUNTPOINT" "$FUSE_PID" "$SMOKE_DIR/rw-fuse.log"

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

mkdir "$RW_MOUNTPOINT/rename-me"
mv "$RW_MOUNTPOINT/rename-me" "$RW_MOUNTPOINT/renamed-ok"
printf 'rename payload' >"$RW_MOUNTPOINT/file-rename-me.txt"
mv "$RW_MOUNTPOINT/file-rename-me.txt" "$RW_MOUNTPOINT/file-renamed-ok.txt"
if [[ ! -d "$RW_MOUNTPOINT/renamed-ok" || "$(cat "$RW_MOUNTPOINT/file-renamed-ok.txt")" != 'rename payload' ]]; then
  echo "fuse-smoke-fail: rename round trip failed" >&2
  exit 1
fi

# Truncate an existing mounted file to zero and assert the truncate takes
# effect end-to-end.
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

"${UNMOUNT[@]}" "$RW_MOUNTPOINT"
wait "$FUSE_PID" || true
FUSE_PID=""
kill "$DAEMON_PID" >/dev/null 2>&1 || true
wait "$DAEMON_PID" 2>/dev/null || true
DAEMON_PID=""

echo "fuse-workspace-write-smoke-ok"
