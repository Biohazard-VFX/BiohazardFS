# BiohazardFS Configuration Contract

Status: draft reference, partially implemented
Audience: CLI/daemon/server/desktop implementers, operators, automation agents

BiohazardFS uses one shared typed runtime configuration model across the Rust CLI, daemon, server, and desktop app. The current implementation has TOML-backed Rust config loading in `biohazardfs-core::config`, CLI redacted introspection, and server-side redacted introspection via:

```bash
biohazardfs config path
biohazardfs config show --redacted
biohazardfs config validate
biohazardfs-server config
```

The config schema version is:

```text
2026-07-config-v1
```

## Goals

- Keep config consistent across CLI, daemon, desktop, and server.
- Avoid embedding daemon/server defaults separately in each binary.
- Keep secrets out of command-line arguments and logs.
- Make redacted config introspectable for operators and agents.
- Support local development, packaged desktop installs, self-hosted Docker/Helm installs, and CI.
- Keep Kitsu, Nextcloud, storage, and BiohazardFS separately configurable.

## Precedence

Target precedence, highest first:

1. explicit safe CLI flags, for non-secret process options only
2. environment variables
3. selected profile in config file
4. built-in defaults

Secrets must not be accepted through argv. Secrets may come from environment variables in CI/headless deployments, OS keychain/credential stores in desktop installs, or owner-only local fallback files when explicitly supported.

## Config files

Target default paths:

| Platform | Config file |
| --- | --- |
| Linux | `$XDG_CONFIG_HOME/biohazardfs/config.toml` or `~/.config/biohazardfs/config.toml` |
| macOS | `~/Library/Application Support/BiohazardFS/config.toml` |
| Windows | `%APPDATA%\\BiohazardFS\\config.toml` |

Overrides:

| Env var | Purpose |
| --- | --- |
| `BIOHAZARDFS_CONFIG_FILE` | explicit config file path |
| `BIOHAZARDFS_CONFIG_DIR` | explicit config directory for tests/agents/headless installs |
| `BIOHAZARDFS_PROFILE` | profile name, default `dev` |

The Rust scaffold now parses TOML config files. Missing default config files are allowed and produce built-in defaults. Invalid TOML or unreadable explicit config files return `config_parse_error` / `config_read_error` through the CLI command envelope.

## Shared environment variables

| Env var | Type | Secret? | Applies to | Notes |
| --- | --- | --- | --- | --- |
| `BIOHAZARDFS_PROFILE` | string | no | all | selected runtime profile, default `dev` |
| `BIOHAZARDFS_LOG` | string | no | all | log level/filter, default `info` |
| `BIOHAZARDFS_CONFIG_FILE` | path | no | all | explicit config file |
| `BIOHAZARDFS_CONFIG_DIR` | path | no | all | explicit config dir |
| `BIOHAZARDFS_LOCAL_TOKEN` | string | yes | daemon/CLI/desktop | local owner token; never pass via argv |
| `BIOHAZARDFS_SERVER_BIND` | host:port | no | server | default `127.0.0.1:8080`; containers set `0.0.0.0:8080` |
| `BIOHAZARDFS_SERVER_PUBLIC_URL` | URL | no | server/clients | externally visible server URL |
| `BIOHAZARDFS_DATABASE_URL` | URL | yes-ish | server | Postgres connection URL; redact in output |
| `BIOHAZARDFS_OBJECT_STORE_PROVIDER` | enum/string | no | server | default `rustfs` |
| `BIOHAZARDFS_OBJECT_STORE_ENDPOINT` | URL | no | server | S3-compatible endpoint |
| `BIOHAZARDFS_OBJECT_STORE_BUCKET` | string | no | server | content bucket |
| `BIOHAZARDFS_OBJECT_STORE_REGION` | string | no | server | optional S3 region |
| `BIOHAZARDFS_OBJECT_STORE_ACCESS_KEY_ID` | string | sensitive | server | access key ID; report only whether set |
| `BIOHAZARDFS_OBJECT_STORE_SECRET_ACCESS_KEY` | string | yes | server | secret key; always redacted |

## Canonical object store

RustFS is the canonical BiohazardFS development and self-hosted object-store default.

Product docs may say “S3-compatible object storage” because BiohazardFS should work with RustFS, MinIO, AWS S3, Cloudflare R2, and other compatible systems over time. But repository examples should dogfood RustFS unless a compatibility test explicitly targets another provider.

Default dev Compose service:

```yaml
object-store:
  image: rustfs/rustfs:1.0.0-beta.8
  command: ["--address", ":9000", "--console-enable", "/data"]
```

Provider rules:

- Default provider is `rustfs`.
- Non-RustFS providers are allowed for compatibility but should produce an informational warning in config validation.
- Secret values must not be serialized in config/status output.
- Bucket existence and credentials validation are not implemented yet; current server health only reports whether config appears present.

## Daemon config contract

Production daemon transport remains platform IPC. Loopback HTTP is only for development and tests.

Current shared fields:

| Field | Default |
| --- | --- |
| `daemon.transport` | `platform_ipc` |
| `daemon.dev_loopback_http_endpoint` | `127.0.0.1:47666` |
| `daemon.local_token_set` | derived from `BIOHAZARDFS_LOCAL_TOKEN` |

Rules:

- `BIOHAZARDFS_LOCAL_TOKEN` is the only current local token source for CLI/daemon smoke tests.
- Local token must not be accepted through argv.
- Daemon RPC stays separate from CLI config: daemon envelopes use `method`; CLI output uses `command`.

## TOML shape

Top-level fields apply to every profile. `[profiles.<name>]` fields override top-level fields for the selected profile. Environment variables override both.

```toml
schema_version = "2026-07-config-v1"
profile = "dev"
log = "info"

[server]
bind = "127.0.0.1:8080"
public_url = "http://localhost:8080"

[database]
url = "postgres://user:password@postgres:5432/biohazardfs"

[object_store]
provider = "rustfs"
endpoint = "http://object-store:9000"
bucket = "biohazardfs-dev"
region = "us-east-1"
access_key_id = "biohazardfs"
secret_access_key = "dev-only-secret"

[profiles.ci]
log = "debug"

[profiles.ci.server]
bind = "0.0.0.0:8080"
```

The redacted runtime shape reports secret-bearing values as booleans or `***REDACTED***`:

```json
{
  "schema_version": "2026-07-config-v1",
  "database": { "url_set": true },
  "object_store": {
    "provider": "rustfs",
    "access_key_id_set": true,
    "secret_access_key": "***REDACTED***"
  }
}
```

`biohazardfs config show --redacted` and `biohazardfs-server config` print safe JSON envelopes. It is safe for CI logs as long as new secret-bearing fields use the redaction type or boolean `*_set` convention.

## Validation warnings

The shared config scaffold currently emits warnings for:

- non-default object-store provider (`non_default_object_store`)
- object-store endpoint set without a bucket (`object_store_bucket_missing`)
- access key ID set without secret access key (`object_store_secret_missing`)

These are warnings for now because the server scaffold does not yet connect to Postgres or object storage. They should become stricter in install/doctor paths once real dependency checks exist.

## Next implementation steps

1. Add config file writing and controlled `config get/set` commands.
2. Generate a JSON schema for `2026-07-config-v1` under `generated/schemas/`.
3. Mirror the shared config shape in the Electron TypeScript preload/renderer boundary.
4. Add secure credential lookup: OS keyring first, owner-only fallback for headless/dev.
5. Add server dependency checks that validate Postgres connectivity and RustFS bucket access.
6. Add Compose integration smoke that boots RustFS and creates/checks the dev bucket.
