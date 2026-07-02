use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const CONFIG_SCHEMA_VERSION: &str = "2026-07-config-v1";
pub const DEFAULT_PROFILE: &str = "dev";
pub const DEFAULT_LOG: &str = "info";
pub const DEFAULT_SERVER_BIND: &str = "127.0.0.1:8080";
pub const DEFAULT_SERVER_PUBLIC_URL: &str = "http://localhost:8080";
pub const DEFAULT_DAEMON_TRANSPORT: &str = "platform_ipc";
pub const DEFAULT_OBJECT_STORE_PROVIDER: &str = "rustfs";

pub const ENV_PROFILE: &str = "BIOHAZARDFS_PROFILE";
pub const ENV_LOG: &str = "BIOHAZARDFS_LOG";
pub const ENV_CONFIG_FILE: &str = "BIOHAZARDFS_CONFIG_FILE";
pub const ENV_CONFIG_DIR: &str = "BIOHAZARDFS_CONFIG_DIR";
pub const ENV_SERVER_BIND: &str = "BIOHAZARDFS_SERVER_BIND";
pub const ENV_SERVER_PUBLIC_URL: &str = "BIOHAZARDFS_SERVER_PUBLIC_URL";
pub const ENV_DATABASE_URL: &str = "BIOHAZARDFS_DATABASE_URL";
pub const ENV_OBJECT_STORE_PROVIDER: &str = "BIOHAZARDFS_OBJECT_STORE_PROVIDER";
pub const ENV_OBJECT_STORE_ENDPOINT: &str = "BIOHAZARDFS_OBJECT_STORE_ENDPOINT";
pub const ENV_OBJECT_STORE_BUCKET: &str = "BIOHAZARDFS_OBJECT_STORE_BUCKET";
pub const ENV_OBJECT_STORE_REGION: &str = "BIOHAZARDFS_OBJECT_STORE_REGION";
pub const ENV_OBJECT_STORE_ACCESS_KEY_ID: &str = "BIOHAZARDFS_OBJECT_STORE_ACCESS_KEY_ID";
pub const ENV_OBJECT_STORE_SECRET_ACCESS_KEY: &str = "BIOHAZARDFS_OBJECT_STORE_SECRET_ACCESS_KEY";
pub const ENV_LOCAL_TOKEN: &str = "BIOHAZARDFS_LOCAL_TOKEN";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub schema_version: String,
    pub profile: String,
    pub log: String,
    pub paths: ConfigPaths,
    pub daemon: DaemonConfig,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub object_store: ObjectStoreConfig,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let config_file = non_empty(lookup(ENV_CONFIG_FILE));
        let config_dir = non_empty(lookup(ENV_CONFIG_DIR));
        let local_token = non_empty(lookup(ENV_LOCAL_TOKEN));
        let object_store_secret = non_empty(lookup(ENV_OBJECT_STORE_SECRET_ACCESS_KEY));

        Self {
            schema_version: CONFIG_SCHEMA_VERSION.to_string(),
            profile: non_empty(lookup(ENV_PROFILE)).unwrap_or_else(|| DEFAULT_PROFILE.to_string()),
            log: non_empty(lookup(ENV_LOG)).unwrap_or_else(|| DEFAULT_LOG.to_string()),
            paths: ConfigPaths {
                config_file,
                config_dir,
            },
            daemon: DaemonConfig {
                transport: DEFAULT_DAEMON_TRANSPORT.to_string(),
                dev_loopback_http_endpoint: biohazardfs_api_types::DEV_LOOPBACK_HTTP_ENDPOINT
                    .to_string(),
                local_token_set: local_token.is_some(),
            },
            server: ServerConfig {
                bind: non_empty(lookup(ENV_SERVER_BIND))
                    .unwrap_or_else(|| DEFAULT_SERVER_BIND.to_string()),
                public_url: non_empty(lookup(ENV_SERVER_PUBLIC_URL))
                    .unwrap_or_else(|| DEFAULT_SERVER_PUBLIC_URL.to_string()),
            },
            database: DatabaseConfig {
                url_set: non_empty(lookup(ENV_DATABASE_URL)).is_some(),
            },
            object_store: ObjectStoreConfig {
                provider: non_empty(lookup(ENV_OBJECT_STORE_PROVIDER))
                    .unwrap_or_else(|| DEFAULT_OBJECT_STORE_PROVIDER.to_string()),
                endpoint: non_empty(lookup(ENV_OBJECT_STORE_ENDPOINT)),
                bucket: non_empty(lookup(ENV_OBJECT_STORE_BUCKET)),
                region: non_empty(lookup(ENV_OBJECT_STORE_REGION)),
                access_key_id_set: non_empty(lookup(ENV_OBJECT_STORE_ACCESS_KEY_ID)).is_some(),
                secret_access_key: object_store_secret.map(RedactedSecret::new),
            },
        }
    }

    pub fn validation_warnings(&self) -> Vec<ConfigWarning> {
        let mut warnings = Vec::new();

        if self.object_store.provider != DEFAULT_OBJECT_STORE_PROVIDER {
            warnings.push(ConfigWarning {
                code: "non_default_object_store".to_string(),
                message: format!(
                    "object-store provider is {}; RustFS is the BiohazardFS default",
                    self.object_store.provider
                ),
            });
        }

        if self.object_store.endpoint.is_some() && self.object_store.bucket.is_none() {
            warnings.push(ConfigWarning {
                code: "object_store_bucket_missing".to_string(),
                message: format!(
                    "{} is set but {} is missing",
                    ENV_OBJECT_STORE_ENDPOINT, ENV_OBJECT_STORE_BUCKET
                ),
            });
        }

        if self.object_store.access_key_id_set && self.object_store.secret_access_key.is_none() {
            warnings.push(ConfigWarning {
                code: "object_store_secret_missing".to_string(),
                message: format!(
                    "{} is set but {} is missing",
                    ENV_OBJECT_STORE_ACCESS_KEY_ID, ENV_OBJECT_STORE_SECRET_ACCESS_KEY
                ),
            });
        }

        warnings
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigPaths {
    pub config_file: Option<String>,
    pub config_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonConfig {
    pub transport: String,
    pub dev_loopback_http_endpoint: String,
    pub local_token_set: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerConfig {
    pub bind: String,
    pub public_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseConfig {
    pub url_set: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectStoreConfig {
    pub provider: String,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub access_key_id_set: bool,
    pub secret_access_key: Option<RedactedSecret>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RedactedSecret(String);

impl RedactedSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn expose_for_process_boundary(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RedactedSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RedactedSecret(***REDACTED***)")
    }
}

impl Serialize for RedactedSecret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("***REDACTED***")
    }
}

impl<'de> Deserialize<'de> for RedactedSecret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigWarning {
    pub code: String,
    pub message: String,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(
        values: &'static [(&'static str, &'static str)],
    ) -> impl FnMut(&str) -> Option<String> {
        move |key| {
            values
                .iter()
                .find_map(|(candidate, value)| (*candidate == key).then(|| (*value).to_string()))
        }
    }

    #[test]
    fn defaults_to_rustfs_and_safe_loopback_bind() {
        let config = RuntimeConfig::from_lookup(|_| None);
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(config.server.bind, DEFAULT_SERVER_BIND);
        assert_eq!(config.object_store.provider, "rustfs");
        assert!(config.object_store.endpoint.is_none());
        assert!(!config.daemon.local_token_set);
    }

    #[test]
    fn reads_shared_environment_contract() {
        let config = RuntimeConfig::from_lookup(lookup(&[
            (ENV_PROFILE, "ci"),
            (ENV_LOG, "debug"),
            (ENV_SERVER_BIND, "0.0.0.0:8080"),
            (ENV_SERVER_PUBLIC_URL, "https://biohazardfs.example"),
            (ENV_DATABASE_URL, "postgres://example"),
            (ENV_OBJECT_STORE_ENDPOINT, "http://object-store:9000"),
            (ENV_OBJECT_STORE_BUCKET, "biohazardfs-dev"),
            (ENV_OBJECT_STORE_ACCESS_KEY_ID, "biohazardfs"),
            (ENV_OBJECT_STORE_SECRET_ACCESS_KEY, "super-secret"),
            (ENV_LOCAL_TOKEN, "local-token"),
        ]));

        assert_eq!(config.profile, "ci");
        assert_eq!(config.log, "debug");
        assert_eq!(config.server.bind, "0.0.0.0:8080");
        assert_eq!(config.server.public_url, "https://biohazardfs.example");
        assert!(config.database.url_set);
        assert_eq!(config.object_store.provider, "rustfs");
        assert_eq!(
            config.object_store.endpoint.as_deref(),
            Some("http://object-store:9000")
        );
        assert_eq!(
            config.object_store.bucket.as_deref(),
            Some("biohazardfs-dev")
        );
        assert!(config.object_store.access_key_id_set);
        assert!(config.object_store.secret_access_key.is_some());
        assert!(config.daemon.local_token_set);
    }

    #[test]
    fn redacts_secret_during_serialization() {
        let config = RuntimeConfig::from_lookup(lookup(&[(
            ENV_OBJECT_STORE_SECRET_ACCESS_KEY,
            "do-not-print",
        )]));
        let json = serde_json::to_string(&config).expect("config serializes");
        assert!(json.contains("***REDACTED***"));
        assert!(!json.contains("do-not-print"));
    }

    #[test]
    fn warns_when_object_store_provider_is_not_rustfs() {
        let config = RuntimeConfig::from_lookup(lookup(&[(ENV_OBJECT_STORE_PROVIDER, "minio")]));
        let warnings = config.validation_warnings();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.code == "non_default_object_store")
        );
    }
}
