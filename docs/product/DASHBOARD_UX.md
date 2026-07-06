# Biohazard Workspace Dashboard UX Spec

Status: draft product requirement
Primary audience: BiohazardFS implementers and designers
Scope: desktop app onboarding, dashboard information architecture, and LucidLink-parity UX expectations

## 1. Confirmed product decisions

The desktop app is not a single-project dashboard. It is a multi-studio workspace manager for freelancers, artists, admins, and technical users who may be connected to more than one BiohazardFS-compatible studio at the same time.

Confirmed decisions:

1. **Freelancer onboarding is the first dashboard parity target.** The first-run flow should get a nontechnical freelancer from invite to connected, mounted, and ready to find work in roughly one minute when prerequisites are satisfied.
2. **An invite joins a studio/workspace provider, not a fixed project folder.** Project, workset, and folder assignments can change while production is active, so invites should not permanently bind users to one folder path.
3. **BiohazardFS-native permissions are the MVP source of truth.** Kitsu and other production systems can automate permissions later, but the first reliable model is BiohazardFS grants/users/devices/workspaces.
4. **A freelancer can connect to multiple studios at once.** Example: Biohazard VFX, Studio B, and a personal/self-hosted workspace can all be active profiles in one app.
5. **Use one mounted drive/location per studio for MVP.** Projects/worksets/folders appear inside that studio mount according to current permissions.
6. **Access is dynamic.** The mounted namespace and dashboard must refresh when a user's permissions change; users should not need a new invite or reinstall for every project change.
7. **The dashboard must explain current safety.** The app should answer whether the user can work now, where their work is, whether files are synced/cached, and whether any conflict, lock, disk, auth, or connectivity issue needs attention.

## 2. Product model

### 2.1 Connection profile

A connected studio is represented as a local **connection profile**.

Each profile has its own boundary:

- studio/org ID
- display name and icon/initials
- server URL
- authenticated account/user
- registered device ID
- auth token reference, stored securely outside argv/logs
- mount configuration
- cache root and cache policy
- sync state
- permission/access snapshot
- health/error state

Profiles must not casually share auth tokens, cache directories, device registrations, encryption material, or logs containing sensitive paths. Cross-studio isolation is a security and UX requirement.

### 2.2 Studio mount

For MVP, each connected studio gets one mounted drive/location.

Examples:

- Windows: `B:\` for Biohazard VFX, `S:\` for Studio B, or next available letters chosen by the app/user.
- macOS: Finder-visible `Biohazard VFX` location.
- Linux: `~/Biohazard VFX`, `~/Studio B`, or user-selected mount paths.

Inside a studio mount, users see only authorized content:

```text
Biohazard VFX/
  Blood Moon/
  Internal Shared/
  Test Renders/
```

The mount remains stable while visible projects/folders change underneath based on server-side permissions.

### 2.3 Workspaces, filespaces, and wording

Use artist-facing language consistently:

- **Studio**: the organization/provider connection, e.g. Biohazard VFX.
- **Workspace**: a user-visible shared filespace inside a studio, e.g. Blood Moon, Internal Shared, RnD.
- **Biohazard Drive**: informal phrase for the mounted studio location.
- **Make available offline**: primary wording for pin/cache.
- **Remove local copy**: primary wording for dehydrate/uncache.
- **Cache**: acceptable when talking about storage use/settings.
- **Dehydrate/uncache**: advanced/dev wording only.

LucidLink calls these filespaces; Biohazard Workspace may use `Workspace` in the UI while keeping `filespace` as an internal/server term if useful.

## 3. First-run onboarding flow

The first-run flow is not a generic dashboard. It is a focused join flow.

### 3.1 Entry points

Supported entry points should include:

- app opened from an invite/deep link
- pasted invite link/code
- manual `Join a Studio`
- later: create/self-host a new studio/workspace

Example links:

```text
biohazard://join?invite=...
https://biohazardvfx.com/join/...
```

The invite should know the studio/server and enrollment context, but should not need to hard-code a project folder.

### 3.2 Welcome / join studio

The first screen should say what the user is joining.

Example:

```text
Join Biohazard VFX

This will add Biohazard VFX as a connected studio on this computer.
```

Primary actions:

- `Continue`
- `Use a different invite`

The user should not type server URLs, bucket names, database names, mount internals, or object-store details during normal onboarding.

### 3.3 Identify and register device

The app authenticates the user and registers this workstation/device.

Acceptable MVP auth flows include:

- invite code
- device code
- generated user API token
- magic-link style flow later
- Google/SSO later

Visible questions answered:

- Which studio am I joining?
- Who am I signed in as?
- Which device is being registered?

Device registration must be individually revocable by an admin.

### 3.4 Workstation preflight

The app checks prerequisites and shows friendly progress.

Checks:

- daemon installed and running
- platform filesystem support available
- mount can be created
- server reachable
- invite valid
- device registered
- local cache path writable
- enough disk space for default cache policy
- OS permissions are sufficient

User-facing rows:

```text
Checking your workstation...

Biohazard daemon        Ready
Drive support           Ready
Server connection       Ready
Local cache             412 GB available
Permissions             Ready
```

Failures should have one obvious repair action:

- `Fix daemon`
- `Install filesystem support`
- `Choose another cache folder`
- `Retry connection`
- `Contact admin`

### 3.5 Choose mount and cache

Defaults should be preselected and changed only when necessary.

Windows:

- mount: next available drive letter, with a clear display name
- cache: user-local app/cache directory by default

macOS:

- mount: Finder-visible studio location
- cache: app support/cache directory by default

Linux:

- mount: `~/Studio Name` by default, user-selectable
- cache: XDG cache location by default

Primary action:

- `Mount Studio`

Secondary action:

- `Advanced`

Advanced settings may expose drive letter/path, cache path, cache size, proxy/server diagnostics, and logging level.

### 3.6 Load current access

After mounting, the app loads the user's current BiohazardFS-native access.

Example:

```text
Loading your available work...
```

Possible result:

```text
You currently have access to:

- Blood Moon
  - shots/010/comp
  - shots/020/plates
- Internal Shared
- Test Renders
```

Important behavior:

- Loading access does not mean downloading all file content.
- Namespace metadata should appear quickly.
- Optional starter/offline/pinned content can begin caching with honest progress.
- Incomplete uploads must never appear as fully committed complete files.

### 3.7 Success state

The flow ends with a simple success state.

Example:

```text
You're connected.

Biohazard VFX is mounted.
Your current work is available in the Biohazard VFX drive.
```

Primary actions:

- `Open Biohazard VFX`
- `View My Work`
- `Make key files available offline`

If no work is assigned yet:

```text
You're connected to Biohazard VFX, but no work is assigned to you yet.
Ask a producer or admin to grant access, then refresh.
```

Actions:

- `Refresh access`
- `Contact admin`
- `Open Biohazard VFX`

## 4. Dashboard information architecture

### 4.1 Far-left studio rail

The far-left rail switches between connected studios/profiles.

Example:

```text
[BH] Biohazard VFX
[SB] Studio B
[ME] Personal
[+]  Add Studio
```

Each icon should show health at a glance:

- healthy/connected
- degraded/warning
- blocked/error
- disconnected/offline
- attention needed, e.g. conflict or unsynced dirty files

The rail should make it obvious that multiple studios can be connected concurrently.

### 4.2 Selected-studio sidebar

When a studio is selected, the next sidebar scopes navigation to that studio.

Recommended freelancer/default navigation:

```text
Biohazard VFX

My Work
Drive
Transfers
Cache
Conflicts
Activity
Settings
```

Admin-only or permission-gated pages:

```text
Members
Devices
Permissions
Audit
Storage
Server
```

Freelancers should not see admin controls they cannot use. If a read-only admin-adjacent page is useful, it must be clearly labeled as read-only.

### 4.3 Main dashboard / My Work

`My Work` is the default selected-studio landing page for freelancers.

It answers:

1. Can I work right now?
2. Where is my work?
3. Are my files synced?
4. What is available offline?
5. Is anything dangerous happening?

Recommended cards/sections:

- **Work readiness**: Mounted, online/degraded/offline, safe to edit/read-only/sync blocked.
- **Available workspaces**: Current visible projects/workspaces/folders.
- **Recent / assigned folders**: Shortcuts into the mounted drive.
- **Transfer summary**: Uploading, downloading, queued, blocked, failed.
- **Offline/cache summary**: cache used, pinned/offline folders, low disk warnings.
- **Attention required**: conflicts, locks, dirty unsynced files, auth expiry, server unreachable.

Workspace cards should include:

```text
Blood Moon
Compositor access
2 folders available
Mounted in Biohazard VFX

[Open] [Make available offline] [Details]
```

Empty state:

```text
No work assigned yet

You are connected to Biohazard VFX, but no projects or folders are currently shared with you.
Ask a producer/admin to assign access.

[Refresh access]
```

### 4.4 ASCII wireframes

These mockups are intentionally low fidelity. They define hierarchy and required information, not final visual style.

#### First-run invite

```text
+------------------------------------------------------------------+
| Biohazard Workspace                                              |
+------------------------------------------------------------------+
|                                                                  |
|                         Join Biohazard VFX                       |
|                                                                  |
|   This invite will add Biohazard VFX as a connected studio on     |
|   this computer. Your project and folder access can update later. |
|                                                                  |
|   Studio        Biohazard VFX                                    |
|   Server        workspace.biohazardvfx.com                       |
|   Account       Sign in or identify this device                  |
|                                                                  |
|                  [ Continue ]  [ Use different invite ]          |
|                                                                  |
+------------------------------------------------------------------+
```

#### Workstation preflight

```text
+------------------------------------------------------------------+
| Biohazard Workspace                                              |
+------------------------------------------------------------------+
| Checking your workstation...                                     |
|                                                                  |
|   Biohazard daemon          READY                                |
|   Drive support             READY                                |
|   Server connection         READY                                |
|   Local cache               412 GB available                     |
|   Device registration       READY                                |
|                                                                  |
|   Mount                     Biohazard VFX -> B:\                 |
|   Cache                     C:\Users\Avery\Biohazard Cache       |
|                                                                  |
|                         [ Mount Studio ]                         |
|                         [ Advanced ]                             |
+------------------------------------------------------------------+
```

#### Loading current access

```text
+------------------------------------------------------------------+
| Biohazard Workspace                                              |
+------------------------------------------------------------------+
| Loading your available work...                                   |
|                                                                  |
|   Mounting Biohazard VFX drive        DONE                       |
|   Loading folder structure            DONE                       |
|   Refreshing permissions              IN PROGRESS                |
|   Preparing offline starters          QUEUED                     |
|                                                                  |
|   Blood Moon                          shots/010/comp             |
|                                       shots/020/plates           |
|   Internal Shared                     available                  |
|                                                                  |
|             [ Open Biohazard VFX ]  [ View My Work ]             |
+------------------------------------------------------------------+
```

#### Multi-studio dashboard shell

```text
+------+----------------------+---------------------------------------------+
| BH ● | Biohazard VFX        | My Work                                     |
| SB ! |----------------------|---------------------------------------------|
| ME ○ | My Work              | READY TO WORK                               |
|  +   | Drive                | Mounted: B:\Biohazard VFX                  |
|      | Transfers            | Online. Safe to edit. Last sync: 1 min ago |
|      | Cache                |                                             |
|      | Conflicts            | Available work                             |
|      | Activity             | +-----------------------------------------+ |
|      | Settings             | | Blood Moon                              | |
|      |                      | | Compositor access · 2 folders           | |
|      | Admin                | | [Open] [Make available offline] [...]   | |
|      | Members              | +-----------------------------------------+ |
|      | Devices              | +-----------------------------------------+ |
|      | Permissions          | | Internal Shared                         | |
|      |                      | | Read access                             | |
|      |                      | | [Open] [Details]                        | |
|      |                      | +-----------------------------------------+ |
|      |                      |                                             |
|      |                      | Attention                                  |
|      |                      | No conflicts. No blocked uploads.          |
+------+----------------------+---------------------------------------------+
```

#### Problem state

```text
+------+----------------------+---------------------------------------------+
| BH ! | Biohazard VFX        | My Work                                     |
| SB ● |----------------------|---------------------------------------------|
| ME ○ | My Work              | DEGRADED                                    |
|  +   | Drive                | Mounted: B:\Biohazard VFX                  |
|      | Transfers            | Server reachable. Upload blocked.          |
|      | Cache                |                                             |
|      | Conflicts            | Attention required                         |
|      | Activity             | +-----------------------------------------+ |
|      | Settings             | | 3 files waiting to upload               | |
|      |                      | | Low cache space: 8 GB free              | |
|      |                      | | [Choose cache folder] [Retry uploads]   | |
|      |                      | +-----------------------------------------+ |
|      |                      | +-----------------------------------------+ |
|      |                      | | 1 conflict in Blood Moon                | |
|      |                      | | Both versions are preserved             | |
|      |                      | | [Review conflict]                       | |
|      |                      | +-----------------------------------------+ |
+------+----------------------+---------------------------------------------+
```

#### No-work assigned state

```text
+------+----------------------+---------------------------------------------+
| BH ● | Biohazard VFX        | My Work                                     |
|  +   |----------------------|---------------------------------------------|
|      | My Work              | No work assigned yet                        |
|      | Drive                |                                             |
|      | Transfers            | You are connected to Biohazard VFX, but no |
|      | Cache                | projects or folders are currently shared   |
|      | Conflicts            | with you.                                  |
|      | Activity             |                                             |
|      | Settings             | Ask a producer/admin to grant access, then |
|      |                      | refresh.                                   |
|      |                      |                                             |
|      |                      | [ Refresh access ] [ Open Biohazard VFX ]  |
+------+----------------------+---------------------------------------------+
```

## 5. Required dashboard capabilities

### 5.1 Studio connection state

For each studio/profile, the dashboard must show:

- signed-in user
- device name
- auth status
- server reachability
- last successful connection
- permissions last refreshed
- mount status

### 5.2 Mount controls

For each studio/profile:

- mounted/unmounted state
- mount path or drive letter
- `Open in Finder/Explorer/File Manager`
- remount/repair
- unmount, if safe
- explain why mount is unavailable

### 5.3 Work access

The dashboard must show current authorized work:

- visible workspaces/projects/folders
- recently added access
- removed access where useful and safe to show
- refresh access
- no-access empty state

Unauthorized folders must be hidden in the mounted namespace.

### 5.4 Transfers

The dashboard must distinguish:

- uploads
- downloads
- queued work
- blocked work
- failed work
- paused state
- offline state
- last successful sync

Large/in-progress uploads should have honest state. Other collaborators must not mistake incomplete bytes for committed complete versions.

### 5.5 Cache and offline state

The dashboard must support:

- cache location
- cache limit
- used/free cache
- per-studio cache use
- pinned/offline folders
- make available offline
- remove local copy
- low-disk warnings
- cache-full safe handling
- cache move/repair in advanced settings

Dirty/unuploaded files must never be automatically evicted.

### 5.6 Conflicts, locks, and safety

The dashboard must surface:

- unsynced dirty files
- conflicts
- active locks
- lock failures
- offline edits waiting to reconcile
- files blocked by cache-full or auth/server failures
- safe-to-quit/safe-to-unplug state

The app should avoid ambiguous green states. If user work is not durably uploaded or safely queued, the dashboard must say so.

### 5.7 Admin and recovery controls

For users with permission, the dashboard should eventually include:

- members
- groups
- devices
- revocation
- workspace/folder grants
- audit/version history
- trash/restore
- snapshots
- storage/server health

MVP can hide these if not implemented, but the information architecture must leave space for them.

## 6. LucidLink parity implications

For dashboard parity, Biohazard Workspace must behave like a polished front door to a mounted cloud workspace, not like a developer control panel.

Parity expectations:

- The app makes connection/mount/access state obvious.
- A freelancer can add multiple studios and switch between them through a visual rail.
- Each studio has one stable mounted drive/location.
- Current access updates dynamically inside the mounted namespace.
- The app gives simple actions: open, mount, make available offline, remove local copy, refresh, repair.
- Problems are expressed in user outcomes: cannot edit safely, upload blocked, low disk, signed out, server unreachable, conflict needs attention.
- Advanced diagnostics exist, but do not dominate the default freelancer dashboard.

## 7. MVP dashboard acceptance checklist

A first credible dashboard MVP should satisfy this checklist:

- [ ] User can join a studio from an invite/code without typing infrastructure details.
- [ ] User can add at least two studio profiles locally.
- [ ] Far-left rail shows connected studios and health.
- [ ] One studio can be selected and viewed independently from another.
- [ ] Each studio has one configured mount path/drive letter.
- [ ] `My Work` shows current authorized work from BiohazardFS-native permissions.
- [ ] No-work empty state is clear and actionable.
- [ ] User can open the mounted studio drive from the app.
- [ ] User can refresh current access.
- [ ] Dashboard shows mounted/unmounted, online/offline/degraded, and safe-to-edit status.
- [ ] Dashboard shows transfers, cache/offline state, conflicts, locks, and dirty/unsynced warnings.
- [ ] Auth tokens and sensitive server details are not shown casually or passed through argv.
- [ ] Admin-only controls are hidden or read-only for freelancers.

## 8. Non-goals for the first dashboard pass

The first dashboard pass does not need:

- Kitsu-derived permissions as the source of truth.
- One invite per exact project/folder.
- One mount per project/workspace.
- Full admin console parity.
- Advanced storage/database/object-store configuration in normal onboarding.
- Full video streaming/range-read UX beyond honest hydrate/cache status.

## 9. Open questions

These remain product/design questions:

1. Should the UI term be `Workspace`, `Filespace`, or another Biohazard-specific word?
2. What exact default mount names should be used when a freelancer connects to multiple studios with similar names?
3. Should studio icons be uploaded/admin-controlled, generated initials, or both?
4. How should the app present removed access without leaking project names after revocation?
5. Which repair actions can be fully automatic on each platform, and which require installer/admin privileges?
6. What is the minimum offline starter set, if any, after first join?
7. Should a personal/local workspace be built into MVP or deferred?
