# BiohazardFS Product Spec

Status: draft scaffold
Primary product: Biohazard Workspace
Core filesystem/client: BiohazardFS

## 1. Product summary

BiohazardFS is an open-source LucidLink-style virtual sync filesystem for VFX production. It provides a mounted workspace where authorized files appear instantly as placeholders, hydrate into local cache on access or pinning, sync safe writes back to server storage, and allow users to remove local copies without deleting cloud data.

BiohazardFS is built for Biohazard first and should later become a public Apache-2.0-licensed product other studios can self-host and use commercially.

## 2. Opinionated stack

BiohazardFS should be opinionated but not Biohazard-only.

Required/expected backend primitives:

- S3-compatible object storage: RustFS, MinIO, Cloudflare R2, AWS S3, etc.
- PostgreSQL for control-plane metadata.
- BiohazardFS server/control plane.

Optional integrations:

- Kitsu for projects, task assignments, worksets, folder templates, and publish/version writeback.
- Google identity/workspace style auth later.
- SSO/OIDC/SAML later.
- Frame.io/review integrations later.

JuiceFS may remain useful infrastructure, but BiohazardFS should not require artists to manage JuiceFS directly.

## 3. Product requirements from discovery

### Users

First users:

- Nicholai
- internal Biohazard artists
- agents
- eventually freelancers/vendors/clients

A nontechnical freelancer must be able to onboard in under 10 minutes from a link or invite.

### Platforms

MVP targets:

- Linux: real target and first dogfood platform.
- Windows: mandatory artist platform.
- macOS: mandatory MVP target.

### UX

- Default mount name: `Biohazard`.
- Windows: choose drive letter, default to next available.
- macOS: mount appears in Finder sidebar automatically if possible.
- Linux/macOS: choose mount location.
- Users see authorized root hierarchy; unauthorized folders are hidden.
- Native Explorer/Finder status badges/placeholders are MVP requirements.
- App should feel like a small, beautiful utility that lives in tray/menu bar.
- Command palette is desired.

### Filesystem behavior

- Files appear instantly as placeholders.
- Opening a file hydrates it into local cache.
- MVP may hydrate full file before open for DCC safety.
- Future streaming can be added for video/media workloads.
- Image sequence nearby-frame prefetch is desirable.
- Folder status is required.
- Symlinks are required.
- Executable files and permissions are required.
- Git compatibility is required.

### Cache behavior

- User chooses cache location during setup.
- Default cache location: home directory if skipped.
- Default cache limit: free space minus safety buffer, e.g. 500GB free -> 450GB limit.
- Cache should be understandable to people who do not know what caching means.
- Cache should be viewable by directory/project in the app.
- User can manually select folders/files in a tree and cache/prefetch them.
- Cache fill behavior is an open question; default should pause downloads and ask rather than risking data loss.
- Dirty/unuploaded files must never be evicted automatically.
- Clear-all-local-cache panic button is required.
- Moving cache after setup is required.
- Multiple cache drives are desirable after MVP.

### Sync language

Preferred artist-facing language:

- Make available offline
- Cache
- Remove local copy
- Dehydrate/uncache in advanced/dev contexts

Cloud delete must be separate from local cache removal. Cloud deletes go to trash/recycler.

### Auth and onboarding

Initial auth should avoid WorkOS.

Acceptable auth flows:

- invite code
- device code
- generated user API token
- Kitsu-linked token
- Google sign-in later
- SSO/2FA later by architecture

Ideal freelancer flow:

1. Admin provisions invite/download link.
2. Freelancer installs Biohazard Workspace.
3. Invite/token is already embedded or pasted.
4. User enters name if needed.
5. Mount appears with assigned workset.

Devices must be revocable individually.

### Permissions/access

Access can come from:

- BiohazardFS itself
- Kitsu assignments/worksets
- Google/workspace identity later
- folder shares
- invite links

Permissions should support at least:

- hidden/no access
- read
- write/edit
- admin/manage
- client/vendor share access
- expiry windows
- download/share limits

Kitsu should be the source of truth for assignments when integrated, but BiohazardFS must work without Kitsu.

### Versioning, audit, snapshots, and conflict model

BiohazardFS should copy LucidLink's broad filesystem model rather than using Git/Git LFS as the live filesystem core.

Core decision:

- Do not make raw Git or Git LFS the primary storage engine.
- Use a mounted virtual filesystem with local cache/pinning, distributed locks, immutable file versions, an append-only event journal, and point-in-time snapshots.
- Make audit and version control explicit and agent-friendly.
- Keep Git/Git LFS optional for code, templates, manifests, explicit import/export, or project-specific workflows.

Required primitives:

- Every committed write creates an immutable file version.
- Every meaningful operation creates an audit event with actor, device, source, path, object IDs, and timestamp.
- Source/provenance must distinguish UI, CLI, agent, API, and server automation.
- Snapshots capture point-in-time filesystem/project/workset state.
- Snapshots are mountable/browsable read-only when feasible.
- Restores copy or promote data from snapshots without destroying current data by default.
- Conflicts preserve every version and never silently overwrite.
- File locks protect binary/scene files where merging is impossible.

### Writes and conflicts

MVP must support writes.

- Users edit directly inside mounted drive.
- Working files sync immediately on save.
- Uploads resume after restart/internet loss.
- After client crash, daemon should enter safe mode, pause if needed, and notify user.
- If two users edit same file, preserve both versions and notify both users.
- Binary scene files need locks.
- Image sequences/cache folders probably skip locking.
- Conflicts appear in mounted filesystem and Workspace app.
- Every conflicting version is preserved.

### Project tracking and templates

BiohazardFS must not enforce one studio folder structure in code.

It should support:

- folder templates
- Kitsu project folder creation when integrated
- optional `.kitsu.json` or equivalent metadata markers for shots/tasks
- Kitsu publish/version writeback

Artists should not manually create top-level project folders.

### Filesystem and cache semantics

Filesystem and cache semantics are specified in `docs/architecture/FILESYSTEM_SEMANTICS.md` and are product requirements, not implementation details.

- Preserve filename case, but enforce case-insensitive sibling uniqueness by default.
- Delete in mounted workspace means server/cloud trash; local cache removal is separate dehydrate/uncache behavior.
- Writes are durable locally before upload/commit, and server versions commit only after content integrity verification.
- MVP hydrates full files before normal open/edit workflows.
- Dirty/unuploaded and pinned files are never auto-evicted.
- Cache-full behavior pauses downloads and blocks/fails new writes safely before data loss.
- Lock conflicts block writes where possible; otherwise user work is preserved as conflict copies.
- Offline mutations record base state and divergent reconnects always create conflicts.
- Symlinks are supported but constrained to authorized roots unless policy allows otherwise.
- Image sequences are normal files in v1 with listing/prefetch optimization, not special version semantics.

### Server/control-plane runtime behavior

The server/control-plane runtime architecture is specified in `docs/architecture/SERVER_ARCHITECTURE.md` and is a product requirement, not an implementation detail.

- The server/control plane lives in the public BiohazardFS repo from the beginning.
- Docker packaging and an in-repo Helm chart are required for self-hosting.
- Start as a modular monolith, not microservices.
- One server codebase can run API, worker, migration, and admin modes.
- PostgreSQL is the metadata/audit/operation source of truth.
- S3-compatible object storage holds content manifests and file data.
- Normal clients never receive permanent storage/database credentials.
- Server issues short-lived scoped transfer authorization.
- Server validates every daemon/client operation and never blindly trusts the daemon.

### Server metadata behavior

The server/control-plane metadata schema is specified in `docs/architecture/METADATA_SCHEMA.md` and is a product requirement, not an implementation detail.

- Schema includes an org/studio boundary from day one.
- Filesystem nodes use stable `node_id` identity; path is derived from mutable parent/name.
- Every committed file write creates an immutable `FileVersion` pointing to a content manifest reference.
- Snapshots support org, project, workset, and subtree scopes.
- Grants can attach to projects, worksets, nodes, and shares.
- Locks attach to node IDs where possible, with path snapshots and provisional IDs for offline-created files.
- Offline operations are first-class server records for replay/reconciliation.
- Deletes use trash records, soft-deleted nodes, and retention/purge policy.
- Audit events use indexed envelope columns plus typed schema-versioned JSON payloads.

### Local daemon behavior

The local daemon contract is specified in `docs/architecture/DAEMON_API.md` and is a product requirement, not an implementation detail.

- Artist installs run `biohazardfsd` as a per-user daemon that auto-starts at login.
- Electron, CLI, MCP, agents, and tests use one local daemon API for mount/cache/filesystem state.
- Platform IPC is the preferred production transport; loopback HTTP is optional for dev/test/integration mode.
- Local daemon auth uses same OS user boundary plus an owner-only local session token.
- The daemon API uses JSON-RPC-like method calls and a standard traceable response envelope.
- The daemon exposes an NDJSON/SSE-style event stream first; bidirectional streaming can be added later.
- Local daemon operational state lives in an owner-only SQLite DB.
- Full optimistic offline mode is required: local changes queue durably and reconcile on reconnect.
- Divergent reconnects always preserve both sides and create conflict records; no silent overwrite.
- Explicit direct-server/headless/admin mode is allowed only for operations that do not require local mount/cache/filesystem state.

### Packaging and release behavior

Packaging and release behavior is specified in `docs/reference/PACKAGING.md` and is a product requirement, not an implementation detail.

- Primary distribution is one platform-native installer per OS.
- The desktop installer installs Biohazard Workspace, `biohazardfs`, `biohazardfsd`, autostart registration, and required platform helpers.
- Public/open-source distribution discipline starts from the beginning.
- Release channels are `dev`, `nightly`, `alpha`, `beta`, and `stable`.
- Use one product version across desktop app, CLI, daemon, and server/control-plane artifacts at first.
- Default uninstall preserves cache, config, credentials, logs, and user data unless the user explicitly chooses purge/remove-data.
- Code signing/notarization is required before serious public/stable distribution, but not required for earliest MVP/dev artifacts.

### Agent-native behavior

Agents are first-class users. The CLI contract is specified in `docs/reference/COMMANDS.md` and is a product requirement, not an implementation detail.

- CLI must be noninteractive-friendly.
- JSON output is the default for every command.
- Every command returns one standard traceable JSON envelope.
- Complex and mutating commands accept canonical JSON payloads; human-friendly flags are aliases.
- Command, event, error, and config schemas are introspectable at runtime.
- Fresh installs default to an `agent-safe` mutation profile until first-run setup chooses a policy.
- First-run setup offers `agent-safe` and `human-friendly` mutation profiles.
- Destructive/admin/data-moving commands require dry-run operation tokens in `agent-safe` mode.
- Large reads use safe defaults, warnings, truncation metadata, and pagination cursors.
- The CLI exposes a stdio MCP surface through `biohazardfs mcp`.
- Supported noninteractive auth methods include env token, credential file, and device enrollment code.
- Agents can administer everything if authorized.
- Agents can impersonate users only with explicit provenance.
- Provenance records whether actions came from UI, CLI, agent, or API.

## 4. Implementation stack

### Rust

Rust owns core correctness:

- filesystem adapters
- daemon
- CLI
- cache state
- transfer state
- credentials/tokens
- lock/conflict logic
- API models

### Electron

Electron owns desktop UX:

- tray/menu bar app
- onboarding
- cache setup
- project/workset browser
- transfer queue
- conflict/problem panels
- settings

Frontend stack:

- React
- TypeScript
- Tailwind CSS
- shadcn/ui primitives/components

### Initial repository shape

```text
crates/
  core/
  api-types/
  cli/
  daemon/
  fuse/
  server/
apps/
  workspace-electron/
  admin-web/
  docs-site/
deploy/
  docker/
  helm/
  compose/
packaging/
docs/
  product/
  architecture/
  reference/
  adr/
  operations/
generated/
tests/
scripts/
skills/
```

The `crates/` subdirectories intentionally avoid repeating the product name; package/crate identifiers may remain `biohazardfs-*` where that helps external publication and dependency clarity.

## 5. Immediate implementation phases

1. Product/architecture ADRs.
2. Rust workspace skeleton.
3. Agent-first CLI contract implementation: standard JSON envelope, schema registry, TOML config, redacted auth status, doctor/smoke, and `biohazardfs mcp` for implemented commands.
4. Daemon API foundation: endpoint discovery, IPC transport, local session token auth, SQLite local state DB, standard envelopes, event stream, mock mount/cache/file methods.
5. Metadata schema foundation: org/users/devices/tokens/projects/worksets/nodes/file versions/grants/operations/audit/locks/conflicts/snapshots/trash.
6. Server architecture foundation: public server crate, modular monolith, serve/worker/migrate/admin modes, Dockerfile, Helm chart skeleton, health/readiness endpoints, config, migrations, object-store validation.
7. Filesystem/cache semantics foundation: path normalization, case-insensitive sibling uniqueness, full-file hydrate, safe dehydrate, durable dirty state, cache-full behavior, lock/conflict mock paths.
8. Packaging foundation: version/channel metadata, one-installer contract, bundled CLI/daemon, per-user autostart, checksum metadata, uninstall preserves cache/config by default.
9. JSON-first CLI skeleton modeled after `~/Nextcloud-CLI`.
10. Read-only Linux FUSE prototype with mock namespace.
11. Hydrate-on-open into local cache from simple HTTP/S3 backend.
12. Cache pin/dehydrate controls.
13. Safe writes and conflict preservation.
14. Electron/shadcn utility shell connected to daemon mock, then real daemon.
15. Windows placeholder spike: Cloud Files API vs WinFsp.
16. macOS placeholder spike: File Provider vs FUSE.

## 6. Reference project conventions

Use `~/Nextcloud-CLI` as a local reference for:

- Rust workspace layout
- reusable library crate + thin CLI crate
- JSON-first command output
- stable JSON error envelope
- `commands schema`
- config/profile/credential abstractions
- smoke commands
- redacted audit behavior
- `AGENTS.md`
- docs/spec-driven implementation
