#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
APP_DIR="$ROOT_DIR/apps/workspace-electron"
CHANNEL="${BIOHAZARDFS_RELEASE_CHANNEL:-stable}"
PUBLISH_POLICY="${BIOHAZARDFS_ELECTRON_PUBLISH:-never}"

case "$CHANNEL" in
  dev|nightly|alpha|beta|stable) ;;
  *)
    printf 'invalid BIOHAZARDFS_RELEASE_CHANNEL: %s\n' "$CHANNEL" >&2
    printf 'expected one of: dev, nightly, alpha, beta, stable\n' >&2
    exit 1
    ;;
esac

case "$PUBLISH_POLICY" in
  never|onTag|onTagOrDraft|always) ;;
  *)
    printf 'invalid BIOHAZARDFS_ELECTRON_PUBLISH: %s\n' "$PUBLISH_POLICY" >&2
    printf 'expected one of: never, onTag, onTagOrDraft, always\n' >&2
    exit 1
    ;;
esac

BUILDER_CHANNEL="$CHANNEL"
if [[ "$CHANNEL" == "stable" ]]; then
  BUILDER_CHANNEL="latest"
fi

TMP_CONFIG="$(mktemp "${TMPDIR:-/tmp}/biohazardfs-electron-builder.XXXXXX").json"
trap 'rm -f "$TMP_CONFIG"' EXIT

python3 - <<'PY' "$APP_DIR/package.json" "$TMP_CONFIG" "$BUILDER_CHANNEL"
import json
import sys
from pathlib import Path

package_path, out_path, channel = sys.argv[1:]
package = json.loads(Path(package_path).read_text())
config = dict(package["build"])
publish = config.get("publish")
if isinstance(publish, list):
    config["publish"] = [
        {**entry, "channel": channel} if isinstance(entry, dict) else entry for entry in publish
    ]
elif isinstance(publish, dict):
    config["publish"] = {**publish, "channel": channel}
Path(out_path).write_text(json.dumps(config, indent=2) + "\n")
PY

cd "$APP_DIR"
exec pnpm exec electron-builder --config "$TMP_CONFIG" --publish "$PUBLISH_POLICY" "$@"
