# BiohazardFS Config Draft

Config should live under platform-specific config dirs and support `--config-dir`/env overrides for agents and tests.

Credentials/tokens should use OS keyring when available, with owner-only local fallback for dev/headless contexts.
