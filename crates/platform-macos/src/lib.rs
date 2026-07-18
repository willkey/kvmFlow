#![deny(unsafe_op_in_unsafe_fn)]

//! Hosts the macOS implementation boundary for V2 platform adapters.

mod injection;
mod permissions;
mod runtime;

pub use injection::{InjectionError, InjectionOutcome, MacInjectionState};
pub use permissions::{accessibility_permission, open_accessibility_settings};
pub use runtime::spawn_macos_platform_adapter;
