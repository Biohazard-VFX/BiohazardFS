-- Ensure bearer credentials resolve to exactly one server identity.
-- Raw token values are never stored; this unique index applies to token hashes.

CREATE UNIQUE INDEX tokens_secret_hash_unique
    ON tokens (secret_hash);
