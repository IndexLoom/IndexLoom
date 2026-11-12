//! Platform capabilities used to keep the engine independent of OS concerns.
//! These are capability declarations, not permission requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Android,
    Ios,
    Windows,
    Linux,
    Macos,
    Docker,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub platform: Platform,
    /// True for conventional server processes, not mobile app background modes.
    pub long_running_services: bool,
    pub local_database: bool,
    pub native_webview: bool,
}

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "linux")]
mod docker;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[must_use]
pub fn current() -> Capabilities {
    #[cfg(target_os = "android")]
    {
        return android::CAPABILITIES;
    }
    #[cfg(target_os = "ios")]
    {
        return ios::CAPABILITIES;
    }
    #[cfg(target_os = "windows")]
    {
        return windows::CAPABILITIES;
    }
    #[cfg(target_os = "macos")]
    {
        return macos::CAPABILITIES;
    }
    #[cfg(target_os = "linux")]
    {
        if docker::detected() {
            return docker::CAPABILITIES;
        }
        return linux::CAPABILITIES;
    }
    #[allow(unreachable_code)]
    Capabilities {
        platform: Platform::Other,
        long_running_services: false,
        local_database: true,
        native_webview: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Platform, current};
    #[test]
    fn platform_profile_is_consistent() {
        let profile = current();
        assert!(profile.local_database);
        match profile.platform {
            Platform::Ios | Platform::Android => assert!(!profile.long_running_services),
            _ => {}
        }
    }
}
