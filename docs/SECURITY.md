# BiohazardFS Security Draft

- Do not expose permanent S3/Postgres credentials to normal artist clients.
- Prefer invite/device tokens and short-lived transfer authorization.
- Devices must be revocable.
- Audit provenance for UI, CLI, agent, and API actions.
- Preserve conflicting versions; never silently overwrite.
