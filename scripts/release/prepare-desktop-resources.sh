#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_DIR="$ROOT_DIR/apps/workspace-electron"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
TARGET_PLATFORM=""
TARGET_ARCH=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target-platform)
      TARGET_PLATFORM="${2:-}"
      shift 2
      ;;
    --target-arch)
      TARGET_ARCH="${2:-}"
      shift 2
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      exit 1
      ;;
  esac
done
RELEASE_DIR="$TARGET_DIR/release"
VERSION="$(python3 - <<'PY' "$APP_DIR/package.json"
import json, sys
from pathlib import Path
print(json.loads(Path(sys.argv[1]).read_text())["version"])
PY
)"
CHANNEL="${BIOHAZARDFS_RELEASE_CHANNEL:-stable}"
case "$CHANNEL" in
  dev|nightly|alpha|beta|stable) ;;
  *)
    printf 'invalid BIOHAZARDFS_RELEASE_CHANNEL: %s\n' "$CHANNEL" >&2
    printf 'expected one of: dev, nightly, alpha, beta, stable\n' >&2
    exit 1
    ;;
esac
COMMIT="$(git -C "$ROOT_DIR" rev-parse HEAD)"
SHORT_COMMIT="$(git -C "$ROOT_DIR" rev-parse --short HEAD)"
BUILT_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

case "$(uname -s)" in
  Darwin) PLATFORM="mac" ;;
  Linux) PLATFORM="linux" ;;
  MINGW*|MSYS*|CYGWIN*) PLATFORM="win" ;;
  *) PLATFORM="unknown" ;;
esac

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64) ARCH="x64" ;;
  aarch64|arm64) ARCH="arm64" ;;
esac

TARGET_PLATFORM="${TARGET_PLATFORM:-$PLATFORM}"
TARGET_ARCH="${TARGET_ARCH:-$ARCH}"

case "$TARGET_PLATFORM" in
  linux|mac|win) ;;
  *)
    printf 'invalid target platform: %s\n' "$TARGET_PLATFORM" >&2
    printf 'expected one of: linux, mac, win\n' >&2
    exit 1
    ;;
esac

if [[ "$TARGET_PLATFORM" != "$PLATFORM" || "$TARGET_ARCH" != "$ARCH" ]]; then
  printf 'cross-platform desktop resource staging is not supported yet\n' >&2
  printf 'host: %s-%s\n' "$PLATFORM" "$ARCH" >&2
  printf 'target: %s-%s\n' "$TARGET_PLATFORM" "$TARGET_ARCH" >&2
  exit 1
fi

PLATFORM="$TARGET_PLATFORM"
ARCH="$TARGET_ARCH"

BIN_SUFFIX=""
if [[ "$PLATFORM" == "win" ]]; then
  BIN_SUFFIX=".exe"
fi

BIN_ROOT="$APP_DIR/resources/bin"
mkdir -p "$BIN_ROOT"
find "$BIN_ROOT" -mindepth 1 ! -name .gitkeep -exec rm -rf {} +
DEST="$BIN_ROOT/${PLATFORM}-${ARCH}"
mkdir -p "$DEST"

required=("biohazardfs" "biohazardfsd")
optional=("biohazardfs-fuse")

missing=()
for bin in "${required[@]}"; do
  src="$RELEASE_DIR/${bin}${BIN_SUFFIX}"
  if [[ ! -x "$src" ]]; then
    missing+=("${bin}${BIN_SUFFIX}")
    continue
  fi
  cp "$src" "$DEST/"
done

if (( ${#missing[@]} > 0 )); then
  printf 'missing required release binaries in %s:\n' "$RELEASE_DIR" >&2
  printf '  - %s\n' "${missing[@]}" >&2
  printf 'run: cargo build --release --bins\n' >&2
  exit 1
fi

for bin in "${optional[@]}"; do
  src="$RELEASE_DIR/${bin}${BIN_SUFFIX}"
  if [[ -x "$src" ]]; then
    cp "$src" "$DEST/"
  fi
done

python3 - <<'PY' "$APP_DIR/release-metadata.json" "$VERSION" "$CHANNEL" "$COMMIT" "$SHORT_COMMIT" "$BUILT_AT" "$PLATFORM" "$ARCH"
import json, sys
from pathlib import Path
out, version, channel, commit, short, built_at, platform, arch = sys.argv[1:]
Path(out).write_text(json.dumps({
    "product": "BiohazardFS",
    "desktop_app": "Biohazard Workspace",
    "version": version,
    "channel": channel,
    "commit": commit,
    "short_commit": short,
    "built_at": built_at,
    "platform": platform,
    "arch": arch,
}, indent=2) + "\n")
PY

echo "desktop resources staged: $DEST"
echo "release metadata: $APP_DIR/release-metadata.json"
