#![forbid(unsafe_code)]

//! Defines platform traits and shared values for V2 adapters.

mod input;
mod resources;

pub use input::{
    PlatformBridge, PlatformCommand, PlatformDriver, PlatformEvent, PlatformHandle,
    PlatformSendError, platform_bridge,
};
pub use resources::{ResourceSampleError, ResourceSampler};
