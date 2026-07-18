#![forbid(unsafe_code)]

#[cfg(target_os = "macos")]
fn main() {
    kvmflow_macos_app::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {}
