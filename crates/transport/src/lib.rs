#![forbid(unsafe_code)]

//! TLS transport, long-term device identities, pairing, and trusted sessions.

mod bulk;
mod channel;
mod config;
mod identity;
mod identity_store;
mod runtime;
mod tls;

pub use channel::{ChannelParts, ChannelTrust, SecureChannel, SecureReadHalf, SecureWriteHalf};
pub use config::{
    ConfigError, ConfigStore, DEFAULT_MAC_LISTEN_PORT, MacRuntimeConfig, WindowsRuntimeConfig,
};
pub use identity::{DeviceIdentity, IdentityError, TrustedPeer};
pub use identity_store::{IDENTITY_FILE_NAME, IdentityStoreError, load_or_create_identity};
pub use runtime::{
    RecoveryTrigger, RuntimeBridge, RuntimeCommand, RuntimeEvent, RuntimeHandle,
    spawn_macos_runtime, spawn_windows_runtime,
};
pub use tls::{SecureListener, TransportError, connect};
mod clipboard_history;
mod clipboard_transfer;
