# BiohazardFS Packaging and Release Contract

Status: draft reference
Audience: release owners, desktop app implementers, installer authors, CI maintainers, operators

BiohazardFS packaging should feel like one product, not a bag of binaries. A normal artist should install Biohazard Workspace once and be ready to use the desktop app, daemon, filesystem integration, and CLI without manual wiring.

## Core decisions

- Public/open-source distribution discipline starts from the beginning.
- Primary distribution is one platform-native installer per OS.
- The desktop installer installs the desktop app, CLI, daemon, autostart service, and required integration helpers.
- Use one product version across desktop app, CLI, daemon, and server/control-plane artifacts at first.
- Release channels are `dev`, `nightly`, `alpha`, `beta`, and `stable`.
- Code signing/notarization is required before serious public/stable distribution, but not required for earliest MVP/internal artifacts.
- Uninstall must not delete cache, config, or user data unless the user explicitly chooses a purge/remove-data option.

## Product and component model

The product is BiohazardFS / Biohazard Workspace.

Installed components:

```text
Biohazard Workspace desktop app
biohazardfs CLI
biohazardfsd per-user daemon
platform autostart registration
filesystem/placeholder integration helpers
uninstaller
```

Artifact names may expose components, but the user-facing install experience should be one installer.

Examples:

```text
BiohazardFS-0.1.0-alpha.1-macos-universal.dmg
BiohazardFS-0.1.0-alpha.1-windows-x64.exe
BiohazardFS-0.1.0-alpha.1-linux-x86_64.AppImage
biohazardfs-server-0.1.0-alpha.1-container.tar, later
```

## Platform installers

### macOS

Primary artifact:

```text
.dmg containing Biohazard Workspace.app
```

Installer responsibilities:

- install/copy desktop app
- bundle CLI and daemon inside the app package or install them into a managed app support location
- register a per-user launch agent for `biohazardfsd`
- add/update shell PATH helper or provide a documented CLI shim
- request permissions needed for filesystem integration
- preserve config/cache on uninstall unless explicit purge requested

Future stable/public requirements:

- code signing
- notarization
- universal or clearly separated architecture artifacts
- Sparkle or equivalent auto-update path if adopted

### Windows

Primary artifacts:

```text
.exe installer, preferably user-friendly
.msi optional/enterprise later
```

Installer responsibilities:

- install Biohazard Workspace
- install CLI and daemon
- register per-user startup task/service behavior for `biohazardfsd`
- install or validate filesystem/placeholder prerequisites
- add CLI to user PATH or provide shell integration
- preserve config/cache on uninstall unless explicit purge requested

Future stable/public requirements:

- Authenticode signing
- SmartScreen-friendly release reputation
- MSI or enterprise deployment path if studios need managed rollout

### Linux

Primary artifacts:

```text
.AppImage
deb
rpm
```

Installer responsibilities:

- install desktop app where appropriate
- install CLI and daemon
- register per-user systemd service/autostart for `biohazardfsd` where supported
- validate FUSE/filesystem prerequisites
- add CLI to PATH through package install or documented shim
- preserve config/cache on uninstall unless explicit purge requested

Packaging notes:

- AppImage is the broadest low-friction desktop artifact.
- `deb` and `rpm` are better for managed installs and dependency declarations.
- Package scripts are production code and must be tested.

## One installer rule

The normal install path is:

1. User downloads platform installer.
2. User installs Biohazard Workspace.
3. Installer provides CLI and daemon.
4. Installer registers per-user daemon autostart.
5. User opens app or runs CLI.
6. User enrolls with invite/device code/token.
7. Mount/workspace becomes available.

Users should not manually:

- download separate CLI and daemon binaries
- configure daemon service files by hand
- copy helper binaries into random locations
- manually create socket/token runtime directories
- manually wire desktop app to daemon

Advanced standalone CLI/server artifacts may exist later, but they are not the primary artist distribution path.

## Daemon install behavior

Default artist install:

- per-user daemon
- auto-start at login
- no privileged system service by default
- daemon owns user's mount/cache/local state
- CLI/Electron can start daemon on demand if autostart failed

System-service mode may be added later for render/headless nodes, but it must be a distinct install mode with separate docs and security review.

## Versioning

Use one product version across components at first:

```text
BiohazardFS version = desktop app version = CLI version = daemon version = server/control-plane version
```

Rules:

- All release artifacts for a release channel share the same version.
- CLI/daemon/server protocol compatibility is tested as part of release validation once those surfaces exist.
- If component versions diverge later, the compatibility matrix must be explicit and machine-readable.

Version format:

```text
0.1.0-dev.<build>
0.1.0-nightly.<date>+<sha>
0.1.0-alpha.1
0.1.0-beta.1
0.1.0
```

SemVer is the public versioning baseline.

## Release channels

### dev

Purpose:

- local/dev builds
- frequent internal validation
- not necessarily user-safe

Rules:

- can be unsigned
- can skip platform installers if clearly marked
- must not be confused with stable artifacts

### nightly

Purpose:

- automated build from main or scheduled build
- public/open-source practice from the beginning
- useful for testers who accept risk

Rules:

- CI must be green
- artifact names include date and commit SHA
- release notes can be generated/minimal
- no stability promise

### alpha

Purpose:

- internal/Biohazard-first and early external technical testers
- validates installer and onboarding flow

Rules:

- installer should be usable end-to-end
- known limitations documented
- smoke tests required for claimed platforms/features

### beta

Purpose:

- broader public testing
- packaging and update paths should be realistic

Rules:

- no known critical data-loss/security blockers
- platform smoke required for every platform artifact published
- upgrade/uninstall behavior tested

### stable

Purpose:

- public supported release

Rules:

- code signing/notarization expected where platform requires it
- documented release notes
- compatibility/migration notes
- CI and smoke green
- no critical data-loss/security blockers

## Release gates

Release artifacts must not be published if any of these are true:

- required CI is failing
- required smoke tests for claimed features/platforms are missing or failing
- known critical data-loss blocker is open
- known critical security blocker is open
- installer cannot install CLI + daemon + desktop together for the claimed platform
- uninstall behavior risks deleting cache/config/user data without explicit consent

Platform-specific artifacts require platform-specific validation.

Examples:

- Publishing a Linux AppImage that claims FUSE mount support requires Linux smoke for that support.
- Publishing a Windows installer that claims Explorer placeholder state requires Windows smoke for that support.
- Publishing a macOS build that claims Finder integration requires macOS smoke for that support.

## Checksums, provenance, and release notes

Every published artifact should include:

- checksum
- version
- commit SHA
- channel
- target OS/architecture
- build timestamp
- release notes or changelog entry

Future public/stable releases should include:

- SBOM
- signed checksums or attestations
- reproducibility/provenance metadata where practical

## Code signing and trust

Earliest MVP/dev artifacts may be unsigned if clearly marked.

Before serious public or stable distribution:

- macOS app should be signed and notarized
- Windows installer/binaries should be signed
- Linux packages should provide checksums and repository/package signatures where appropriate

Unsigned artifacts must never pretend to be production-stable.

## Auto-update

Installation/update implementation details live in `docs/reference/INSTALLATION_AND_UPDATES.md`.

Auto-update is desirable but must stay conservative until daemon restart and dirty-upload safety exist.

Current scaffold:

- Electron Builder packaging config exists.
- Release channel is visible/configurable in Settings.
- Packaged-only manual update checks use `electron-updater`.
- Auto-download and install-on-quit are disabled.

Future auto-update rules:

- updates must preserve cache/config/user data
- updates must handle daemon restart safely
- updates must not interrupt active dirty uploads without safe pause/resume
- update channel must be visible and configurable
- CLI and daemon compatibility must be checked before/after update

## Server/control-plane packaging

Server/control-plane packaging is secondary to desktop installer UX but should follow the same version/channel model.

Likely future artifacts:

```text
container image
Helm chart or equivalent deployment bundle
migration job/container
server CLI/admin tool, if needed
```

Rules:

- server artifacts share product version with desktop/CLI/daemon at first
- schema migrations must be explicit and reversible where practical
- server releases must state client compatibility
- self-hosted deployment docs must avoid requiring private Biohazard infrastructure

## Uninstall behavior

Default uninstall removes:

- desktop app
- installed CLI/daemon binaries
- autostart registration
- helper services/tasks installed by the app

Default uninstall preserves:

- local cache
- local config
- credentials/keychain entries unless user requests removal
- logs/support data unless user requests removal

Optional purge/remove-data mode may remove preserved data, but it must be explicit and clearly warn the user.

## First implementation target

The first scaffold now exists, but it is not yet a production installer. The next packaging implementation should prove the one-installer path on at least one platform:

1. Build desktop shell artifact.
2. Bundle CLI and daemon binaries.
3. Register per-user daemon autostart.
4. Verify CLI can find/talk to daemon after install.
5. Verify uninstall preserves cache/config by default.
6. Produce checksum and version metadata.
7. Publish as dev/nightly artifact after green CI.

Do not claim alpha installer quality until the installer can install desktop app + CLI + daemon together and pass platform smoke for the claimed platform.
