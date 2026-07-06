//! BiohazardFS core: domain model, IDs, path normalization, and cache state
//! machine. Config loading lives in [`config`]; this crate hosts the
//! safety-critical domain types shared by the daemon and server crates.

pub mod cache;
pub mod config;
pub mod conflict;
pub mod error;
pub mod event;
pub mod grant;
pub mod id;
pub mod lock;
pub mod node;
pub mod operation;
pub mod org;
pub mod path;
pub mod snapshot;
pub mod version;

pub use biohazardfs_api_types::Source;

pub const PRODUCT_NAME: &str = "BiohazardFS";
pub const CLI_BIN: &str = "biohazardfs";
pub const DAEMON_BIN: &str = "biohazardfsd";
pub const DESKTOP_APP_NAME: &str = "Biohazard Workspace";

pub fn dev_loopback_http_endpoint() -> &'static str {
    biohazardfs_api_types::DEV_LOOPBACK_HTTP_ENDPOINT
}
