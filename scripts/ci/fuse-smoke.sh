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

SOURCE_ROOT="$(mktemp -d)"
MOUNTPOINT="$(mktemp -d)"
SMOKE_DIR="$(mktemp -d)"
FUSE_PID=""

cleanup() {
  "$FUSERMOUNT" -u "$MOUNTPOINT" >/dev/null 2>&1 || true
  if [[ -n "$FUSE_PID" ]]; then
    kill "$FUSE_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$SOURCE_ROOT" "$MOUNTPOINT" "$SMOKE_DIR"
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

echo "fuse-live-mount-smoke-ok"
