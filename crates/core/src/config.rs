use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigLoadOptions {
    pub config_file: Option<PathBuf>,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadedConfig {
    pub config: RuntimeConfig,
    pub config_file_path: String,
    pub config_file_exists: bool,
    pub selected_profile: String,
    pub warnings: Vec<ConfigWarning>,
}

impl LoadedConfig {
    pub fn validation_warnings(&self) -> Vec<ConfigWarning> {
        let mut warnings = self.warnings.clone();
        warnings.extend(self.config.validation_warnings());
        warnings
    }
}

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

    pub fn load(options: ConfigLoadOptions) -> Result<LoadedConfig, ConfigError> {
        Self::load_with_lookup(
            options,
            |key| std::env::var(key).ok(),
            |path| std::fs::read_to_string(path),
        )
    }

    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let selected_profile =
            non_empty(lookup(ENV_PROFILE)).unwrap_or_else(|| DEFAULT_PROFILE.to_string());
        let mut config = Self::defaults(selected_profile);
        apply_env_overrides(&mut config, lookup);
        config
    }

    fn load_with_lookup(
        options: ConfigLoadOptions,
        mut lookup: impl FnMut(&str) -> Option<String>,
        mut read_to_string: impl FnMut(&PathBuf) -> std::io::Result<String>,
    ) -> Result<LoadedConfig, ConfigError> {
        let option_config_file = options.config_file.clone();
        let env_config_file = non_empty(lookup(ENV_CONFIG_FILE)).map(PathBuf::from);
        let env_config_dir = non_empty(lookup(ENV_CONFIG_DIR)).map(PathBuf::from);
        let explicit_config_file =
            option_config_file.is_some() || env_config_file.is_some() || env_config_dir.is_some();
        let config_file_path = resolve_config_file_path_from_parts(
            option_config_file,
            env_config_file,
            env_config_dir,
        );
        let config_file_path_string = config_file_path.to_string_lossy().to_string();

        let document_text = match read_to_string(&config_file_path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit_config_file => {
                None
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::new(
                    "config_not_found",
                    format!("explicit config file was not found: {config_file_path_string}"),
                ));
            }
            Err(error) => {
                return Err(ConfigError::new(
                    "config_read_error",
                    format!("failed to read config file {config_file_path_string}: {error}"),
                ));
            }
        };

        let document = document_text
            .as_deref()
            .map(parse_config_document)
            .transpose()?;

        let env_profile = non_empty(lookup(ENV_PROFILE));
        let selected_profile = options
            .profile
            .and_then(|profile| non_empty(Some(profile)))
            .or(env_profile)
            .or_else(|| document.as_ref().and_then(|doc| doc.profile.clone()))
            .unwrap_or_else(|| DEFAULT_PROFILE.to_string());

        let mut config = Self::defaults(selected_profile.clone());
        config.paths = ConfigPaths {
            config_file: Some(config_file_path_string.clone()),
            config_dir: config_file_path
                .parent()
                .map(|path| path.to_string_lossy().to_string()),
        };

        let mut warnings = Vec::new();
        if let Some(document) = document {
            apply_document(&mut config, &document, &selected_profile, &mut warnings);
        }
        apply_env_overrides(&mut config, lookup);
        config.profile = selected_profile.clone();
        config.paths.config_file = Some(config_file_path_string.clone());
        config.paths.config_dir = config_file_path
            .parent()
            .map(|path| path.to_string_lossy().to_string());

        Ok(LoadedConfig {
            config,
            config_file_path: config_file_path_string,
            config_file_exists: document_text.is_some(),
            selected_profile,
            warnings,
        })
    }

    fn defaults(profile: String) -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION.to_string(),
            profile,
            log: DEFAULT_LOG.to_string(),
            paths: ConfigPaths {
                config_file: None,
                config_dir: None,
            },
            daemon: DaemonConfig {
                transport: DEFAULT_DAEMON_TRANSPORT.to_string(),
                dev_loopback_http_endpoint: biohazardfs_api_types::DEV_LOOPBACK_HTTP_ENDPOINT
                    .to_string(),
                local_token_set: false,
            },
            server: ServerConfig {
                bind: DEFAULT_SERVER_BIND.to_string(),
                public_url: DEFAULT_SERVER_PUBLIC_URL.to_string(),
            },
            database: DatabaseConfig { url_set: false },
            object_store: ObjectStoreConfig {
                provider: DEFAULT_OBJECT_STORE_PROVIDER.to_string(),
                endpoint: None,
                bucket: None,
                region: None,
                access_key_id_set: false,
                secret_access_key: None,
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

impl fmt::Debug for RedactedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub code: String,
    pub message: String,
}

impl ConfigError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct ConfigDocument {
    schema_version: Option<String>,
    profile: Option<String>,
    log: Option<String>,
    daemon: Option<DaemonDocument>,
    server: Option<ServerDocument>,
    database: Option<DatabaseDocument>,
    object_store: Option<ObjectStoreDocument>,
    profiles: BTreeMap<String, ProfileDocument>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct ProfileDocument {
    log: Option<String>,
    daemon: Option<DaemonDocument>,
    server: Option<ServerDocument>,
    database: Option<DatabaseDocument>,
    object_store: Option<ObjectStoreDocument>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct DaemonDocument {
    transport: Option<String>,
    dev_loopback_http_endpoint: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct ServerDocument {
    bind: Option<String>,
    public_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct DatabaseDocument {
    url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct ObjectStoreDocument {
    provider: Option<String>,
    endpoint: Option<String>,
    bucket: Option<String>,
    region: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
}

pub fn resolve_config_file_path(options: &ConfigLoadOptions) -> PathBuf {
    let env_config_file = std::env::var(ENV_CONFIG_FILE)
        .ok()
        .and_then(|value| non_empty(Some(value)))
        .map(PathBuf::from);
    let env_config_dir = std::env::var(ENV_CONFIG_DIR)
        .ok()
        .and_then(|value| non_empty(Some(value)))
        .map(PathBuf::from);
    resolve_config_file_path_from_parts(
        options.config_file.clone(),
        env_config_file,
        env_config_dir,
    )
}

pub fn default_config_file_path() -> PathBuf {
    default_config_dir().join("config.toml")
}

pub fn default_config_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("BiohazardFS")
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library")
            .join("Application Support")
            .join("BiohazardFS")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("biohazardfs")
    }
}

fn resolve_config_file_path_from_parts(
    option_config_file: Option<PathBuf>,
    env_config_file: Option<PathBuf>,
    env_config_dir: Option<PathBuf>,
) -> PathBuf {
    option_config_file
        .or(env_config_file)
        .or_else(|| env_config_dir.map(|dir| dir.join("config.toml")))
        .unwrap_or_else(default_config_file_path)
}

fn parse_config_document(text: &str) -> Result<ConfigDocument, ConfigError> {
    toml::from_str::<ConfigDocument>(text).map_err(|error| {
        let location = error
            .span()
            .map(|span| line_column(text, span.start))
            .map(|(line, column)| format!(" at line {line}, column {column}"))
            .unwrap_or_default();
        ConfigError::new(
            "config_parse_error",
            format!("failed to parse BiohazardFS TOML config{location}; source text is omitted"),
        )
    })
}

fn line_column(input: &str, byte_index: usize) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;
    for (index, character) in input.char_indices() {
        if index >= byte_index {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn apply_document(
    config: &mut RuntimeConfig,
    document: &ConfigDocument,
    selected_profile: &str,
    warnings: &mut Vec<ConfigWarning>,
) {
    if let Some(schema_version) = document.schema_version.as_deref().and_then(non_empty_str)
        && schema_version != CONFIG_SCHEMA_VERSION
    {
        warnings.push(ConfigWarning {
            code: "config_schema_version_mismatch".to_string(),
            message: format!(
                "config schema_version is {schema_version}; expected {CONFIG_SCHEMA_VERSION}"
            ),
        });
    }

    apply_root_fragment(config, document);
    if let Some(profile) = document.profiles.get(selected_profile) {
        apply_profile_fragment(config, profile);
    } else if !document.profiles.is_empty() && selected_profile != DEFAULT_PROFILE {
        warnings.push(ConfigWarning {
            code: "config_profile_missing".to_string(),
            message: format!("selected profile {selected_profile} was not found in config file"),
        });
    }
}

fn apply_root_fragment(config: &mut RuntimeConfig, document: &ConfigDocument) {
    if let Some(log) = document.log.as_deref().and_then(non_empty_str) {
        config.log = log.to_string();
    }
    if let Some(daemon) = &document.daemon {
        apply_daemon(config, daemon);
    }
    if let Some(server) = &document.server {
        apply_server(config, server);
    }
    if let Some(database) = &document.database {
        apply_database(config, database);
    }
    if let Some(object_store) = &document.object_store {
        apply_object_store(config, object_store);
    }
}

fn apply_profile_fragment(config: &mut RuntimeConfig, profile: &ProfileDocument) {
    if let Some(log) = profile.log.as_deref().and_then(non_empty_str) {
        config.log = log.to_string();
    }
    if let Some(daemon) = &profile.daemon {
        apply_daemon(config, daemon);
    }
    if let Some(server) = &profile.server {
        apply_server(config, server);
    }
    if let Some(database) = &profile.database {
        apply_database(config, database);
    }
    if let Some(object_store) = &profile.object_store {
        apply_object_store(config, object_store);
    }
}

fn apply_daemon(config: &mut RuntimeConfig, daemon: &DaemonDocument) {
    if let Some(transport) = daemon.transport.as_deref().and_then(non_empty_str) {
        config.daemon.transport = transport.to_string();
    }
    if let Some(endpoint) = daemon
        .dev_loopback_http_endpoint
        .as_deref()
        .and_then(non_empty_str)
    {
        config.daemon.dev_loopback_http_endpoint = endpoint.to_string();
    }
}

fn apply_server(config: &mut RuntimeConfig, server: &ServerDocument) {
    if let Some(bind) = server.bind.as_deref().and_then(non_empty_str) {
        config.server.bind = bind.to_string();
    }
    if let Some(public_url) = server.public_url.as_deref().and_then(non_empty_str) {
        config.server.public_url = public_url.to_string();
    }
}

fn apply_database(config: &mut RuntimeConfig, database: &DatabaseDocument) {
    if database.url.as_deref().and_then(non_empty_str).is_some() {
        config.database.url_set = true;
    }
}

fn apply_object_store(config: &mut RuntimeConfig, object_store: &ObjectStoreDocument) {
    if let Some(provider) = object_store.provider.as_deref().and_then(non_empty_str) {
        config.object_store.provider = provider.to_string();
    }
    if let Some(endpoint) = object_store.endpoint.as_deref().and_then(non_empty_str) {
        config.object_store.endpoint = Some(endpoint.to_string());
    }
    if let Some(bucket) = object_store.bucket.as_deref().and_then(non_empty_str) {
        config.object_store.bucket = Some(bucket.to_string());
    }
    if let Some(region) = object_store.region.as_deref().and_then(non_empty_str) {
        config.object_store.region = Some(region.to_string());
    }
    if object_store
        .access_key_id
        .as_deref()
        .and_then(non_empty_str)
        .is_some()
    {
        config.object_store.access_key_id_set = true;
    }
    if let Some(secret) = object_store
        .secret_access_key
        .as_deref()
        .and_then(non_empty_str)
    {
        config.object_store.secret_access_key = Some(RedactedSecret::new(secret));
    }
}

fn apply_env_overrides(config: &mut RuntimeConfig, mut lookup: impl FnMut(&str) -> Option<String>) {
    if let Some(profile) = non_empty(lookup(ENV_PROFILE)) {
        config.profile = profile;
    }
    if let Some(log) = non_empty(lookup(ENV_LOG)) {
        config.log = log;
    }
    if let Some(config_file) = non_empty(lookup(ENV_CONFIG_FILE)) {
        config.paths.config_file = Some(config_file);
    }
    if let Some(config_dir) = non_empty(lookup(ENV_CONFIG_DIR)) {
        config.paths.config_dir = Some(config_dir);
    }
    if non_empty(lookup(ENV_LOCAL_TOKEN)).is_some() {
        config.daemon.local_token_set = true;
    }
    if let Some(bind) = non_empty(lookup(ENV_SERVER_BIND)) {
        config.server.bind = bind;
    }
    if let Some(public_url) = non_empty(lookup(ENV_SERVER_PUBLIC_URL)) {
        config.server.public_url = public_url;
    }
    if non_empty(lookup(ENV_DATABASE_URL)).is_some() {
        config.database.url_set = true;
    }
    if let Some(provider) = non_empty(lookup(ENV_OBJECT_STORE_PROVIDER)) {
        config.object_store.provider = provider;
    }
    if let Some(endpoint) = non_empty(lookup(ENV_OBJECT_STORE_ENDPOINT)) {
        config.object_store.endpoint = Some(endpoint);
    }
    if let Some(bucket) = non_empty(lookup(ENV_OBJECT_STORE_BUCKET)) {
        config.object_store.bucket = Some(bucket);
    }
    if let Some(region) = non_empty(lookup(ENV_OBJECT_STORE_REGION)) {
        config.object_store.region = Some(region);
    }
    if non_empty(lookup(ENV_OBJECT_STORE_ACCESS_KEY_ID)).is_some() {
        config.object_store.access_key_id_set = true;
    }
    if let Some(secret) = non_empty(lookup(ENV_OBJECT_STORE_SECRET_ACCESS_KEY)) {
        config.object_store.secret_access_key = Some(RedactedSecret::new(secret));
    }
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

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
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
    fn loads_toml_profile_and_redacts_secrets() {
        let toml = r#"
schema_version = "2026-07-config-v1"
profile = "studio"
log = "info"

[server]
bind = "127.0.0.1:9000"
public_url = "http://localhost:9000"

[object_store]
provider = "rustfs"
endpoint = "http://root-store:9000"
bucket = "root-bucket"

[profiles.studio]
log = "debug"

[profiles.studio.server]
bind = "0.0.0.0:8080"

[profiles.studio.database]
url = "postgres://secret"

[profiles.studio.object_store]
bucket = "studio-bucket"
access_key_id = "access"
secret_access_key = "do-not-print"
"#;
        let loaded = RuntimeConfig::load_with_lookup(
            ConfigLoadOptions {
                config_file: Some(PathBuf::from("/tmp/biohazardfs-test.toml")),
                profile: None,
            },
            |_| None,
            |_| Ok(toml.to_string()),
        )
        .expect("config loads");

        assert!(loaded.config_file_exists);
        assert_eq!(loaded.selected_profile, "studio");
        assert_eq!(loaded.config.log, "debug");
        assert_eq!(loaded.config.server.bind, "0.0.0.0:8080");
        assert_eq!(
            loaded.config.object_store.bucket.as_deref(),
            Some("studio-bucket")
        );
        assert!(loaded.config.database.url_set);
        let json = serde_json::to_string(&loaded).expect("config serializes");
        assert!(json.contains("***REDACTED***"));
        assert!(!json.contains("do-not-print"));
        assert!(!json.contains("postgres://secret"));
    }

    #[test]
    fn cli_profile_overrides_env_and_file_profile() {
        let toml = r#"
profile = "file"

[profiles.file.server]
bind = "127.0.0.1:1111"

[profiles.cli.server]
bind = "127.0.0.1:3333"
"#;
        let loaded = RuntimeConfig::load_with_lookup(
            ConfigLoadOptions {
                config_file: Some(PathBuf::from("/tmp/biohazardfs-test.toml")),
                profile: Some("cli".to_string()),
            },
            lookup(&[(ENV_PROFILE, "env")]),
            |_| Ok(toml.to_string()),
        )
        .expect("config loads");

        assert_eq!(loaded.selected_profile, "cli");
        assert_eq!(loaded.config.server.bind, "127.0.0.1:3333");
    }

    #[test]
    fn env_values_override_toml_profile_values() {
        let toml = r#"
profile = "dev"

[profiles.dev.server]
bind = "127.0.0.1:1111"
"#;
        let loaded = RuntimeConfig::load_with_lookup(
            ConfigLoadOptions {
                config_file: Some(PathBuf::from("/tmp/biohazardfs-test.toml")),
                profile: None,
            },
            lookup(&[(ENV_SERVER_BIND, "127.0.0.1:2222")]),
            |_| Ok(toml.to_string()),
        )
        .expect("config loads");

        assert_eq!(loaded.config.server.bind, "127.0.0.1:2222");
    }

    #[test]
    fn missing_profile_is_a_warning() {
        let toml = r#"
[profiles.dev.server]
bind = "127.0.0.1:1111"
"#;
        let loaded = RuntimeConfig::load_with_lookup(
            ConfigLoadOptions {
                config_file: Some(PathBuf::from("/tmp/biohazardfs-test.toml")),
                profile: Some("missing".to_string()),
            },
            |_| None,
            |_| Ok(toml.to_string()),
        )
        .expect("config loads");

        assert!(
            loaded
                .validation_warnings()
                .iter()
                .any(|warning| warning.code == "config_profile_missing")
        );
    }

    #[test]
    fn explicit_missing_config_file_is_an_error() {
        let error = RuntimeConfig::load_with_lookup(
            ConfigLoadOptions {
                config_file: Some(PathBuf::from("/tmp/missing-biohazardfs-test.toml")),
                profile: None,
            },
            |_| None,
            |_| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
        )
        .expect_err("explicit missing config should fail");

        assert_eq!(error.code, "config_not_found");
    }

    #[test]
    fn explicit_missing_config_dir_file_is_an_error() {
        let error = RuntimeConfig::load_with_lookup(
            ConfigLoadOptions {
                config_file: None,
                profile: None,
            },
            lookup(&[(ENV_CONFIG_DIR, "/tmp/missing-biohazardfs-config-dir")]),
            |_| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
        )
        .expect_err("explicit missing config dir file should fail");

        assert_eq!(error.code, "config_not_found");
    }

    #[test]
    fn parse_errors_do_not_echo_source_lines_or_secret_values() {
        let error = RuntimeConfig::load_with_lookup(
            ConfigLoadOptions {
                config_file: Some(PathBuf::from("/tmp/biohazardfs-test.toml")),
                profile: None,
            },
            |_| None,
            |_| Ok("[object_store]\nsecret_access_key = \"do-not-print\" trailing".to_string()),
        )
        .expect_err("invalid TOML should fail");

        assert_eq!(error.code, "config_parse_error");
        assert!(!error.message.contains("do-not-print"));
        assert!(!error.message.contains("secret_access_key"));
    }

    #[test]
    fn type_errors_do_not_echo_source_values() {
        let error = RuntimeConfig::load_with_lookup(
            ConfigLoadOptions {
                config_file: Some(PathBuf::from("/tmp/biohazardfs-test.toml")),
                profile: None,
            },
            |_| None,
            |_| Ok("database = \"postgres://user:do-not-print@example/db\"".to_string()),
        )
        .expect_err("invalid TOML shape should fail");

        assert_eq!(error.code, "config_parse_error");
        assert!(!error.message.contains("do-not-print"));
        assert!(!error.message.contains("postgres://"));
    }

    #[test]
    fn redacts_secret_during_debug_and_serialization() {
        let config = RuntimeConfig::from_lookup(lookup(&[(
            ENV_OBJECT_STORE_SECRET_ACCESS_KEY,
            "do-not-print",
        )]));
        let json = serde_json::to_string(&config).expect("config serializes");
        let debug = format!("{config:?}");
        assert!(json.contains("***REDACTED***"));
        assert!(debug.contains("***REDACTED***"));
        assert!(!json.contains("do-not-print"));
        assert!(!debug.contains("do-not-print"));
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
