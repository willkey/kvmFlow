#![deny(unsafe_op_in_unsafe_fn)]

//! Hosts the Windows implementation boundary for V2 platform adapters.

mod input_hook;

pub use input_hook::{
    WindowsInputAdapterError, spawn_windows_platform_adapter, virtual_key_to_remote_usage,
};
