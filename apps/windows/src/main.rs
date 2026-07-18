#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
#![forbid(unsafe_code)]

#[cfg(target_os = "windows")]
fn main() {
    kvmflow_windows_app::run()
}

#[cfg(not(target_os = "windows"))]
fn main() {}
