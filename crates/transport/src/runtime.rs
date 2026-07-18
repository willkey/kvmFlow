use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kvmflow_domain::{InputEvent, LocalControlPolicy, NetworkEndpoint, ScreenEdge};
use kvmflow_protocol::ProtocolMessage;
use tokio::{
    sync::{Notify, mpsc},
    time::{Instant, MissedTickBehavior},
};

use crate::{
    ChannelTrust, ConfigStore, DeviceIdentity, MacRuntimeConfig, SecureChannel, SecureListener,
    TransportError, TrustedPeer, WindowsRuntimeConfig, connect,
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

#[derive(Debug)]
pub enum RuntimeCommand {
    ConfigureEndpoint(NetworkEndpoint),
    ConfigureScreenEdge(ScreenEdge),
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
    SetLocalPolicy(LocalControlPolicy),
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
    tokio::spawn(run_windows_runtime(
        identity,
        store,
        config,
        command_rx,
        mouse_movement_rx,
        event_tx,
    ));
    RuntimeBridge {
        handle: RuntimeHandle {
            command_tx,
            mouse_movement: Some(mouse_movement_tx),
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
    tokio::spawn(run_macos_runtime(
        identity, store, config, command_rx, event_tx,
    ));
    RuntimeBridge {
        handle: RuntimeHandle {
            command_tx,
            mouse_movement: None,
        },
        events: event_rx,
    }
}

fn clears_pending_mouse_movement(command: &RuntimeCommand) -> bool {
    matches!(
        command,
        RuntimeCommand::ConfigureEndpoint(_)
            | RuntimeCommand::EnterControl { .. }
            | RuntimeCommand::ReturnControl { .. }
            | RuntimeCommand::ReleaseAll
            | RuntimeCommand::Disconnect
            | RuntimeCommand::Shutdown
            | RuntimeCommand::SetLocalPolicy(LocalControlPolicy::Paused)
    )
}

#[derive(Debug, Default)]
struct PendingMouseMovement {
    dx: i64,
    dy: i64,
}

impl PendingMouseMovement {
    fn push(&mut self, dx: i32, dy: i32) {
        self.dx = self.dx.saturating_add(i64::from(dx));
        self.dy = self.dy.saturating_add(i64::from(dy));
    }

    fn take_chunk(&mut self) -> Option<InputEvent> {
        let dx = self.dx.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        let dy = self.dy.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        self.dx -= i64::from(dx);
        self.dy -= i64::from(dy);
        (dx != 0 || dy != 0).then_some(InputEvent::MouseMove { dx, dy })
    }

    fn is_empty(&self) -> bool {
        self.dx == 0 && self.dy == 0
    }

    fn clear(&mut self) {
        self.dx = 0;
        self.dy = 0;
    }
}

#[derive(Debug, Default)]
struct MouseMovementState {
    pending: Mutex<PendingMouseMovement>,
    notify: Notify,
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
        self.with_pending(|pending| pending.push(dx, dy));
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
        pending.take_chunk()
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
    events: mpsc::Sender<RuntimeEvent>,
) {
    let mut attempt_id = 0_u64;
    let mut failure_count = 0_u32;
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

    loop {
        let Some(endpoint) =
            wait_for_windows_endpoint(&mut config, &store, &mut commands, &events).await
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
                if !windows_backoff(&mut config, &store, &mut commands, &events).await {
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
                if !windows_backoff(&mut config, &store, &mut commands, &events).await {
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
                if !windows_backoff(&mut config, &store, &mut commands, &events).await {
                    return;
                }
                continue;
            }
        };
        failure_count = 0;

        let outcome = prepare_windows_channel(
            channel,
            endpoint.clone(),
            &store,
            &mut config,
            &mut commands,
            &mut mouse_movement,
            &events,
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
                if !windows_backoff(&mut config, &store, &mut commands, &events).await {
                    return;
                }
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
    store: &ConfigStore,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    events: &mpsc::Sender<RuntimeEvent>,
) -> Option<NetworkEndpoint> {
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
            RuntimeCommand::Shutdown => return None,
            _ => {}
        }
    }
}

async fn windows_backoff(
    config: &mut WindowsRuntimeConfig,
    store: &ConfigStore,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    events: &mpsc::Sender<RuntimeEvent>,
) -> bool {
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

async fn prepare_windows_channel(
    mut channel: SecureChannel,
    endpoint: NetworkEndpoint,
    store: &ConfigStore,
    config: &mut WindowsRuntimeConfig,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    mouse_movement: &mut MouseMovementReceiver,
    events: &mpsc::Sender<RuntimeEvent>,
) -> ChannelOutcome {
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

    emit(
        events,
        RuntimeEvent::Online {
            session_id: channel.session_id().to_owned(),
            peer: channel.peer().clone(),
            endpoint: endpoint.clone(),
        },
    )
    .await;
    run_windows_session(channel, config, store, commands, mouse_movement, events).await
}

async fn run_windows_session(
    mut channel: SecureChannel,
    config: &mut WindowsRuntimeConfig,
    store: &ConfigStore,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    mouse_movement: &mut MouseMovementReceiver,
    events: &mpsc::Sender<RuntimeEvent>,
) -> ChannelOutcome {
    mouse_movement.clear();
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
                        if let Err(error) = channel.send(ProtocolMessage::EnterControl { edge, position }).await {
                            return ChannelOutcome::Reconnect(error.to_string());
                        }
                        control_requested_at = Some(Instant::now());
                    }
                    Some(RuntimeCommand::ReturnControl { position }) => {
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
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                    }
                    Some(RuntimeCommand::ConfigureEndpoint(new_endpoint)) => {
                        let _ = channel.send(ProtocolMessage::ReleaseAll).await;
                        return ChannelOutcome::Reconfigure(new_endpoint);
                    }
                    Some(RuntimeCommand::ConfigureScreenEdge(edge)) => {
                        update_windows_edge(config, store, edge, events).await;
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
        }
    }
}

async fn run_macos_runtime(
    identity: DeviceIdentity,
    store: ConfigStore,
    mut config: MacRuntimeConfig,
    mut commands: mpsc::Receiver<RuntimeCommand>,
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
                Some(RuntimeCommand::Shutdown) | None => return,
                _ => {}
            }
            continue;
        }

        let active_listener = listener.as_ref().expect("listener checked");
        tokio::select! {
            accepted = active_listener.accept() => {
                match accepted {
                    Ok((channel, remote_addr)) => {
                        let endpoint = NetworkEndpoint {
                            ip: remote_addr.ip().to_string(),
                            port: remote_addr.port(),
                        };
                        let previous_trust = config.trusted_peer.clone();
                        match prepare_macos_channel(
                            channel,
                            endpoint,
                            &store,
                            &mut config,
                            &mut commands,
                            &events,
                        ).await {
                            ChannelOutcome::Reconnect(reason) => {
                                emit(&events, RuntimeEvent::Offline { reason, retrying: false }).await;
                            }
                            ChannelOutcome::Reconfigure(_) => {}
                            ChannelOutcome::Shutdown => return,
                        }
                        if config.trusted_peer != previous_trust
                            && let Some(listener) = listener.as_mut()
                        {
                            listener.set_trusted_peer(config.trusted_peer.clone());
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
    store: &ConfigStore,
    config: &mut MacRuntimeConfig,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    events: &mpsc::Sender<RuntimeEvent>,
) -> ChannelOutcome {
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
    }

    emit(
        events,
        RuntimeEvent::Online {
            session_id: channel.session_id().to_owned(),
            peer: channel.peer().clone(),
            endpoint,
        },
    )
    .await;
    run_macos_session(channel, config, store, commands, events).await
}

async fn run_macos_session(
    mut channel: SecureChannel,
    config: &mut MacRuntimeConfig,
    store: &ConfigStore,
    commands: &mut mpsc::Receiver<RuntimeCommand>,
    events: &mpsc::Sender<RuntimeEvent>,
) -> ChannelOutcome {
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

#[cfg(target_os = "macos")]
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
            let is_virtual = [
                "utun", "tun", "tap", "ppp", "ipsec", "gif", "stf", "awdl", "llw",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix));
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

#[cfg(not(target_os = "macos"))]
fn local_ipv4_candidates() -> Vec<InterfaceIpv4Candidate> {
    Vec::new()
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
    Reconfigure(NetworkEndpoint),
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvmflow_domain::{KeyAction, KeyUsage};

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
