use super::{Capabilities, Platform};
pub const CAPABILITIES: Capabilities = Capabilities {
    platform: Platform::Linux,
    long_running_services: true,
    local_database: true,
    native_webview: true,
};
