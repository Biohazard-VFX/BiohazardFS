pub mod config;

pub const PRODUCT_NAME: &str = "BiohazardFS";
pub const CLI_BIN: &str = "biohazardfs";
pub const DAEMON_BIN: &str = "biohazardfsd";
pub const DESKTOP_APP_NAME: &str = "Biohazard Workspace";

pub fn dev_loopback_http_endpoint() -> &'static str {
    biohazardfs_api_types::DEV_LOOPBACK_HTTP_ENDPOINT
}
