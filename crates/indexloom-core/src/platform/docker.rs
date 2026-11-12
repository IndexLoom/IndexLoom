use super::{Capabilities, Platform};
pub const CAPABILITIES: Capabilities = Capabilities {
    platform: Platform::Docker,
    long_running_services: true,
    local_database: true,
    native_webview: false,
};

#[cfg(target_os = "linux")]
pub fn detected() -> bool {
    std::path::Path::new("/.dockerenv").exists()
        || std::env::var("INDEXLOOM_PLATFORM").as_deref() == Ok("docker")
}
