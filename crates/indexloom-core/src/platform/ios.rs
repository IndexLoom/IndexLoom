use super::{Capabilities, Platform};
pub const CAPABILITIES: Capabilities = Capabilities {
    platform: Platform::Ios,
    long_running_services: false,
    local_database: true,
    native_webview: true,
};
