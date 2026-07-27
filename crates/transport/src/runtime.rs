use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kvmflow_domain::{
    ClipboardHistoryItem, ClipboardPayload, ClipboardTransferState, InputEvent, LocalControlPolicy,
    NetworkEndpoint, ScreenEdge,
};
use kvmflow_protocol::{ChannelPurpose, ProtocolMessage};
use tokio::{
    sync::{Notify, mpsc},
    time::{Instant, MissedTickBehavior},
};

use crate::{
    ChannelTrust, ConfigStore, DeviceIdentity, MacRuntimeConfig, SecureChannel, SecureListener,
    TransportError, TrustedPeer, WindowsRuntimeConfig,
    bulk::{BulkChannel, BulkMessage},
    clipboard_history::{ClipboardHistoryStore, activated_payload_id},
    clipboard_transfer::{IncomingClipboardTransfer, OutgoingClipboardTransfer},
    connect,
};

const RUNTIME_COMMAND_CAPACITY: usize = 128;
const RUNTIME_EVENT_CAPACITY: usize = 128;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(3);
const CONTROL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MOUSE_MOVEMENT_FLUSH_INTERVAL: Duration = Duration::from_millis(4);
const NETWORK_CHANGE_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryTrigger {
    NetworkChanged,
    SystemResumed,
}

#[derive(Debug)]
pub enum RuntimeCommand {
    ConfigureEndpoint(NetworkEndpoint),
    ConfigureScreenEdge(ScreenEdge),
    ConfigureTargetDisplay(String),
    ConfigureListenPort(u16),
    ConfirmPairing,
    RejectPairing,
    EnterControl {
        edge: ScreenEdge,
        position: f32,
    },
    RespondEnterControl {
        accepted: bool,
        reason: Option<String>,
    },
    ReturnControl {
        position: f32,
    },
    SendInput(InputEvent),
    ReleaseAll,
    RecoverNow(RecoveryTrigger),
    SetControlEnabled(bool),
    SetGameModeEnabled(bool),
    SetLocalPolicy(LocalControlPolicy),
    SetClipboard(ClipboardPayload),
    ClipboardUpdated,
    ActivateClipboardHistory(String),
    ClearClipboardHistory,
    Disconnect,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeEvent {
    TrustedPeerLoaded {
        peer: Option<TrustedPeer>,
    },
    LocalPolicyChanged {
        policy: LocalControlPolicy,
    },
    ControlEnabledChanged {
        enabled: bool,
    },
    GameModeEnabledChanged {
        enabled: bool,
    },
    ClipboardTransferChanged {
        state: ClipboardTransferState,
        error: Option<String>,
        updated_at_unix_ms: Option<u64>,
    },
    ClipboardReceived(ClipboardPayload),
    ClipboardHistoryChanged(Vec<ClipboardHistoryItem>),
    ClipboardHistoryActivated(ClipboardPayload),
    ScreenEdgeChanged {
        edge: ScreenEdge,
    },
    ListenerStarted {
        ip: String,
        port: u16,
    },
    ListenerFailed {
        port: u16,
        message: String,
    },
    Connecting {
        attempt_id: u64,
        endpoint: NetworkEndpoint,
    },
    ConnectionFailed {
        attempt_id: u64,
        failure_count: u32,
        message: String,
    },
    RecoveryTriggered {
        trigger: RecoveryTrigger,
    },
    Pairing {
        code: String,
        peer: TrustedPeer,
        local_confirmed: bool,
        remote_confirmed: bool,
        expires_at_unix_ms: u64,
    },
    PairingRejected {
        local: bool,
    },
    Online {
        session_id: String,
        peer: TrustedPeer,
        endpoint: NetworkEndpoint,
    },
    Offline {
        reason: String,
        retrying: bool,
    },
    LatencyUpdated {
        milliseconds: f32,
    },
    EnterControlRequested {
        edge: ScreenEdge,
        position: f32,
    },
    EnterControlResult {
        accepted: bool,
        reason: Option<String>,
    },
    ReturnControlRequested {
        position: f32,
    },
    ReturnControlAcknowledged,
    InputReceived(InputEvent),
    ReleaseAllRequested,
    RemotePauseChanged {
        paused: bool,
    },
    ConfigUpdated,
    RuntimeFailed {
        message: String,
    },
}

#[derive(Clone)]
pub struct RuntimeHandle {
    command_tx: mpsc::Sender<RuntimeCommand>,
    mouse_movement: Option<MouseMovementSender>,
    clipboard: ClipboardMailbox,
}

impl std::fmt::Debug for RuntimeHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeHandle")
            .finish_non_exhaustive()
    }
}

impl RuntimeHandle {
    pub async fn send(&self, command: RuntimeCommand) -> Result<(), RuntimeSendError> {
        let command = match command {
            RuntimeCommand::SetClipboard(payload) => {
                self.clipboard.replace(payload);
                RuntimeCommand::ClipboardUpdated
            }
            command => command,
        };
        if let Some(mouse_movement) = self.mouse_movement.as_ref() {
            if let RuntimeCommand::SendInput(InputEvent::MouseMove { dx, dy }) = &command {
                if self.command_tx.is_closed() {
                    return Err(RuntimeSendError::Stopped);
                }
                mouse_movement.push(*dx, *dy);
                return Ok(());
            }
            if clears_pending_mouse_movement(&command) {
                mouse_movement.clear();
            }
        }
        self.command_tx
            .send(command)
            .await
            .map_err(|_| RuntimeSendError::Stopped)
    }
}

pub struct RuntimeBridge {
    pub handle: RuntimeHandle,
    pub events: mpsc::Receiver<RuntimeEvent>,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeSendError {
    #[error("KVM runtime has stopped")]
    Stopped,
}

pub fn spawn_windows_runtime(
    identity: DeviceIdentity,
    store: ConfigStore,
    config: WindowsRuntimeConfig,
) -> RuntimeBridge {
    let (command_tx, command_rx) = mpsc::channel(RUNTIME_COMMAND_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel(RUNTIME_EVENT_CAPACITY);
    let (mouse_movement_tx, mouse_movement_rx) = mouse_movement_channel();
    let clipboard = ClipboardMailbox::default();
    spawn_network_change_watcher(command_tx.clone());
    tokio::spawn(run_windows_runtime(
        identity,
        store,
        config,
        command_rx,
        mouse_movement_rx,
        clipboard.clone(),
        event_tx,
    ));
    RuntimeBridge {
        handle: RuntimeHandle {
            command_tx,
            mouse_movement: Some(mouse_movement_tx),
            clipboard,
        },
        events: event_rx,
    }
}

pub fn spawn_macos_runtime(
    identity: DeviceIdentity,
    store: ConfigStore,
    config: MacRuntimeConfig,
) -> RuntimeBridge {
    let (command_tx, command_rx) = mpsc::channel(RUNTIME_COMMAND_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel(RUNTIME_EVENT_CAPACITY);
    let clipboard = ClipboardMailbox::default();
    spawn_network_change_watcher(command_tx.clone());
    tokio::spawn(run_macos_runtime(
        identity,
        store,
        config,
        command_rx,
        clipboard.clone(),
        event_tx,
    ));
    RuntimeBridge {
        handle: RuntimeHandle {
            command_tx,
            mouse_movement: None,
            clipboard,
        },
        events: event_rx,
    }
}

#[derive(Clone, Default)]
struct ClipboardMailbox {
    latest: Arc<Mutex<Option<ClipboardPayload>>>,
}

impl ClipboardMailbox {
    fn replace(&self, payload: ClipboardPayload) {
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *latest = Some(payload);
    }

    fn current(&self) -> Option<ClipboardPayload> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

struct ClipboardSession {
    mailbox: ClipboardMailbox,
    data_dir: std::path::PathBuf,
    prune_incoming_before_receive: bool,
    outgoing: Option<OutgoingClipboardTransfer>,
    incoming: Option<IncomingClipboardTransfer>,
    prepared_tx: mpsc::Sender<Result<OutgoingClipboardTransfer, String>>,
    prepared_rx: mpsc::Receiver<Result<OutgoingClipboardTransfer, String>>,
    last_requested_id: Option<String>,
    outgoing_started_at: Option<Instant>,
    outgoing_bytes: u64,
    incoming_started_at: Option<Instant>,
    incoming_bytes: u64,
}

struct WindowsRuntimeServices<'a> {
    store: &'a ConfigStore,
    clipboard: &'a ClipboardMailbox,
    history: &'a ClipboardHistoryStore,
    events: &'a mpsc::Sender<RuntimeEvent>,
}

struct WindowsConnectionContext<'a> {
    address: SocketAddr,
    identity: &'a DeviceIdentity,
    endpoint: NetworkEndpoint,
}

struct MacChannelServices<'a> {
    listener: &'a mut SecureListener,
    store: &'a ConfigStore,
    clipboard: &'a ClipboardMailbox,
    events: &'a mpsc::Sender<RuntimeEvent>,
}

impl ClipboardSession {
    fn new(
        mailbox: ClipboardMailbox,
        data_dir: std::path::PathBuf,
        prune_incoming_before_receive: bool,
    ) -> Self {
        let (prepared_tx, prepared_rx) = mpsc::channel(2);
        Self {
            mailbox,
            data_dir,
            prune_incoming_before_receive,
            outgoing: None,
            incoming: None,
            prepared_tx,
            prepared_rx,
            last_requested_id: None,
            outgoing_started_at: None,
            outgoing_bytes: 0,
            incoming_started_at: None,
            incoming_bytes: 0,
        }
    }

    async fn request_current(&mut self, events: &mpsc::Sender<RuntimeEvent>) {
        let Some(payload) = self.mailbox.current() else {
            return;
        };
        if self.last_requested_id.as_deref() == Some(payload.id.as_str()) {
            return;
        }
        self.last_requested_id = Some(payload.id.clone());
        self.outgoing = None;
        emit(
            events,
            RuntimeEvent::ClipboardTransferChanged {
                state: ClipboardTransferState::Sending,
                error: None,
                updated_at_unix_ms: Some(payload.captured_at_unix_ms),
            },
        )
        .await;
        let prepared_tx = self.prepared_tx.clone();
        tokio::spawn(async move {
            let result = OutgoingClipboardTransfer::prepare(payload)
                .await
                .map_err(|error| error.to_string());
            let _ = prepared_tx.send(result).await;
        });
    }

    fn accept_prepared(
        &mut self,
        prepared: Result<OutgoingClipboardTransfer, String>,
    ) -> Result<(), String> {
        let prepared = prepared?;
        if self
            .mailbox
            .current()
            .as_ref()
            .is_some_and(|payload| payload.id == prepared.transfer_id())
        {
            self.outgoing = Some(prepared);
            self.outgoing_started_at = Some(Instant::now());
            self.outgoing_bytes = 0;
        }
        Ok(())
    }

    fn has_outgoing(&self) -> bool {
        self.outgoing.is_some()
    }

    fn finish_outgoing(&mut self) {
        self.outgoing = None;
        self.outgoing_started_at = None;
        self.outgoing_bytes = 0;
    }

    fn record_outgoing_sent(&mut self, message: &BulkMessage) {
        match message {
            BulkMessage::Chunk { data, .. } => {
                self.outgoing_bytes = self.outgoing_bytes.saturating_add(data.len() as u64);
            }
            BulkMessage::Complete { transfer_id } => {
                log_clipboard_throughput(
                    "send",
                    transfer_id,
                    self.outgoing_bytes,
                    self.outgoing_started_at,
                );
            }
            BulkMessage::Offer(_) => {}
        }
    }

    async fn receive(&mut self, message: BulkMessage) -> Result<Option<ClipboardPayload>, String> {
        match message {
            BulkMessage::Offer(offer) => {
                if offer.reset {
                    self.incoming_started_at = Some(Instant::now());
                    self.incoming_bytes = 0;
                    if self.prune_incoming_before_receive {
                        crate::clipboard_transfer::prune_incoming_cache(
                            &self.data_dir,
                            &std::collections::HashSet::new(),
                            1,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    }
                    self.incoming = Some(
                        IncomingClipboardTransfer::begin(&self.data_dir, offer)
                            .await
                            .map_err(|error| error.to_string())?,
                    );
                } else {
                    self.incoming
                        .as_mut()
                        .ok_or_else(|| {
                            "clipboard manifest continuation has no transfer".to_owned()
                        })?
                        .append_offer(offer)
                        .await
                        .map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            BulkMessage::Chunk {
                transfer_id,
                stream_id,
                offset,
                data,
            } => {
                let incoming = self
                    .incoming
                    .as_mut()
                    .ok_or_else(|| "clipboard chunk has no active transfer".to_owned())?;
                if incoming.transfer_id() != transfer_id {
                    return Err("clipboard chunk transfer identity changed".into());
                }
                incoming
                    .write_chunk(stream_id, offset, &data)
                    .await
                    .map_err(|error| error.to_string())?;
                self.incoming_bytes = self.incoming_bytes.saturating_add(data.len() as u64);
                Ok(None)
            }
            BulkMessage::Complete { transfer_id } => {
                let incoming = self
                    .incoming
                    .take()
                    .ok_or_else(|| "clipboard completion has no active transfer".to_owned())?;
                if incoming.transfer_id() != transfer_id {
                    return Err("clipboard completion transfer identity changed".into());
                }
                let payload = incoming
                    .complete()
                    .await
                    .map_err(|error| error.to_string())?;
                log_clipboard_throughput(
                    "receive",
                    &transfer_id,
                    self.incoming_bytes,
                    self.incoming_started_at,
                );
                self.incoming_started_at = None;
                self.incoming_bytes = 0;
                Ok(Some(payload))
            }
        }
    }
}

fn log_clipboard_throughput(
    direction: &'static str,
    transfer_id: &str,
    bytes: u64,
    started_at: Option<Instant>,
) {
    let elapsed = started_at.map_or(Duration::ZERO, |started_at| started_at.elapsed());
    let elapsed_ms = elapsed.as_millis().max(1) as u64;
    let mebibytes_per_second = bytes as f64 / (1024.0 * 1024.0) / (elapsed_ms as f64 / 1_000.0);
    tracing::info!(
        component = "clipboard",
        direction,
        transfer_id,
        bytes,
        elapsed_ms,
        mebibytes_per_second,
        "clipboard bulk transfer summary"
    );
}

async fn run_clipboard_worker(
    mut bulk: BulkChannel,
    mailbox: ClipboardMailbox,
    data_dir: std::path::PathBuf,
    prune_incoming_before_receive: bool,
    history: Option<ClipboardHistoryStore>,
    mut updates: mpsc::Receiver<()>,
    events: mpsc::Sender<RuntimeEvent>,
) -> Result<(), String> {
    let mut session = ClipboardSession::new(mailbox, data_dir, prune_incoming_before_receive);
    session.request_current(&events).await;
    loop {
        tokio::select! {
            update = updates.recv() => {
                if update.is_none() {
                    return Ok(());
                }
                session.request_current(&events).await;
            }
            prepared = session.prepared_rx.recv() => {
                if let Some(prepared) = prepared
                    && let Err(error) = session.accept_prepared(prepared)
                {
                    emit(&events, RuntimeEvent::ClipboardTransferChanged {
                        state: ClipboardTransferState::Failed,
                        error: Some(error),
                        updated_at_unix_ms: None,
                    }).await;
                }
            }
            message = bulk.receive() => {
                let message = message.map_err(|error| error.to_string())?;
                let receiving_started =
                    matches!(&message, BulkMessage::Offer(offer) if offer.reset);
                if receiving_started {
                    emit(&events, RuntimeEvent::ClipboardTransferChanged {
                        state: ClipboardTransferState::Receiving,
                        error: None,
                        updated_at_unix_ms: None,
                    }).await;
                }
                match session.receive(message).await {
                    Ok(Some(payload)) => {
                        let updated_at_unix_ms = payload.captured_at_unix_ms;
                        if let Some(history) = history.as_ref() {
                            record_clipboard_payload(&payload, history, &events).await;
                        }
                        emit(&events, RuntimeEvent::ClipboardReceived(payload)).await;
                        emit(&events, RuntimeEvent::ClipboardTransferChanged {
                            state: ClipboardTransferState::Idle,
                            error: None,
                            updated_at_unix_ms: Some(updated_at_unix_ms),
                        }).await;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        emit(&events, RuntimeEvent::ClipboardTransferChanged {
                            state: ClipboardTransferState::Failed,
                            error: Some(error.clone()),
                            updated_at_unix_ms: None,
                        }).await;
                        return Err(error);
                    }
                }
            }
            outgoing_message = async {
                session
                    .outgoing
                    .as_mut()
                    .expect("outgoing clipboard checked")
                    .next_message()
                    .await
                    .map_err(|error| error.to_string())
            }, if session.has_outgoing() => {
                match outgoing_message {
                    Ok(Some(message)) => {
                        let completed = matches!(&message, BulkMessage::Complete { .. });
                        bulk.send(&message).await.map_err(|error| error.to_string())?;
                        session.record_outgoing_sent(&message);
                        if completed {
                            session.finish_outgoing();
                            emit(&events, RuntimeEvent::ClipboardTransferChanged {
                                state: ClipboardTransferState::Idle,
                                error: None,
                                updated_at_unix_ms: None,
                            }).await;
                        }
                    }
                    Ok(None) => session.finish_outgoing(),
                    Err(error) => {
                        session.finish_outgoing();
                        emit(&events, RuntimeEvent::ClipboardTransferChanged {
                            state: ClipboardTransferState::Failed,
                            error: Some(error),
                            updated_at_unix_ms: None,
                        }).await;
                    }
                }
            }
        }
    }
}

fn clears_pending_mouse_movement(command: &RuntimeCommand) -> bool {
    matches!(
        command,
        RuntimeCommand::ConfigureEndpoint(_)
            | RuntimeCommand::EnterControl { .. }
            | RuntimeCommand::ReturnControl { .. }
            | RuntimeCommand::ReleaseAll
            | RuntimeCommand::RecoverNow(_)
            | RuntimeCommand::Disconnect
            | RuntimeCommand::Shutdown
            | RuntimeCommand::SetControlEnabled(false)
            | RuntimeCommand::SetLocalPolicy(LocalControlPolicy::Paused)
    )
}

#[derive(Debug, Default)]
struct PendingMouseMovement {
    dx: i64,
    dy: i64,
    pending_since: Option<Instant>,
}

impl PendingMouseMovement {
    fn push(&mut self, dx: i32, dy: i32) {
        if self.is_empty() {
            self.pending_since = Some(Instant::now());
        }
        self.dx = self.dx.saturating_add(i64::from(dx));
        self.dy = self.dy.saturating_add(i64::from(dy));
        if self.is_empty() {
            self.pending_since = None;
        }
    }

    fn take_chunk(&mut self) -> Option<InputEvent> {
        let dx = self.dx.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        let dy = self.dy.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        self.dx -= i64::from(dx);
        self.dy -= i64::from(dy);
        if self.is_empty() {
            self.pending_since = None;
        }
        (dx != 0 || dy != 0).then_some(InputEvent::MouseMove { dx, dy })
    }

    fn is_empty(&self) -> bool {
        self.dx == 0 && self.dy == 0
    }

    fn clear(&mut self) {
        self.dx = 0;
        self.dy = 0;
        self.pending_since = None;
    }
}

#[derive(Debug, Default)]
struct MouseMovementState {
    pending: Mutex<PendingMouseMovement>,
    notify: Notify,
    captured_events: AtomicU64,
    emitted_events: AtomicU64,
    max_pending_age_us: AtomicU64,
}

#[derive(Clone, Debug)]
struct MouseMovementSender {
    state: Arc<MouseMovementState>,
}

impl MouseMovementSender {
    fn push(&self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        self.with_pending(|pending| {
            pending.push(dx, dy);
            self.state.captured_events.fetch_add(1, Ordering::Relaxed);
        });
        self.state.notify.notify_one();
    }

    fn clear(&self) {
        self.with_pending(PendingMouseMovement::clear);
    }

    fn with_pending(&self, update: impl FnOnce(&mut PendingMouseMovement)) {
        let mut pending = self
            .state
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut pending);
    }
}

#[derive(Debug)]
struct MouseMovementReceiver {
    state: Arc<MouseMovementState>,
    last_emitted_at: Option<Instant>,
    activation_logged: bool,
}

impl MouseMovementReceiver {
    async fn receive(&mut self) -> InputEvent {
        loop {
            let notified = self.state.notify.notified();
            if !self.has_pending() {
                notified.await;
                continue;
            }
            if let Some(last_emitted_at) = self.last_emitted_at {
                let deadline = last_emitted_at + MOUSE_MOVEMENT_FLUSH_INTERVAL;
                if deadline > Instant::now() {
                    tokio::time::sleep_until(deadline).await;
                }
            }
            if let Some(event) = self.take_pending() {
                self.last_emitted_at = Some(Instant::now());
                if !self.activation_logged {
                    tracing::info!(
                        component = "input",
                        flush_interval_ms = MOUSE_MOVEMENT_FLUSH_INTERVAL.as_millis(),
                        "Windows mouse movement coalescing active"
                    );
                    self.activation_logged = true;
                }
                return event;
            }
        }
    }

    fn clear(&mut self) {
        let mut pending = self
            .state
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.clear();
        let captured_events = self.state.captured_events.swap(0, Ordering::Relaxed);
        let emitted_events = self.state.emitted_events.swap(0, Ordering::Relaxed);
        let max_pending_age_us = self.state.max_pending_age_us.swap(0, Ordering::Relaxed);
        if captured_events > 0 {
            tracing::info!(
                component = "input",
                stage = "windows_transport",
                captured_events,
                emitted_events,
                coalesced_events = captured_events.saturating_sub(emitted_events),
                max_pending_age_us,
                flush_interval_ms = MOUSE_MOVEMENT_FLUSH_INTERVAL.as_millis(),
                "Windows mouse movement transport summary"
            );
        }
        self.last_emitted_at = None;
        self.activation_logged = false;
    }

    fn has_pending(&self) -> bool {
        let pending = self
            .state
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !pending.is_empty()
    }

    fn take_pending(&self) -> Option<InputEvent> {
        let mut pending = self
            .state
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let pending_since = pending.pending_since;
        let event = pending.take_chunk();
        if event.is_some() {
            self.state.emitted_events.fetch_add(1, Ordering::Relaxed);
            let pending_age_us = pending_since
                .map(|started_at| started_at.elapsed().as_micros())
                .unwrap_or_default()
                .min(u128::from(u64::MAX)) as u64;
            self.state
                .max_pending_age_us
                .fetch_max(pending_age_us, Ordering::Relaxed);
        }
        event
    }
}

fn mouse_movement_channel() -> (MouseMovementSender, MouseMovementReceiver) {
    let state = Arc::new(MouseMovementState::default());
    (
        MouseMovementSender {
            state: state.clone(),
        },
        MouseMovementReceiver {
            state,
            last_emitted_at: None,
            activation_logged: false,
        },
    )
}

async fn run_windows_runtime(
    identity: DeviceIdentity,
    store: ConfigStore,
    mut config: WindowsRuntimeConfig,
    mut commands: mpsc::Receiver<RuntimeCommand>,
    mut mouse_movement: MouseMovementReceiver,
    clipboard: ClipboardMailbox,
    events: mpsc::Sender<RuntimeEvent>,
) {
    let mut attempt_id = 0_u64;
    let mut failure_count = 0_u32;
    let history = ClipboardHistoryStore::new(store.data_dir());
    if let Err(error) = history.prune_incoming_cache().await {
        emit_clipboard_history_error(&events, error).await;
    }
    let services = WindowsRuntimeServices {
        store: &store,
        clipboard: &clipboard,
        history: &history,
        events: &events,
    };
    emit(
        &events,
        RuntimeEvent::TrustedPeerLoaded {
            peer: config.trusted_peer.clone(),
        },
    )
    .await;
    emit(
        &events,
        RuntimeEvent::ScreenEdgeChanged { edge: config.edge },
    )
    .await;
    emit(
        &events,
        RuntimeEvent::ControlEnabledChanged {
            enabled: config.control_enabled,
        },
    )
    .await;
    emit(
        &events,
        RuntimeEvent::GameModeEnabledChanged {
            enabled: config.game_mode_enabled,
        },
    )
    .await;
    publish_clipboard_history(&history, &events).await;

    loop {
        let Some(endpoint) = wait_for_windows_endpoint(&mut config, &mut commands, &services).await
        else {
            return;
        };
        attempt_id = attempt_id.saturating_add(1);
        emit(
            &events,
            RuntimeEvent::Connecting {
                attempt_id,
                endpoint: endpoint.clone(),
            },
        )
        .await;

        let address = match endpoint_to_socket_addr(&endpoint) {
            Ok(address) => address,
            Err(error) => {
                failure_count = failure_count.saturating_add(1);
                emit(
                    &events,
                    RuntimeEvent::ConnectionFailed {
                        attempt_id,
                        failure_count,
                        message: error,
                    },
                )
                .await;
                if !windows_backoff(&mut config, &mut commands, &services).await {
                    return;
                }
                continue;
            }
        };
        let connection = tokio::time::timeout(
            CONNECT_TIMEOUT,
            connect(address, &identity, config.trusted_peer.as_ref()),
        )
        .await;
        let channel = match connection {
            Ok(Ok(channel)) => channel,
            Ok(Err(error)) => {
                failure_count = failure_count.saturating_add(1);
                emit_connection_failure(&events, attempt_id, failure_count, error).await;
                if !windows_backoff(&mut config, &mut commands, &services).await {
                    return;
                }
                continue;
            }
            Err(_) => {
                failure_count = failure_count.saturating_add(1);
                emit(
                    &events,
                    RuntimeEvent::ConnectionFailed {
                        attempt_id,
                        failure_count,
                        message: "连接 Mac 超时".into(),
                    },
                )
                .await;
                if !windows_backoff(&mut config, &mut commands, &services).await {
                    return;
                }
                continue;
            }
        };
        failure_count = 0;

        let outcome = prepare_windows_channel(
            channel,
            WindowsConnectionContext {
                address,
                identity: &identity,
                endpoint: endpoint.clone(),
            },
            &mut config,
            &mut commands,
            &mut mouse_movement,
            &services,
        )
        .await;
        mouse_movement.clear();
        match outcome {
            ChannelOutcome::Reconnect(reason) => {
                emit(
                    &events,
                    RuntimeEvent::Offline {
                        reason,
                        retrying: true,
                    },
                )
                .await;
                if !windows_backoff(&mut config, &mut commands, &services).await {
                    return;
                }
            }
            ChannelOutcome::ReconnectNow(reason) => {
                emit(
                    &events,
                    RuntimeEvent::Offline {
                        reason,
                        retrying: true,
                    },
                )
                .await;
            }
            ChannelOutcome::Reconfigure(endpoint) => {
                config.endpoint = Some(endpoint);
                if let Err(error) = store.save_windows(&config) {
                    emit_runtime_failed(&events, error).await;
                }
            }
            ChannelOutcome::Shutdown => return,
        }
    }
}

async fn wait_for_windows_endpoint(
    config: &mut WindowsRuntimeConfig,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    services: &WindowsRuntimeServices<'_>,
) -> Option<NetworkEndpoint> {
    let WindowsRuntimeServices {
        store,
        clipboard,
        history,
        events,
    } = services;
    if let Some(endpoint) = config.endpoint.clone() {
        return Some(endpoint);
    }
    loop {
        match commands.recv().await? {
            RuntimeCommand::ConfigureEndpoint(endpoint) => {
                if let Err(message) = endpoint.validate() {
                    emit(
                        events,
                        RuntimeEvent::RuntimeFailed {
                            message: message.into(),
                        },
                    )
                    .await;
                    continue;
                }
                config.endpoint = Some(endpoint.clone());
                if let Err(error) = store.save_windows(config) {
                    emit_runtime_failed(events, error).await;
                    continue;
                }
                emit(events, RuntimeEvent::ConfigUpdated).await;
                return Some(endpoint);
            }
            RuntimeCommand::ConfigureScreenEdge(edge) => {
                update_windows_edge(config, store, edge, events).await;
            }
            RuntimeCommand::ConfigureTargetDisplay(display_id) => {
                update_windows_target_display(config, store, display_id, events).await;
            }
            RuntimeCommand::SetControlEnabled(enabled) => {
                update_windows_control_enabled(config, store, enabled, events).await;
            }
            RuntimeCommand::SetGameModeEnabled(enabled) => {
                update_windows_game_mode_enabled(config, store, enabled, events).await;
            }
            RuntimeCommand::ClipboardUpdated => {
                record_current_clipboard(clipboard, history, events).await;
            }
            RuntimeCommand::ActivateClipboardHistory(id) => {
                activate_clipboard_history(&id, clipboard, history, events).await;
            }
            RuntimeCommand::ClearClipboardHistory => {
                clear_clipboard_history(history, events).await;
            }
            RuntimeCommand::RecoverNow(trigger) => {
                emit_recovery_trigger(events, trigger).await;
            }
            RuntimeCommand::Shutdown => return None,
            _ => {}
        }
    }
}

async fn windows_backoff(
    config: &mut WindowsRuntimeConfig,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    services: &WindowsRuntimeServices<'_>,
) -> bool {
    let WindowsRuntimeServices {
        store,
        clipboard,
        history,
        events,
    } = services;
    let delay = tokio::time::sleep(RECONNECT_INTERVAL);
    tokio::pin!(delay);
    loop {
        tokio::select! {
            _ = &mut delay => return true,
            command = commands.recv() => {
                match command {
                    Some(RuntimeCommand::ConfigureEndpoint(endpoint)) => {
                        if endpoint.validate().is_ok() {
                            config.endpoint = Some(endpoint);
                            if let Err(error) = store.save_windows(config) {
                                emit_runtime_failed(events, error).await;
                            } else {
                                emit(events, RuntimeEvent::ConfigUpdated).await;
                            }
                            return true;
                        }
                    }
                    Some(RuntimeCommand::ConfigureScreenEdge(edge)) => {
                        update_windows_edge(config, store, edge, events).await;
                    }
                    Some(RuntimeCommand::ConfigureTargetDisplay(display_id)) => {
                        update_windows_target_display(config, store, display_id, events).await;
                    }
                    Some(RuntimeCommand::SetControlEnabled(enabled)) => {
                        update_windows_control_enabled(config, store, enabled, events).await;
                    }
                    Some(RuntimeCommand::SetGameModeEnabled(enabled)) => {
                        update_windows_game_mode_enabled(config, store, enabled, events).await;
                    }
                    Some(RuntimeCommand::ClipboardUpdated) => {
                        record_current_clipboard(clipboard, history, events).await;
                    }
                    Some(RuntimeCommand::ActivateClipboardHistory(id)) => {
                        activate_clipboard_history(&id, clipboard, history, events).await;
                    }
                    Some(RuntimeCommand::ClearClipboardHistory) => {
                        clear_clipboard_history(history, events).await;
                    }
                    Some(RuntimeCommand::RecoverNow(trigger)) => {
                        emit_recovery_trigger(events, trigger).await;
                        return true;
                    }
                    Some(RuntimeCommand::Shutdown) | None => return false,
                    Some(RuntimeCommand::Disconnect) => {
                        config.endpoint = None;
                        if let Err(error) = store.save_windows(config) {
                            emit_runtime_failed(events, error).await;
                        }
                        return true;
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn publish_clipboard_history(
    history: &ClipboardHistoryStore,
    events: &mpsc::Sender<RuntimeEvent>,
) {
    match history.summaries().await {
        Ok(items) => emit(events, RuntimeEvent::ClipboardHistoryChanged(items)).await,
        Err(error) => emit_clipboard_history_error(events, error).await,
    }
}

async fn record_current_clipboard(
    clipboard: &ClipboardMailbox,
    history: &ClipboardHistoryStore,
    events: &mpsc::Sender<RuntimeEvent>,
) {
    if let Some(payload) = clipboard.current() {
        record_clipboard_payload(&payload, history, events).await;
    }
}

async fn record_clipboard_payload(
    payload: &ClipboardPayload,
    history: &ClipboardHistoryStore,
    events: &mpsc::Sender<RuntimeEvent>,
) {
    match history.record(payload).await {
        Ok(items) => emit(events, RuntimeEvent::ClipboardHistoryChanged(items)).await,
        Err(error) => emit_clipboard_history_error(events, error).await,
    }
}

async fn activate_clipboard_history(
    id: &str,
    clipboard: &ClipboardMailbox,
    history: &ClipboardHistoryStore,
    events: &mpsc::Sender<RuntimeEvent>,
) {
    match history.load(id).await {
        Ok(mut payload) => {
            payload.id = activated_payload_id();
            payload.captured_at_unix_ms = unix_time_ms();
            clipboard.replace(payload.clone());
            emit(events, RuntimeEvent::ClipboardHistoryActivated(payload)).await;
        }
        Err(error) => emit_clipboard_history_error(events, error).await,
    }
}

async fn clear_clipboard_history(
    history: &ClipboardHistoryStore,
    events: &mpsc::Sender<RuntimeEvent>,
) {
    match history.clear().await {
        Ok(()) => emit(events, RuntimeEvent::ClipboardHistoryChanged(Vec::new())).await,
        Err(error) => emit_clipboard_history_error(events, error).await,
    }
}

async fn emit_clipboard_history_error(
    events: &mpsc::Sender<RuntimeEvent>,
    error: impl std::fmt::Display,
) {
    emit(
        events,
        RuntimeEvent::RuntimeFailed {
            message: format!("剪贴板历史操作失败：{error}"),
        },
    )
    .await;
}

async fn update_windows_edge(
    config: &mut WindowsRuntimeConfig,
    store: &ConfigStore,
    edge: ScreenEdge,
    events: &mpsc::Sender<RuntimeEvent>,
) {
    if config.edge == edge {
        emit(events, RuntimeEvent::ScreenEdgeChanged { edge }).await;
        return;
    }
    let previous = config.edge;
    config.edge = edge;
    if let Err(error) = store.save_windows(config) {
        config.edge = previous;
        emit(events, RuntimeEvent::ScreenEdgeChanged { edge: previous }).await;
        emit_runtime_failed(events, error).await;
        return;
    }
    emit(events, RuntimeEvent::ScreenEdgeChanged { edge }).await;
    emit(events, RuntimeEvent::ConfigUpdated).await;
}

async fn update_windows_target_display(
    config: &mut WindowsRuntimeConfig,
    store: &ConfigStore,
    display_id: String,
    events: &mpsc::Sender<RuntimeEvent>,
) {
    if display_id.trim().is_empty()
        || config.target_display_id.as_deref() == Some(display_id.as_str())
    {
        return;
    }
    let previous = config.target_display_id.replace(display_id);
    if let Err(error) = store.save_windows(config) {
        config.target_display_id = previous;
        emit_runtime_failed(events, error).await;
        return;
    }
    emit(events, RuntimeEvent::ConfigUpdated).await;
}

async fn update_windows_control_enabled(
    config: &mut WindowsRuntimeConfig,
    store: &ConfigStore,
    enabled: bool,
    events: &mpsc::Sender<RuntimeEvent>,
) {
    if config.control_enabled == enabled {
        emit(events, RuntimeEvent::ControlEnabledChanged { enabled }).await;
        return;
    }
    let previous = config.control_enabled;
    config.control_enabled = enabled;
    if let Err(error) = store.save_windows(config) {
        config.control_enabled = previous;
        emit(
            events,
            RuntimeEvent::ControlEnabledChanged { enabled: previous },
        )
        .await;
        emit_runtime_failed(events, error).await;
        return;
    }
    emit(events, RuntimeEvent::ControlEnabledChanged { enabled }).await;
    emit(events, RuntimeEvent::ConfigUpdated).await;
}

async fn update_windows_game_mode_enabled(
    config: &mut WindowsRuntimeConfig,
    store: &ConfigStore,
    enabled: bool,
    events: &mpsc::Sender<RuntimeEvent>,
) {
    if config.game_mode_enabled == enabled {
        emit(events, RuntimeEvent::GameModeEnabledChanged { enabled }).await;
        return;
    }
    let previous = config.game_mode_enabled;
    config.game_mode_enabled = enabled;
    if let Err(error) = store.save_windows(config) {
        config.game_mode_enabled = previous;
        emit(
            events,
            RuntimeEvent::GameModeEnabledChanged { enabled: previous },
        )
        .await;
        emit_runtime_failed(events, error).await;
        return;
    }
    emit(events, RuntimeEvent::GameModeEnabledChanged { enabled }).await;
    emit(events, RuntimeEvent::ConfigUpdated).await;
}

async fn prepare_windows_channel(
    mut channel: SecureChannel,
    connection: WindowsConnectionContext<'_>,
    config: &mut WindowsRuntimeConfig,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    mouse_movement: &mut MouseMovementReceiver,
    services: &WindowsRuntimeServices<'_>,
) -> ChannelOutcome {
    let WindowsConnectionContext {
        address,
        identity,
        endpoint,
    } = connection;
    let WindowsRuntimeServices { store, events, .. } = services;
    let previous_trust = config.trusted_peer.clone();
    if let ChannelTrust::Pairing { code } = channel.trust().clone() {
        let paired = match run_pairing(&mut channel, code, commands, events).await {
            Ok(paired) => paired,
            Err(outcome) => return outcome,
        };
        config.trusted_peer = Some(paired);
        if let Err(error) = store.save_windows(config) {
            config.trusted_peer = previous_trust;
            emit_runtime_failed(events, error).await;
            return ChannelOutcome::Reconnect("无法保存配对信息".into());
        }
        if let Err(error) = exchange_session_ready(&mut channel).await {
            config.trusted_peer = previous_trust;
            let _ = store.save_windows(config);
            return ChannelOutcome::Reconnect(error);
        }
    }
    if let Err(error) = open_channel(&mut channel, ChannelPurpose::Control, None).await {
        return ChannelOutcome::Reconnect(error);
    }
    let Some(trusted_peer) = config.trusted_peer.as_ref() else {
        return ChannelOutcome::Reconnect("控制通道缺少可信设备信息".into());
    };
    let bulk = match connect_windows_bulk_channel(address, identity, trusted_peer, &channel).await {
        Ok(bulk) => bulk,
        Err(error) => return ChannelOutcome::Reconnect(error),
    };

    emit(
        events,
        RuntimeEvent::Online {
            session_id: channel.session_id().to_owned(),
            peer: channel.peer().clone(),
            endpoint: endpoint.clone(),
        },
    )
    .await;
    run_windows_session(channel, bulk, config, commands, mouse_movement, services).await
}

async fn run_windows_session(
    mut channel: SecureChannel,
    bulk: BulkChannel,
    config: &mut WindowsRuntimeConfig,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    mouse_movement: &mut MouseMovementReceiver,
    services: &WindowsRuntimeServices<'_>,
) -> ChannelOutcome {
    let WindowsRuntimeServices {
        store,
        clipboard,
        history,
        events,
    } = services;
    mouse_movement.clear();
    let (clipboard_updates, clipboard_update_rx) = mpsc::channel(8);
    let clipboard_worker = run_clipboard_worker(
        bulk,
        (*clipboard).clone(),
        store.data_dir().to_owned(),
        false,
        Some((*history).clone()),
        clipboard_update_rx,
        (*events).clone(),
    );
    tokio::pin!(clipboard_worker);
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let mut last_ack = Instant::now();
    let mut control_requested_at = None;

    loop {
        tokio::select! {
            biased;
            command = commands.recv() => {
                match command {
                    Some(RuntimeCommand::EnterControl { edge, position }) => {
                        mouse_movement.clear();
                        if let Err(error) = channel.send(ProtocolMessage::EnterControl { edge, position }).await {
                            return ChannelOutcome::Reconnect(error.to_string());
                        }
                        control_requested_at = Some(Instant::now());
                    }
                    Some(RuntimeCommand::ReturnControl { position }) => {
                        mouse_movement.clear();
                        control_requested_at = None;
                        if let Err(error) = channel.send(ProtocolMessage::ReturnControl { position }).await {
                            return ChannelOutcome::Reconnect(error.to_string());
                        }
                    }
                    Some(RuntimeCommand::SendInput(event)) => {
                        if let Err(error) = channel.send(ProtocolMessage::Input { event }).await {
                            return ChannelOutcome::Reconnect(error.to_string());
                        }
                    }
                    Some(RuntimeCommand::ReleaseAll) => {
                        mouse_movement.clear();
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                    }
                    Some(RuntimeCommand::ConfigureEndpoint(new_endpoint)) => {
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                        return ChannelOutcome::Reconfigure(new_endpoint);
                    }
                    Some(RuntimeCommand::ConfigureScreenEdge(edge)) => {
                        update_windows_edge(config, store, edge, events).await;
                    }
                    Some(RuntimeCommand::ConfigureTargetDisplay(display_id)) => {
                        update_windows_target_display(config, store, display_id, events).await;
                    }
                    Some(RuntimeCommand::ClipboardUpdated) => {
                        record_current_clipboard(clipboard, history, events).await;
                        let _ = clipboard_updates.try_send(());
                    }
                    Some(RuntimeCommand::ActivateClipboardHistory(id)) => {
                        activate_clipboard_history(&id, clipboard, history, events).await;
                        let _ = clipboard_updates.try_send(());
                    }
                    Some(RuntimeCommand::ClearClipboardHistory) => {
                        clear_clipboard_history(history, events).await;
                    }
                    Some(RuntimeCommand::SetControlEnabled(enabled)) => {
                        update_windows_control_enabled(config, store, enabled, events).await;
                    }
                    Some(RuntimeCommand::SetGameModeEnabled(enabled)) => {
                        update_windows_game_mode_enabled(config, store, enabled, events).await;
                    }
                    Some(RuntimeCommand::RecoverNow(trigger)) => {
                        emit_recovery_trigger(events, trigger).await;
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                        return ChannelOutcome::ReconnectNow(
                            "网络变化或系统唤醒，正在立即恢复连接".into(),
                        );
                    }
                    Some(RuntimeCommand::Disconnect) => {
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                        return ChannelOutcome::Reconnect("用户断开连接".into());
                    }
                    Some(RuntimeCommand::Shutdown) | None => {
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                        return ChannelOutcome::Shutdown;
                    }
                    _ => {}
                }
            }
            message = channel.receive() => {
                match message {
                    Ok(ProtocolMessage::HeartbeatAck { sent_at_unix_ms }) => {
                        last_ack = Instant::now();
                        let latency = unix_time_ms().saturating_sub(sent_at_unix_ms) as f32;
                        emit(events, RuntimeEvent::LatencyUpdated { milliseconds: latency }).await;
                    }
                    Ok(ProtocolMessage::EnterControlAck { accepted, reason }) => {
                        if control_requested_at.take().is_some() {
                            emit(events, RuntimeEvent::EnterControlResult { accepted, reason }).await;
                        }
                    }
                    Ok(ProtocolMessage::ReturnControl { position }) => {
                        control_requested_at = None;
                        let _ = channel.send(ProtocolMessage::ReturnControlAck).await;
                        emit(events, RuntimeEvent::ReturnControlRequested { position }).await;
                    }
                    Ok(ProtocolMessage::ReturnControlAck) => {
                        emit(events, RuntimeEvent::ReturnControlAcknowledged).await;
                    }
                    Ok(ProtocolMessage::RemotePause { paused }) => {
                        emit(events, RuntimeEvent::RemotePauseChanged { paused }).await;
                    }
                    Ok(ProtocolMessage::ReleaseAll) => {
                        emit(events, RuntimeEvent::ReleaseAllRequested).await;
                    }
                    Ok(_) => {}
                    Err(error) => return ChannelOutcome::Reconnect(error.to_string()),
                }
            }
            _ = heartbeat.tick() => {
                if last_ack.elapsed() >= HEARTBEAT_TIMEOUT {
                    let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                    return ChannelOutcome::Reconnect("心跳超时".into());
                }
                if control_requested_at
                    .is_some_and(|requested_at: Instant| requested_at.elapsed() >= CONTROL_REQUEST_TIMEOUT)
                {
                    control_requested_at = None;
                    emit(events, RuntimeEvent::EnterControlResult {
                        accepted: false,
                        reason: Some("等待 Mac 准备键鼠控制超时".into()),
                    }).await;
                }
                let sent_at_unix_ms = unix_time_ms();
                if let Err(error) = channel.send(ProtocolMessage::Heartbeat { sent_at_unix_ms }).await {
                    return ChannelOutcome::Reconnect(error.to_string());
                }
            }
            event = mouse_movement.receive() => {
                if let Err(error) = channel.send(ProtocolMessage::Input { event }).await {
                    return ChannelOutcome::Reconnect(error.to_string());
                }
            }
            result = &mut clipboard_worker => {
                return ChannelOutcome::Reconnect(
                    result.err().unwrap_or_else(|| "剪贴板通道已停止".into())
                );
            }
        }
    }
}

async fn run_macos_runtime(
    identity: DeviceIdentity,
    store: ConfigStore,
    mut config: MacRuntimeConfig,
    mut commands: mpsc::Receiver<RuntimeCommand>,
    clipboard: ClipboardMailbox,
    events: mpsc::Sender<RuntimeEvent>,
) {
    emit(
        &events,
        RuntimeEvent::TrustedPeerLoaded {
            peer: config.trusted_peer.clone(),
        },
    )
    .await;
    emit(
        &events,
        RuntimeEvent::LocalPolicyChanged {
            policy: config.local_policy,
        },
    )
    .await;
    let mut listener = match bind_macos_listener(&identity, &config).await {
        Ok(listener) => {
            emit(
                &events,
                RuntimeEvent::ListenerStarted {
                    ip: local_ipv4_address(),
                    port: listener
                        .local_addr()
                        .map(|address| address.port())
                        .unwrap_or(config.listen_port),
                },
            )
            .await;
            Some(listener)
        }
        Err(error) => {
            emit(
                &events,
                RuntimeEvent::ListenerFailed {
                    port: config.listen_port,
                    message: error.to_string(),
                },
            )
            .await;
            None
        }
    };

    loop {
        if listener.is_none() {
            match commands.recv().await {
                Some(RuntimeCommand::ConfigureListenPort(port)) => {
                    if !replace_macos_listener(
                        &identity,
                        &store,
                        &mut config,
                        &mut listener,
                        port,
                        &events,
                    )
                    .await
                    {
                        continue;
                    }
                }
                Some(RuntimeCommand::RecoverNow(trigger)) => {
                    emit_recovery_trigger(&events, trigger).await;
                    let port = config.listen_port;
                    replace_macos_listener(
                        &identity,
                        &store,
                        &mut config,
                        &mut listener,
                        port,
                        &events,
                    )
                    .await;
                }
                Some(RuntimeCommand::Shutdown) | None => return,
                _ => {}
            }
            continue;
        }

        tokio::select! {
            accepted = async {
                listener
                    .as_ref()
                    .expect("listener checked")
                    .accept()
                    .await
            } => {
                match accepted {
                    Ok((channel, remote_addr)) => {
                        let endpoint = NetworkEndpoint {
                            ip: remote_addr.ip().to_string(),
                            port: remote_addr.port(),
                        };
                        let services = MacChannelServices {
                            listener: listener.as_mut().expect("listener checked"),
                            store: &store,
                            clipboard: &clipboard,
                            events: &events,
                        };
                        match prepare_macos_channel(
                            channel,
                            endpoint,
                            &mut config,
                            &mut commands,
                            services,
                        ).await {
                            ChannelOutcome::Reconnect(reason) => {
                                emit(&events, RuntimeEvent::Offline { reason, retrying: false }).await;
                            }
                            ChannelOutcome::ReconnectNow(reason) => {
                                emit(&events, RuntimeEvent::Offline { reason, retrying: false }).await;
                            }
                            ChannelOutcome::Reconfigure(_) => {}
                            ChannelOutcome::Shutdown => return,
                        }
                    }
                    Err(error) => {
                        emit(&events, RuntimeEvent::RuntimeFailed { message: error.to_string() }).await;
                    }
                }
            }
            command = commands.recv() => {
                match command {
                    Some(RuntimeCommand::ConfigureListenPort(port)) => {
                        replace_macos_listener(
                            &identity,
                            &store,
                            &mut config,
                            &mut listener,
                            port,
                            &events,
                        ).await;
                    }
                    Some(RuntimeCommand::SetLocalPolicy(policy)) => {
                        config.local_policy = policy;
                        if let Err(error) = store.save_macos(&config) {
                            emit_runtime_failed(&events, error).await;
                        } else {
                            emit(&events, RuntimeEvent::ConfigUpdated).await;
                            emit(&events, RuntimeEvent::LocalPolicyChanged { policy }).await;
                        }
                    }
                    Some(RuntimeCommand::RecoverNow(trigger)) => {
                        emit_recovery_trigger(&events, trigger).await;
                        emit(
                            &events,
                            RuntimeEvent::ListenerStarted {
                                ip: local_ipv4_address(),
                                port: listener
                                    .as_ref()
                                    .expect("listener checked")
                                    .local_addr()
                                    .map(|address| address.port())
                                    .unwrap_or(config.listen_port),
                            },
                        )
                        .await;
                    }
                    Some(RuntimeCommand::Shutdown) | None => return,
                    _ => {}
                }
            }
        }
    }
}

async fn bind_macos_listener(
    identity: &DeviceIdentity,
    config: &MacRuntimeConfig,
) -> Result<SecureListener, TransportError> {
    SecureListener::bind(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, config.listen_port)),
        identity.clone(),
        config.trusted_peer.clone(),
    )
    .await
}

async fn replace_macos_listener(
    identity: &DeviceIdentity,
    store: &ConfigStore,
    config: &mut MacRuntimeConfig,
    listener: &mut Option<SecureListener>,
    port: u16,
    events: &mpsc::Sender<RuntimeEvent>,
) -> bool {
    if port < 1024 {
        emit(
            events,
            RuntimeEvent::ListenerFailed {
                port,
                message: "端口必须在 1024-65535 范围内".into(),
            },
        )
        .await;
        return false;
    }
    if listener
        .as_ref()
        .and_then(|listener| listener.local_addr().ok())
        .is_some_and(|address| address.port() == port)
    {
        return true;
    }
    let candidate_config = MacRuntimeConfig {
        listen_port: port,
        ..config.clone()
    };
    match bind_macos_listener(identity, &candidate_config).await {
        Ok(candidate) => {
            if let Err(error) = store.save_macos(&candidate_config) {
                emit_runtime_failed(events, error).await;
                return false;
            }
            *config = candidate_config;
            *listener = Some(candidate);
            emit(
                events,
                RuntimeEvent::ListenerStarted {
                    ip: local_ipv4_address(),
                    port,
                },
            )
            .await;
            emit(events, RuntimeEvent::ConfigUpdated).await;
            true
        }
        Err(error) => {
            emit(
                events,
                RuntimeEvent::ListenerFailed {
                    port,
                    message: error.to_string(),
                },
            )
            .await;
            false
        }
    }
}

async fn prepare_macos_channel(
    mut channel: SecureChannel,
    endpoint: NetworkEndpoint,
    config: &mut MacRuntimeConfig,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    services: MacChannelServices<'_>,
) -> ChannelOutcome {
    let MacChannelServices {
        listener,
        store,
        clipboard,
        events,
    } = services;
    let previous_trust = config.trusted_peer.clone();
    if let ChannelTrust::Pairing { code } = channel.trust().clone() {
        let paired = match run_pairing(&mut channel, code, commands, events).await {
            Ok(paired) => paired,
            Err(outcome) => return outcome,
        };
        config.trusted_peer = Some(paired);
        if let Err(error) = store.save_macos(config) {
            config.trusted_peer = previous_trust;
            emit_runtime_failed(events, error).await;
            return ChannelOutcome::Reconnect("无法保存配对信息".into());
        }
        if let Err(error) = exchange_session_ready(&mut channel).await {
            config.trusted_peer = previous_trust;
            let _ = store.save_macos(config);
            return ChannelOutcome::Reconnect(error);
        }
        listener.set_trusted_peer(config.trusted_peer.clone());
    }
    if let Err(error) = accept_channel(&mut channel, ChannelPurpose::Control, None).await {
        return ChannelOutcome::Reconnect(error);
    }
    let bulk = match accept_macos_bulk_channel(listener, &channel).await {
        Ok(bulk) => bulk,
        Err(error) => return ChannelOutcome::Reconnect(error),
    };

    emit(
        events,
        RuntimeEvent::Online {
            session_id: channel.session_id().to_owned(),
            peer: channel.peer().clone(),
            endpoint,
        },
    )
    .await;
    run_macos_session(channel, bulk, config, store, commands, clipboard, events).await
}

async fn run_macos_session(
    mut channel: SecureChannel,
    bulk: BulkChannel,
    config: &mut MacRuntimeConfig,
    store: &ConfigStore,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    clipboard: &ClipboardMailbox,
    events: &mpsc::Sender<RuntimeEvent>,
) -> ChannelOutcome {
    if let Err(error) = channel
        .send(ProtocolMessage::RemotePause {
            paused: config.local_policy == LocalControlPolicy::Paused,
        })
        .await
    {
        return ChannelOutcome::Reconnect(error.to_string());
    }
    let (clipboard_updates, clipboard_update_rx) = mpsc::channel(8);
    let clipboard_worker = run_clipboard_worker(
        bulk,
        clipboard.clone(),
        store.data_dir().to_owned(),
        true,
        None,
        clipboard_update_rx,
        events.clone(),
    );
    tokio::pin!(clipboard_worker);

    loop {
        tokio::select! {
            message = channel.receive() => {
                match message {
                    Ok(ProtocolMessage::Heartbeat { sent_at_unix_ms }) => {
                        if let Err(error) = channel.send(ProtocolMessage::HeartbeatAck { sent_at_unix_ms }).await {
                            return ChannelOutcome::Reconnect(error.to_string());
                        }
                    }
                    Ok(ProtocolMessage::EnterControl { edge, position }) => {
                        emit(events, RuntimeEvent::EnterControlRequested { edge, position }).await;
                    }
                    Ok(ProtocolMessage::ReturnControl { position }) => {
                        let _ = channel.send(ProtocolMessage::ReturnControlAck).await;
                        emit(events, RuntimeEvent::ReturnControlRequested { position }).await;
                    }
                    Ok(ProtocolMessage::ReturnControlAck) => {
                        emit(events, RuntimeEvent::ReturnControlAcknowledged).await;
                    }
                    Ok(ProtocolMessage::Input { event }) => {
                        emit(events, RuntimeEvent::InputReceived(event)).await;
                    }
                    Ok(ProtocolMessage::ReleaseAll) => {
                        emit(events, RuntimeEvent::ReleaseAllRequested).await;
                    }
                    Ok(ProtocolMessage::RemotePause { paused }) => {
                        emit(events, RuntimeEvent::RemotePauseChanged { paused }).await;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        emit(events, RuntimeEvent::ReleaseAllRequested).await;
                        return ChannelOutcome::Reconnect(error.to_string());
                    }
                }
            }
            command = commands.recv() => {
                match command {
                    Some(RuntimeCommand::RespondEnterControl { accepted, reason }) => {
                        if let Err(error) = channel.send(ProtocolMessage::EnterControlAck { accepted, reason }).await {
                            return ChannelOutcome::Reconnect(error.to_string());
                        }
                    }
                    Some(RuntimeCommand::ReturnControl { position }) => {
                        if let Err(error) = channel.send(ProtocolMessage::ReturnControl { position }).await {
                            return ChannelOutcome::Reconnect(error.to_string());
                        }
                    }
                    Some(RuntimeCommand::ReleaseAll) => {
                        emit(events, RuntimeEvent::ReleaseAllRequested).await;
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                    }
                    Some(RuntimeCommand::SetLocalPolicy(policy)) => {
                        config.local_policy = policy;
                        if let Err(error) = store.save_macos(config) {
                            emit_runtime_failed(events, error).await;
                        } else {
                            emit(events, RuntimeEvent::LocalPolicyChanged { policy }).await;
                        }
                        let paused = policy == LocalControlPolicy::Paused;
                        if paused {
                            emit(events, RuntimeEvent::ReleaseAllRequested).await;
                        }
                        if let Err(error) = channel.send(ProtocolMessage::RemotePause { paused }).await {
                            return ChannelOutcome::Reconnect(error.to_string());
                        }
                    }
                    Some(RuntimeCommand::ClipboardUpdated) => {
                        let _ = clipboard_updates.try_send(());
                    }
                    Some(RuntimeCommand::RecoverNow(trigger)) => {
                        emit_recovery_trigger(events, trigger).await;
                        emit(events, RuntimeEvent::ReleaseAllRequested).await;
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                        return ChannelOutcome::ReconnectNow(
                            "网络变化或系统唤醒，正在恢复监听".into(),
                        );
                    }
                    Some(RuntimeCommand::ConfigureListenPort(_)) => {
                        emit(events, RuntimeEvent::RuntimeFailed {
                            message: "活动会话期间不能修改监听端口".into(),
                        }).await;
                    }
                    Some(RuntimeCommand::Disconnect) => {
                        emit(events, RuntimeEvent::ReleaseAllRequested).await;
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                        return ChannelOutcome::Reconnect("用户断开连接".into());
                    }
                    Some(RuntimeCommand::Shutdown) | None => {
                        emit(events, RuntimeEvent::ReleaseAllRequested).await;
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                        return ChannelOutcome::Shutdown;
                    }
                    _ => {}
                }
            }
            result = &mut clipboard_worker => {
                return ChannelOutcome::Reconnect(
                    result.err().unwrap_or_else(|| "剪贴板通道已停止".into())
                );
            }
        }
    }
}

async fn run_pairing(
    channel: &mut SecureChannel,
    code: String,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    events: &mpsc::Sender<RuntimeEvent>,
) -> Result<TrustedPeer, ChannelOutcome> {
    let expires_at_unix_ms = unix_time_ms().saturating_add(PAIRING_TIMEOUT.as_millis() as u64);
    let peer = channel.peer().clone();
    let mut local_confirmed = false;
    let mut remote_confirmed = false;
    emit(
        events,
        RuntimeEvent::Pairing {
            code: code.clone(),
            peer: peer.clone(),
            local_confirmed,
            remote_confirmed,
            expires_at_unix_ms,
        },
    )
    .await;
    let timeout = tokio::time::sleep(PAIRING_TIMEOUT);
    tokio::pin!(timeout);

    while !local_confirmed || !remote_confirmed {
        tokio::select! {
            _ = &mut timeout => {
                return Err(ChannelOutcome::Reconnect("配对请求已过期".into()));
            }
            message = channel.receive() => {
                match message {
                    Ok(ProtocolMessage::PairingDecision { confirmed: true }) => {
                        remote_confirmed = true;
                        emit(events, RuntimeEvent::Pairing {
                            code: code.clone(),
                            peer: peer.clone(),
                            local_confirmed,
                            remote_confirmed,
                            expires_at_unix_ms,
                        }).await;
                    }
                    Ok(ProtocolMessage::PairingDecision { confirmed: false }) => {
                        emit(events, RuntimeEvent::PairingRejected { local: false }).await;
                        return Err(ChannelOutcome::Reconnect("远端拒绝配对".into()));
                    }
                    Ok(_) => {}
                    Err(error) => return Err(ChannelOutcome::Reconnect(error.to_string())),
                }
            }
            command = commands.recv() => {
                match command {
                    Some(RuntimeCommand::ConfirmPairing) if !local_confirmed => {
                        if let Err(error) = channel.send(ProtocolMessage::PairingDecision { confirmed: true }).await {
                            return Err(ChannelOutcome::Reconnect(error.to_string()));
                        }
                        local_confirmed = true;
                        emit(events, RuntimeEvent::Pairing {
                            code: code.clone(),
                            peer: peer.clone(),
                            local_confirmed,
                            remote_confirmed,
                            expires_at_unix_ms,
                        }).await;
                    }
                    Some(RuntimeCommand::RejectPairing) => {
                        let _ = channel.send(ProtocolMessage::PairingDecision { confirmed: false }).await;
                        emit(events, RuntimeEvent::PairingRejected { local: true }).await;
                        return Err(ChannelOutcome::Reconnect("本机拒绝配对".into()));
                    }
                    Some(RuntimeCommand::RecoverNow(trigger)) => {
                        emit_recovery_trigger(events, trigger).await;
                        let _ = channel.send(ProtocolMessage::PairingDecision { confirmed: false }).await;
                        return Err(ChannelOutcome::ReconnectNow(
                            "网络变化或系统唤醒，配对会话已重新开始".into(),
                        ));
                    }
                    Some(RuntimeCommand::Shutdown) | None => {
                        let _ = channel.send(ProtocolMessage::PairingDecision { confirmed: false }).await;
                        return Err(ChannelOutcome::Shutdown);
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(peer)
}

async fn exchange_session_ready(channel: &mut SecureChannel) -> Result<(), String> {
    channel
        .send(ProtocolMessage::SessionReady)
        .await
        .map_err(|error| error.to_string())?;
    match tokio::time::timeout(Duration::from_secs(5), channel.receive()).await {
        Ok(Ok(ProtocolMessage::SessionReady)) => Ok(()),
        Ok(Ok(_)) => Err("配对完成后收到意外消息".into()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("等待远端进入会话超时".into()),
    }
}

async fn open_channel(
    channel: &mut SecureChannel,
    purpose: ChannelPurpose,
    control_session_id: Option<String>,
) -> Result<(), String> {
    channel
        .send(ProtocolMessage::OpenChannel {
            purpose,
            control_session_id,
        })
        .await
        .map_err(|error| error.to_string())?;
    match tokio::time::timeout(CONNECT_TIMEOUT, channel.receive()).await {
        Ok(Ok(ProtocolMessage::ChannelReady {
            purpose: remote_purpose,
        })) if remote_purpose == purpose => Ok(()),
        Ok(Ok(_)) => Err("通道建立时收到意外消息".into()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("等待远端建立通道超时".into()),
    }
}

async fn accept_channel(
    channel: &mut SecureChannel,
    purpose: ChannelPurpose,
    control_session_id: Option<&str>,
) -> Result<(), String> {
    match tokio::time::timeout(CONNECT_TIMEOUT, channel.receive()).await {
        Ok(Ok(ProtocolMessage::OpenChannel {
            purpose: remote_purpose,
            control_session_id: remote_control_session_id,
        })) if remote_purpose == purpose
            && remote_control_session_id.as_deref() == control_session_id => {}
        Ok(Ok(_)) => return Err("远端请求了无效通道".into()),
        Ok(Err(error)) => return Err(error.to_string()),
        Err(_) => return Err("等待远端声明通道用途超时".into()),
    }
    channel
        .send(ProtocolMessage::ChannelReady { purpose })
        .await
        .map_err(|error| error.to_string())
}

async fn connect_windows_bulk_channel(
    address: SocketAddr,
    identity: &DeviceIdentity,
    trusted_peer: &TrustedPeer,
    control_channel: &SecureChannel,
) -> Result<BulkChannel, String> {
    let mut channel = tokio::time::timeout(
        CONNECT_TIMEOUT,
        connect(address, identity, Some(trusted_peer)),
    )
    .await
    .map_err(|_| "连接 Mac 剪贴板通道超时".to_owned())?
    .map_err(|error| error.to_string())?;
    if channel.trust() != &ChannelTrust::Authenticated
        || channel.peer().device_id != control_channel.peer().device_id
    {
        return Err("剪贴板通道设备身份不匹配".into());
    }
    open_channel(
        &mut channel,
        ChannelPurpose::Clipboard,
        Some(control_channel.session_id().to_owned()),
    )
    .await?;
    Ok(BulkChannel::from_secure(channel))
}

async fn accept_macos_bulk_channel(
    listener: &SecureListener,
    control_channel: &SecureChannel,
) -> Result<BulkChannel, String> {
    let (mut channel, _) = tokio::time::timeout(CONNECT_TIMEOUT, listener.accept())
        .await
        .map_err(|_| "等待 Windows 剪贴板通道超时".to_owned())?
        .map_err(|error| error.to_string())?;
    if channel.trust() != &ChannelTrust::Authenticated
        || channel.peer().device_id != control_channel.peer().device_id
    {
        return Err("剪贴板通道设备身份不匹配".into());
    }
    accept_channel(
        &mut channel,
        ChannelPurpose::Clipboard,
        Some(control_channel.session_id()),
    )
    .await?;
    Ok(BulkChannel::from_secure(channel))
}

fn endpoint_to_socket_addr(endpoint: &NetworkEndpoint) -> Result<SocketAddr, String> {
    endpoint.validate().map_err(str::to_owned)?;
    let ip = endpoint
        .ip
        .parse::<Ipv4Addr>()
        .map_err(|_| "IP 地址必须是有效的 IPv4 地址".to_owned())?;
    Ok(SocketAddr::V4(SocketAddrV4::new(ip, endpoint.port)))
}

#[derive(Debug)]
struct InterfaceIpv4Candidate {
    name: String,
    address: Ipv4Addr,
    is_up: bool,
    is_loopback: bool,
    is_virtual: bool,
}

fn local_ipv4_address() -> String {
    local_ipv4_candidates()
        .into_iter()
        .filter_map(|candidate| {
            interface_ipv4_score(&candidate).map(|score| (score, candidate.address))
        })
        .max_by_key(|(score, _)| *score)
        .map(|(_, address)| address.to_string())
        .unwrap_or_else(|| Ipv4Addr::UNSPECIFIED.to_string())
}

fn interface_ipv4_score(candidate: &InterfaceIpv4Candidate) -> Option<u16> {
    let octets = candidate.address.octets();
    let benchmark_network = octets[0] == 198 && matches!(octets[1], 18 | 19);
    if !candidate.is_up
        || candidate.is_loopback
        || candidate.is_virtual
        || candidate.address.is_unspecified()
        || candidate.address.is_loopback()
        || candidate.address.is_link_local()
        || candidate.address.is_multicast()
        || benchmark_network
    {
        return None;
    }

    let mut score = 1;
    if candidate.address.is_private() {
        score += 100;
    }
    if candidate.name == "en0" {
        score += 50;
    } else if candidate.name.starts_with("en")
        || candidate.name.starts_with("eth")
        || candidate.name.starts_with("wlan")
    {
        score += 40;
    }
    Some(score)
}

fn local_ipv4_candidates() -> Vec<InterfaceIpv4Candidate> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|interface| {
            let is_up = interface.is_oper_up();
            let if_addrs::IfAddr::V4(address) = interface.addr else {
                return None;
            };
            let name = interface.name;
            let is_virtual = is_virtual_interface_name(&name);
            Some(InterfaceIpv4Candidate {
                is_up,
                is_loopback: address.ip.is_loopback(),
                is_virtual,
                name,
                address: address.ip,
            })
        })
        .collect()
}

fn is_virtual_interface_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "utun",
        "tun",
        "tap",
        "ppp",
        "ipsec",
        "gif",
        "stf",
        "awdl",
        "llw",
        "vethernet",
        "virtual",
        "vmware",
        "hyper-v",
        "tailscale",
        "wireguard",
        "tunnel",
        "loopback",
    ]
    .iter()
    .any(|marker| name.contains(marker))
}

fn network_signature() -> Vec<Ipv4Addr> {
    let mut addresses: Vec<_> = local_ipv4_candidates()
        .into_iter()
        .filter(|candidate| interface_ipv4_score(candidate).is_some())
        .map(|candidate| candidate.address)
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    addresses
}

fn spawn_network_change_watcher(commands: mpsc::Sender<RuntimeCommand>) {
    tokio::spawn(async move {
        let mut previous = network_signature();
        let mut poll = tokio::time::interval(NETWORK_CHANGE_POLL_INTERVAL);
        poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
        poll.tick().await;
        loop {
            poll.tick().await;
            if commands.is_closed() {
                return;
            }
            let current = network_signature();
            if current == previous {
                continue;
            }
            let network_available = !current.is_empty();
            tracing::info!(
                component = "connection",
                network_available,
                "local network interface state changed"
            );
            previous = current;
            if network_available
                && commands
                    .send(RuntimeCommand::RecoverNow(RecoveryTrigger::NetworkChanged))
                    .await
                    .is_err()
            {
                return;
            }
        }
    });
}

async fn emit(events: &mpsc::Sender<RuntimeEvent>, event: RuntimeEvent) {
    let _ = events.send(event).await;
}

async fn emit_connection_failure(
    events: &mpsc::Sender<RuntimeEvent>,
    attempt_id: u64,
    failure_count: u32,
    error: TransportError,
) {
    emit(
        events,
        RuntimeEvent::ConnectionFailed {
            attempt_id,
            failure_count,
            message: error.to_string(),
        },
    )
    .await;
}

async fn emit_recovery_trigger(events: &mpsc::Sender<RuntimeEvent>, trigger: RecoveryTrigger) {
    tracing::info!(component = "connection", ?trigger, "KVM recovery triggered");
    emit(events, RuntimeEvent::RecoveryTriggered { trigger }).await;
}

async fn emit_runtime_failed(events: &mpsc::Sender<RuntimeEvent>, error: impl std::fmt::Display) {
    emit(
        events,
        RuntimeEvent::RuntimeFailed {
            message: error.to_string(),
        },
    )
    .await;
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

enum ChannelOutcome {
    Reconnect(String),
    ReconnectNow(String),
    Reconfigure(NetworkEndpoint),
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvmflow_domain::{KeyAction, KeyUsage};

    #[tokio::test]
    async fn selected_windows_display_is_persisted() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        let store = ConfigStore::new(directory.path());
        let mut config = WindowsRuntimeConfig::default();
        let (events, mut received_events) = mpsc::channel(4);

        update_windows_target_display(&mut config, &store, r"\\.\DISPLAY2".into(), &events).await;

        assert_eq!(config.target_display_id.as_deref(), Some(r"\\.\DISPLAY2"));
        assert_eq!(
            store
                .load_windows()
                .expect("persisted Windows config")
                .target_display_id
                .as_deref(),
            Some(r"\\.\DISPLAY2")
        );
        assert!(matches!(
            received_events.recv().await,
            Some(RuntimeEvent::ConfigUpdated)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_command_skips_the_remaining_windows_backoff() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        let store = ConfigStore::new(directory.path());
        let mut config = WindowsRuntimeConfig::default();
        let clipboard = ClipboardMailbox::default();
        let history = ClipboardHistoryStore::new(store.data_dir());
        let (command_tx, mut commands) = mpsc::channel(1);
        let (events, _received_events) = mpsc::channel(1);
        let services = WindowsRuntimeServices {
            store: &store,
            clipboard: &clipboard,
            history: &history,
            events: &events,
        };

        command_tx
            .send(RuntimeCommand::RecoverNow(RecoveryTrigger::SystemResumed))
            .await
            .expect("queue immediate recovery");

        let started_at = Instant::now();
        assert!(windows_backoff(&mut config, &mut commands, &services).await);
        assert_eq!(Instant::now(), started_at);
    }

    #[test]
    fn pending_mouse_movement_preserves_the_accumulated_delta() {
        let mut pending = PendingMouseMovement::default();

        pending.push(8, -3);
        pending.push(5, 7);
        pending.push(-2, -1);

        assert_eq!(
            pending.take_chunk(),
            Some(InputEvent::MouseMove { dx: 11, dy: 3 })
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn pending_mouse_movement_keeps_overflow_for_the_next_chunk() {
        let mut pending = PendingMouseMovement::default();

        pending.push(i32::MAX, 0);
        pending.push(i32::MAX, 0);

        assert_eq!(
            pending.take_chunk(),
            Some(InputEvent::MouseMove {
                dx: i32::MAX,
                dy: 0
            })
        );
        assert_eq!(
            pending.take_chunk(),
            Some(InputEvent::MouseMove {
                dx: i32::MAX,
                dy: 0
            })
        );
        assert!(pending.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn runtime_handle_coalesces_mouse_moves_outside_the_reliable_mailbox() {
        let (command_tx, mut commands) = mpsc::channel(1);
        let (mouse_movement_tx, mut mouse_movement_rx) = mouse_movement_channel();
        let handle = RuntimeHandle {
            command_tx,
            mouse_movement: Some(mouse_movement_tx),
            clipboard: ClipboardMailbox::default(),
        };

        handle
            .send(RuntimeCommand::SendInput(InputEvent::MouseMove {
                dx: 4,
                dy: -2,
            }))
            .await
            .expect("first movement");
        handle
            .send(RuntimeCommand::SendInput(InputEvent::MouseMove {
                dx: 6,
                dy: 5,
            }))
            .await
            .expect("second movement");

        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(
            mouse_movement_rx.receive().await,
            InputEvent::MouseMove { dx: 10, dy: 3 }
        );
        assert_eq!(
            mouse_movement_rx
                .state
                .captured_events
                .load(Ordering::Relaxed),
            2
        );
        assert_eq!(
            mouse_movement_rx
                .state
                .emitted_events
                .load(Ordering::Relaxed),
            1
        );

        let key = InputEvent::Key {
            usage: KeyUsage::A,
            action: KeyAction::Down,
        };
        handle
            .send(RuntimeCommand::SendInput(key.clone()))
            .await
            .expect("reliable key");
        assert!(matches!(
            commands.recv().await,
            Some(RuntimeCommand::SendInput(received)) if received == key
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn mouse_movement_is_emitted_no_faster_than_the_flush_interval() {
        let (mouse_movement_tx, mut mouse_movement_rx) = mouse_movement_channel();

        mouse_movement_tx.push(1, 0);
        assert_eq!(
            mouse_movement_rx.receive().await,
            InputEvent::MouseMove { dx: 1, dy: 0 }
        );

        mouse_movement_tx.push(2, 0);
        let second_movement = mouse_movement_rx.receive();
        tokio::pin!(second_movement);
        tokio::select! {
            _ = &mut second_movement => panic!("movement emitted before the flush interval"),
            _ = tokio::time::sleep(MOUSE_MOVEMENT_FLUSH_INTERVAL - Duration::from_millis(1)) => {}
        }
        assert_eq!(
            second_movement.await,
            InputEvent::MouseMove { dx: 2, dy: 0 }
        );
    }

    #[tokio::test]
    async fn runtime_handle_rejects_mouse_movement_after_runtime_stops() {
        let (command_tx, commands) = mpsc::channel(1);
        let (mouse_movement_tx, _mouse_movement_rx) = mouse_movement_channel();
        let handle = RuntimeHandle {
            command_tx,
            mouse_movement: Some(mouse_movement_tx),
            clipboard: ClipboardMailbox::default(),
        };
        drop(commands);

        assert!(matches!(
            handle
                .send(RuntimeCommand::SendInput(InputEvent::MouseMove {
                    dx: 1,
                    dy: 1,
                }))
                .await,
            Err(RuntimeSendError::Stopped)
        ));
    }

    #[test]
    fn endpoint_conversion_rejects_hostnames_and_privileged_ports() {
        assert!(
            endpoint_to_socket_addr(&NetworkEndpoint {
                ip: "mac.local".into(),
                port: 45_888,
            })
            .is_err()
        );
        assert!(
            endpoint_to_socket_addr(&NetworkEndpoint {
                ip: "127.0.0.1".into(),
                port: 80,
            })
            .is_err()
        );
    }

    #[test]
    fn interface_selection_rejects_virtual_and_non_lan_addresses() {
        for candidate in [
            candidate("utun1024", "198.18.0.1", true, false),
            candidate("lo0", "127.0.0.1", false, true),
            candidate("en0", "169.254.10.20", false, false),
        ] {
            assert_eq!(interface_ipv4_score(&candidate), None);
        }
    }

    #[test]
    fn interface_selection_prefers_private_physical_networks() {
        let physical = candidate("en0", "192.168.50.10", false, false);
        let secondary = candidate("bridge0", "172.16.0.2", false, false);

        assert!(
            interface_ipv4_score(&physical).expect("physical score")
                > interface_ipv4_score(&secondary).expect("secondary score")
        );
    }

    #[test]
    fn virtual_interface_names_are_excluded_from_recovery_signatures() {
        for name in [
            "utun4",
            "vEthernet (WSL)",
            "Tailscale",
            "VMware Network Adapter",
        ] {
            assert!(is_virtual_interface_name(name), "{name}");
        }
        for name in ["en0", "Ethernet", "Wi-Fi", "wlan0"] {
            assert!(!is_virtual_interface_name(name), "{name}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_interface_enumeration_does_not_select_the_benchmark_tunnel() {
        let selected = local_ipv4_address();

        assert!(!selected.starts_with("198.18."));
        assert!(!selected.starts_with("198.19."));
        if let Some(en0) = local_ipv4_candidates()
            .into_iter()
            .find(|candidate| candidate.name == "en0" && candidate.address.is_private())
        {
            assert_eq!(selected, en0.address.to_string());
        }
    }

    fn candidate(
        name: &str,
        address: &str,
        is_point_to_point: bool,
        is_loopback: bool,
    ) -> InterfaceIpv4Candidate {
        InterfaceIpv4Candidate {
            name: name.into(),
            address: address.parse().expect("test IPv4 address"),
            is_up: true,
            is_loopback,
            is_virtual: is_point_to_point,
        }
    }
}
