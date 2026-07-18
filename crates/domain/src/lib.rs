#![forbid(unsafe_code)]

//! Defines the shared V2 domain state and policy boundary.

mod connection;
mod input;
mod lifecycle;
mod snapshot;

pub use connection::{
    ConnectionError, ConnectionErrorStage, ControlState, KvmStatus, LinkState, LocalControlPolicy,
    NetworkEndpoint, PairingStatus, PermissionStatus, ScreenEdge, TrustState,
};
pub use input::{
    InputEvent, KeyAction, KeyUsage, KeyboardMonitorState, KeyboardTransition, MouseButton,
};
pub use lifecycle::{
    BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole, EngineEffect, EngineEvent,
    GateStatus, MetricProgress, ResourceSample, RuntimePhase, RuntimeState, ValidationProgress,
    WindowState, reduce,
};
pub use snapshot::AppSnapshot;
