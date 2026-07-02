# BiohazardFS Architecture Draft

```text
Electron Workspace UI
  ↓ local IPC/API
Rust daemon: biohazardfsd
  ↓
Filesystem adapter: FUSE / WinFsp / Cloud Files / File Provider
  ↓
Local cache + state DB
  ↓
BiohazardFS control plane
  ↓
S3-compatible object storage + PostgreSQL metadata
```

Electron is a shell. Rust is the sync/filesystem engine.
