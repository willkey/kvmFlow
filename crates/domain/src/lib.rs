#![forbid(unsafe_code)]

//! Defines the shared V2 domain state and policy boundary.

mod clipboard;
mod connection;
mod input;
mod lifecycle;
mod snapshot;

pub use clipboard::{
    ClipboardContentKind, ClipboardFile, ClipboardHistoryItem, ClipboardPayload, ClipboardStatus,
    ClipboardTransferState,
};
pub use connection::{
    ConnectionError, ConnectionErrorStage, ControlState, DisplayTarget, KvmStatus, LinkState,
    LocalControlPolicy, NetworkEndpoint, PairingStatus, PermissionStatus, ScreenEdge, TrustState,
};
pub use input::{
    InputEvent, KeyAction, KeyUsage, KeyboardMonitorState, KeyboardTransition, MouseButton,
};
pub use lifecycle::{
    BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole, EngineEffect, EngineEvent,
    GateStatus, KvmValidationProgress, MetricProgress, ResourceSample, RuntimePhase, RuntimeState,
    ValidationProgress, WindowState, reduce,
};
pub use snapshot::AppSnapshot;
