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

### Agent-native behavior

Agents are first-class users.

- CLI must be noninteractive-friendly.
- JSON output by default.
- Destructive/cloud-mutating commands require dry-run/yes guardrails.
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
  biohazardfs-core/
  biohazardfs-cli/
  biohazardfs-daemon/
  biohazardfs-api-types/
  biohazardfs-fuse/
apps/
  workspace-electron/
docs/
  SPEC.md
  COMMANDS.md
  ARCHITECTURE.md
  CONFIG.md
  SECURITY.md
  ROADMAP.md
  SMOKE.md
```

## 5. Immediate implementation phases

1. Product/architecture ADRs.
2. Rust workspace skeleton.
3. JSON-first CLI skeleton modeled after `~/Nextcloud-CLI`.
4. Read-only Linux FUSE prototype with mock namespace.
5. Hydrate-on-open into local cache from simple HTTP/S3 backend.
6. Cache pin/dehydrate controls.
7. Safe writes and conflict preservation.
8. Electron/shadcn utility shell connected to daemon mock, then real daemon.
9. Windows placeholder spike: Cloud Files API vs WinFsp.
10. macOS placeholder spike: File Provider vs FUSE.

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
