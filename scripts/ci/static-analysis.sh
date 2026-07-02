#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check --workspace --all-features
cargo test --workspace --all-features

if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check advisories bans licenses sources
else
  echo "cargo-deny not installed; skipping local cargo-deny check" >&2
fi

pnpm --dir apps/workspace-electron install --frozen-lockfile
pnpm --dir apps/workspace-electron run static
pnpm --dir apps/workspace-electron run build

if command -v shellcheck >/dev/null 2>&1; then
  mapfile -t shell_scripts < <(find scripts -type f -name '*.sh' | sort)
  if ((${#shell_scripts[@]} > 0)); then
    shellcheck "${shell_scripts[@]}"
  fi
else
  echo "shellcheck not installed; skipping local shellcheck" >&2
fi

if command -v actionlint >/dev/null 2>&1; then
  actionlint
else
  echo "actionlint not installed; skipping local actionlint" >&2
fi

if command -v hadolint >/dev/null 2>&1; then
  hadolint deploy/docker/server/Dockerfile
else
  echo "hadolint not installed; skipping local hadolint" >&2
fi

git diff --check
