# Installation and Auto-Update Implementation

Status: scaffold reference
Audience: desktop app implementers, release owners, CI maintainers

This document describes the first install/update foundation for Biohazard Workspace.
It does not claim production installer quality yet.

## Current scope

Implemented scaffold:

- Electron Builder packaging configuration for macOS DMG, Windows NSIS, Linux AppImage, and Linux deb artifacts.
- Desktop packaging scripts in `apps/workspace-electron/package.json`.
- Rust binary staging script: `scripts/release/prepare-desktop-resources.sh`.
- Packaged-only update-check bridge using `electron-updater`.
- Settings UI for release channel, auto-check preference, and manual update check.
- pnpm build-script allowlist for Electron packaging dependencies.

Not implemented yet:

- code signing
- macOS notarization
- Windows Authenticode signing
- automatic download/apply/restart
- daemon restart orchestration during update
- platform service/autostart installation
- uninstall/purge UI
- published release workflow, draft release automation, and checksums

## Packaging commands

Run from `apps/workspace-electron` unless noted.

Build the desktop app only:

```bash
pnpm run build
```

Build Rust release binaries required by the installer:

```bash
pnpm run build:rust:release
```

Stage release binaries and metadata for Electron Builder:

```bash
pnpm run prepare:desktop-resources
```

Set the release channel with `BIOHAZARDFS_RELEASE_CHANNEL`; valid values are `dev`, `nightly`, `alpha`, `beta`, and `stable`. The default is `stable` for release packaging.

```bash
BIOHAZARDFS_RELEASE_CHANNEL=beta pnpm run prepare:desktop-resources
```

The staging script validates the channel, first removes stale generated binaries from `resources/bin/`, then copies required binaries from `target/release` into:

```text
apps/workspace-electron/resources/bin/<platform>-<arch>/
```

Required binaries:

- `biohazardfs`
- `biohazardfsd`

Optional when present:

- `biohazardfs-fuse`

It also writes:

```text
apps/workspace-electron/release-metadata.json
```

Build unpacked installer directory:

```bash
pnpm run dist:dir
```

Build platform artifacts for the current host:

```bash
pnpm run dist
```

Platform-specific shortcuts:

```bash
pnpm run dist:linux
pnpm run dist:mac
pnpm run dist:win
```

The `dist*` scripts rebuild the desktop app, rebuild Rust release binaries, stage resources, then call `scripts/release/electron-builder.sh`, which validates `BIOHAZARDFS_RELEASE_CHANNEL`, maps `stable` to Electron Builder's `latest` update channel, and passes the selected channel into generated updater metadata.

Publishing defaults to `never` to prevent unsigned scaffold artifacts from being uploaded accidentally. Release workflows must opt in explicitly with `BIOHAZARDFS_ELECTRON_PUBLISH=onTag`, `onTagOrDraft`, or `always` after signing/notarization policy exists.

Cross-platform packaging is intentionally blocked for bundled Rust resources right now: the staging script fails if a requested target platform/arch does not match the host-staged binaries. Use platform-specific runners/signing setup. Do not promise a platform artifact until that platform has passed install and smoke validation.

## Electron Builder contract

The Electron package config uses:

- app ID: `com.biohazardvfx.biohazardfs.workspace`
- product name: `Biohazard Workspace`
- output directory: `apps/workspace-electron/release/`
- extra resources:
  - `resources/bin/**`
  - `release-metadata.json`

Artifact targets:

- macOS: DMG and ZIP (ZIP is required for updater metadata)
- Windows: NSIS `.exe`
- Linux: AppImage and deb

Artifacts are unsigned until signing/notarization is added. Unsigned builds are dev/nightly/internal artifacts only. Draft GitHub release creation belongs in the release workflow, not in the updater metadata shipped to clients.

## Update behavior

Biohazard Workspace includes an update-check bridge, but it is deliberately conservative.

Rules:

- Update checks run only in packaged builds.
- Development runs return `unavailable` with a clear message.
- `electron-updater` is configured with `autoDownload = false`.
- `autoInstallOnAppQuit = false`.
- Manual download/apply is deferred until daemon restart and dirty-upload safety are implemented.
- Release channel is visible and stored in Electron prefs.
- Auto-check preference is stored, but checks still require packaged builds.

Settings exposes:

- release channel: `dev`, `nightly`, `alpha`, `beta`, `stable`
- auto-check enabled/disabled
- manual `Check now`
- last update state/message

Channel mapping:

- packaged first-run defaults read `release-metadata.json.channel`
- if packaged metadata is missing/invalid, default channel is `stable`
- development runs default to `dev`
- `stable` -> updater channel `latest`
- all other channels -> matching updater channel name

## Safety requirements before automatic apply

Do not enable automatic download/apply until all are true:

1. Daemon can report dirty uploads and active transfers reliably.
2. Desktop can block or defer update apply when dirty uploads exist.
3. Daemon restart/update flow is explicit and tested.
4. CLI/daemon/server compatibility is checked for the update target.
5. Rollback or recovery guidance exists for failed install/update.
6. Code signing/notarization requirements are met for the target channel.

## Generated files

Ignored/generated paths:

```text
apps/workspace-electron/release/
apps/workspace-electron/release-metadata.json
apps/workspace-electron/resources/bin/*
```

The `.gitkeep` files under `resources/` and `resources/bin/` are tracked so the expected staging directories remain visible.
