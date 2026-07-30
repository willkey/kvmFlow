use kvmflow_domain::{ClipboardPayload, InputEvent, PermissionStatus, ScreenEdge};
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum PlatformCommand {
    SetInputSuppressed(bool),
    SetEdgeSwitchEnabled(bool),
    SetEdgeSwitchEdge(ScreenEdge),
    SetEdgeSwitchDisplay(String),
    BeginEdgeTransition { edge: ScreenEdge, position: f32 },
    RestoreLocalPointer { position: f32 },
    Inject(InputEvent),
    ReleaseAll,
    PrepareRemotePointer { edge: ScreenEdge, position: f32 },
    RefreshPermission,
    OpenAccessibilitySettings,
    ApplyClipboard(ClipboardPayload),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlatformEvent {
    InputCaptured(InputEvent),
    LocalEdgeReached { edge: ScreenEdge, position: f32 },
    TargetDisplayChanged { display_id: String },
    ForegroundFullscreenChanged { active: bool },
    SystemResumed,
    ClipboardChanged(ClipboardPayload),
    ClipboardFailed(String),
    RemoteEdgeReached { position: f32 },
    EmergencyReturn { response_ms: Option<f32> },
    PermissionChanged(PermissionStatus),
    RemotePointerPrepared,
    AdapterFailed(String),
}

#[derive(Clone)]
pub struct PlatformHandle {
    command_tx: mpsc::Sender<PlatformCommand>,
}

impl std::fmt::Debug for PlatformHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlatformHandle")
            .finish_non_exhaustive()
    }
}

impl PlatformHandle {
    pub async fn send(&self, command: PlatformCommand) -> Result<(), PlatformSendError> {
        self.command_tx
            .send(command)
            .await
            .map_err(|_| PlatformSendError::Stopped)
    }
}

pub struct PlatformBridge {
    pub handle: PlatformHandle,
    pub events: mpsc::Receiver<PlatformEvent>,
}

pub struct PlatformDriver {
    pub commands: mpsc::Receiver<PlatformCommand>,
    pub events: mpsc::Sender<PlatformEvent>,
}

pub fn platform_bridge(capacity: usize) -> (PlatformDriver, PlatformBridge) {
    let (command_tx, command_rx) = mpsc::channel(capacity);
    let (event_tx, event_rx) = mpsc::channel(capacity);
    (
        PlatformDriver {
            commands: command_rx,
            events: event_tx,
        },
        PlatformBridge {
            handle: PlatformHandle { command_tx },
            events: event_rx,
        },
    )
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformSendError {
    #[error("platform adapter has stopped")]
    Stopped,
}
