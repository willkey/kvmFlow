use kvmflow_domain::PermissionStatus;

pub fn accessibility_permission() -> PermissionStatus {
    if accessibility_trusted() {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Missing
    }
}

pub fn open_accessibility_settings() -> Result<(), PermissionSettingsError> {
    request_accessibility_permission();
    for target in [
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        "/System/Library/PreferencePanes/Security.prefPane",
    ] {
        if std::process::Command::new("/usr/bin/open")
            .arg(target)
            .status()
            .is_ok_and(|status| status.success())
        {
            return Ok(());
        }
    }
    Err(PermissionSettingsError::Unavailable)
}

#[cfg(target_os = "macos")]
fn request_accessibility_permission() {
    use core_foundation::{
        base::TCFType,
        boolean::CFBoolean,
        dictionary::{CFDictionary, CFDictionaryRef},
        string::CFString,
    };

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    }

    let options = CFDictionary::from_CFType_pairs(&[(
        CFString::from_static_string("AXTrustedCheckOptionPrompt"),
        CFBoolean::true_value(),
    )]);
    unsafe {
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
    }
}

#[cfg(not(target_os = "macos"))]
fn request_accessibility_permission() {}

#[cfg(target_os = "macos")]
fn accessibility_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }

    unsafe { AXIsProcessTrusted() != 0 }
}

#[cfg(not(target_os = "macos"))]
fn accessibility_trusted() -> bool {
    false
}

#[derive(Debug, thiserror::Error)]
pub enum PermissionSettingsError {
    #[error("the macOS Accessibility settings pane could not be opened")]
    Unavailable,
}
