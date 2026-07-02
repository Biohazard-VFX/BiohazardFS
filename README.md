# BiohazardFS

BiohazardFS is an open-source LucidLink-style virtual sync filesystem for VFX production.

It is being built for Biohazard first, then intended to become a public, self-hostable product for other studios.

## Product goal

Install the app, paste an invite code or sign in, choose a cache drive, and get a mounted Biohazard workspace with virtual files, native file states, local caching, pinning, safe writes, and project-aware permissions.

## Architecture direction

- Rust core, daemon, CLI, and filesystem adapters
- Electron desktop app
- React + TypeScript + Tailwind + shadcn/ui primitives
- S3-compatible object storage backend
- PostgreSQL control/metadata database
- Optional Kitsu integration for assignments/worksets
- Agent-native JSON CLI from day one

## Repository status

Planning/scaffolding. See `docs/SPEC.md`.
