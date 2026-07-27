use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kvmflow_diagnostics::{
    KvmValidationEvent, ValidationMonitor, ValidationReportStore, ValidationStoreError,
};
use kvmflow_domain::{
    AppSnapshot, BuildInfo, ClipboardTransferState, ConnectionError, ConnectionErrorStage,
    ControlState, DiagnosticNotice, EndpointRole, EngineEffect, EngineEvent, InputEvent,
    KeyboardMonitorState, LinkState, LocalControlPolicy, NetworkEndpoint, PairingStatus,
    PermissionStatus, RuntimeState, ScreenEdge, TrustState, reduce,
};
use kvmflow_platform_api::{
    PlatformBridge, PlatformCommand, PlatformEvent, PlatformHandle, ResourceSampler,
};
use kvmflow_transport::{
    RecoveryTrigger, RuntimeBridge, RuntimeCommand, RuntimeEvent, RuntimeHandle,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, MissedTickBehavior},
};

const COMMAND_MAILBOX_CAPACITY: usize = 32;

pub struct EngineConfig {
    pub role: EndpointRole,
    pub build: BuildInfo,
    pub diagnostic_notice: Option<DiagnosticNotice>,
    pub sample_interval: Duration,
    pub persist_interval: Duration,
}

pub enum EngineCommand {
    WindowOpened,
    WindowClosed,
    ExportReport {
        reply: oneshot::Sender<Result<PathBuf, EngineError>>,
    },
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
    ReturnControl,
    SetControlEnabled(bool),
    SetGameModeEnabled(bool),
    ActivateClipboardHistory(String),
    ClearClipboardHistory,
    SetLocalPolicy(LocalControlPolicy),
    PermissionUpdated(PermissionStatus),
    RefreshPermission,
    OpenAccessibilitySettings,
    RecoverConnectionNow,
    ShutdownRequested {
        reply: oneshot::Sender<Result<(), EngineError>>,
    },
}

#[derive(Clone)]
pub struct EngineHandle {
    command_tx: mpsc::Sender<EngineCommand>,
    snapshot_rx: watch::Receiver<AppSnapshot>,
    keyboard_rx: watch::Receiver<KeyboardMonitorState>,
}

impl EngineHandle {
    pub fn current_snapshot(&self) -> AppSnapshot {
        self.snapshot_rx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<AppSnapshot> {
        self.snapshot_rx.clone()
    }

    pub fn current_keyboard_monitor(&self) -> KeyboardMonitorState {
        self.keyboard_rx.borrow().clone()
    }

    pub fn subscribe_keyboard(&self) -> watch::Receiver<KeyboardMonitorState> {
        self.keyboard_rx.clone()
    }

    pub async fn send(&self, command: EngineCommand) -> Result<(), EngineError> {
        self.command_tx
            .send(command)
            .await
            .map_err(|_| EngineError::Stopped)
    }

    pub async fn export_report(&self) -> Result<PathBuf, EngineError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(EngineCommand::ExportReport { reply: reply_tx })
            .await?;
        reply_rx.await.map_err(|_| EngineError::Stopped)?
    }

    pub async fn shutdown(&self) -> Result<(), EngineError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(EngineCommand::ShutdownRequested { reply: reply_tx })
            .await?;
        reply_rx.await.map_err(|_| EngineError::Stopped)?
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine actor has stopped")]
    Stopped,
    #[error("validation report persistence failed: {0}")]
    Report(#[from] ValidationStoreError),
}

pub fn spawn_engine(
    config: EngineConfig,
    sampler: Box<dyn ResourceSampler>,
    monitor: ValidationMonitor,
    store: Box<dyn ValidationReportStore>,
) -> EngineHandle {
    spawn_engine_inner(config, sampler, monitor, store, None, None)
}

pub fn spawn_engine_with_runtime(
    config: EngineConfig,
    sampler: Box<dyn ResourceSampler>,
    monitor: ValidationMonitor,
    store: Box<dyn ValidationReportStore>,
    runtime: RuntimeBridge,
) -> EngineHandle {
    spawn_engine_inner(config, sampler, monitor, store, Some(runtime), None)
}

pub fn spawn_engine_with_kvm(
    config: EngineConfig,
    sampler: Box<dyn ResourceSampler>,
    monitor: ValidationMonitor,
    store: Box<dyn ValidationReportStore>,
    runtime: RuntimeBridge,
    platform: PlatformBridge,
) -> EngineHandle {
    spawn_engine_inner(
        config,
        sampler,
        monitor,
        store,
        Some(runtime),
        Some(platform),
    )
}

fn spawn_engine_inner(
    config: EngineConfig,
    sampler: Box<dyn ResourceSampler>,
    monitor: ValidationMonitor,
    store: Box<dyn ValidationReportStore>,
    runtime: Option<RuntimeBridge>,
    platform: Option<PlatformBridge>,
) -> EngineHandle {
    let state = RuntimeState::new(config.role, config.build, config.diagnostic_notice);
    let (snapshot_tx, snapshot_rx) = watch::channel(AppSnapshot::from(&state));
    let (keyboard_tx, keyboard_rx) = watch::channel(KeyboardMonitorState::default());
    let (command_tx, command_rx) = mpsc::channel(COMMAND_MAILBOX_CAPACITY);
    let (runtime_handle, runtime_events) = match runtime {
        Some(runtime) => (Some(runtime.handle), Some(runtime.events)),
        None => (None, None),
    };
    let (platform_handle, platform_events) = match platform {
        Some(platform) => (Some(platform.handle), Some(platform.events)),
        None => (None, None),
    };

    tokio::spawn(run_actor(
        state,
        sampler,
        monitor,
        store,
        command_rx,
        snapshot_tx,
        keyboard_tx,
        config.sample_interval,
        config.persist_interval,
        runtime_handle,
        runtime_events,
        platform_handle,
        platform_events,
    ));

    EngineHandle {
        command_tx,
        snapshot_rx,
        keyboard_rx,
    }
}

// Keep the actor's runtime dependency wiring explicit at this private boundary.
#[allow(clippy::too_many_arguments)]
async fn run_actor(
    mut state: RuntimeState,
    mut sampler: Box<dyn ResourceSampler>,
    mut monitor: ValidationMonitor,
    store: Box<dyn ValidationReportStore>,
    mut command_rx: mpsc::Receiver<EngineCommand>,
    snapshot_tx: watch::Sender<AppSnapshot>,
    keyboard_tx: watch::Sender<KeyboardMonitorState>,
    sample_interval: Duration,
    persist_interval: Duration,
    runtime_handle: Option<RuntimeHandle>,
    mut runtime_events: Option<mpsc::Receiver<RuntimeEvent>>,
    platform_handle: Option<PlatformHandle>,
    mut platform_events: Option<mpsc::Receiver<PlatformEvent>>,
) {
    apply_event(&mut state, &snapshot_tx, EngineEvent::Started);

    let mut sample_timer = interval_after(sample_interval);
    let mut persist_timer = interval_after(persist_interval);

    loop {
        tokio::select! {
            biased;
            _ = sample_timer.tick() => {
                match sampler.sample(state.window_state) {
                    Ok(sample) => {
                        let validation = monitor.record(sample);
                        apply_event(
                            &mut state,
                            &snapshot_tx,
                            EngineEvent::ResourceUpdated { sample, validation },
                        );
                    }
                    Err(error) => {
                        tracing::error!(component = "engine", %error, "resource sampling failed");
                    }
                }
            }
            _ = persist_timer.tick() => {
                if let Err(error) = persist_report(&monitor, store.as_ref()).await {
                    tracing::error!(component = "engine", %error, "periodic validation report persistence failed");
                }
            }
            event = receive_runtime_event(&mut runtime_events) => {
                if let Some(event) = event {
                    update_keyboard_monitor_for_runtime_event(&keyboard_tx, &event);
                    record_runtime_validation(&mut state, &mut monitor, &event);
                    let actions = apply_runtime_event(&mut state, &snapshot_tx, event);
                    for command in actions.runtime {
                        send_runtime_command(runtime_handle.as_ref(), command).await;
                    }
                    for command in actions.platform {
                        send_platform_command(platform_handle.as_ref(), command).await;
                    }
                } else {
                    runtime_events = None;
                }
            }
            event = receive_platform_event(&mut platform_events) => {
                if let Some(event) = event {
                    update_keyboard_monitor_for_platform_event(&state, &keyboard_tx, &event);
                    record_platform_validation(&mut state, &mut monitor, &event);
                    let actions = apply_platform_event(&mut state, &snapshot_tx, event);
                    for command in actions.runtime {
                        send_runtime_command(runtime_handle.as_ref(), command).await;
                    }
                    for command in actions.platform {
                        send_platform_command(platform_handle.as_ref(), command).await;
                    }
                } else {
                    platform_events = None;
                }
            }
            command = command_rx.recv() => {
                let Some(command) = command else {
                    return;
                };

                match command {
                    EngineCommand::WindowOpened => {
                        apply_event(&mut state, &snapshot_tx, EngineEvent::WindowOpened);
                    }
                    EngineCommand::WindowClosed => {
                        apply_event(&mut state, &snapshot_tx, EngineEvent::WindowClosed);
                    }
                    EngineCommand::ExportReport { reply } => {
                        let result = persist_report(&monitor, store.as_ref()).await;
                        let _ = reply.send(result);
                    }
                    EngineCommand::ConfigureEndpoint(endpoint) => {
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::ConfigureEndpoint(endpoint),
                        )
                        .await;
                    }
                    EngineCommand::ConfigureScreenEdge(edge) => {
                        state.kvm.screen_edge = edge;
                        publish_kvm_status(&mut state, &snapshot_tx);
                        send_platform_command(
                            platform_handle.as_ref(),
                            PlatformCommand::SetEdgeSwitchEdge(edge),
                        )
                        .await;
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::ConfigureScreenEdge(edge),
                        )
                        .await;
                    }
                    EngineCommand::ConfigureTargetDisplay(display_id) => {
                        if state.kvm.control == ControlState::Local {
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetEdgeSwitchDisplay(display_id),
                            )
                            .await;
                        }
                    }
                    EngineCommand::ConfigureListenPort(port) => {
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::ConfigureListenPort(port),
                        )
                        .await;
                    }
                    EngineCommand::ConfirmPairing => {
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::ConfirmPairing,
                        )
                        .await;
                    }
                    EngineCommand::RejectPairing => {
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::RejectPairing,
                        )
                        .await;
                    }
                    EngineCommand::EnterControl { edge, position } => {
                        if !windows_control_available(&state) {
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetInputSuppressed(false),
                            )
                            .await;
                            continue;
                        }
                        keyboard_tx.send_modify(KeyboardMonitorState::reset);
                        state.kvm.control = ControlState::EnteringRemote;
                        state.kvm.last_error = None;
                        publish_kvm_status(&mut state, &snapshot_tx);
                        tracing::info!(
                            component = "control",
                            ?edge,
                            position,
                            "Windows requested remote control"
                        );
                        send_platform_command(
                            platform_handle.as_ref(),
                            PlatformCommand::BeginEdgeTransition { edge, position },
                        )
                        .await;
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::EnterControl { edge, position },
                        )
                        .await;
                    }
                    EngineCommand::ReturnControl => {
                        const MANUAL_RETURN_POSITION: f32 = 0.5;
                        keyboard_tx.send_modify(KeyboardMonitorState::clear_pressed);
                        state.kvm.control = ControlState::ReturningLocal;
                        publish_kvm_status(&mut state, &snapshot_tx);
                        if state.role == EndpointRole::WindowsController {
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::RestoreLocalPointer {
                                    position: MANUAL_RETURN_POSITION,
                                },
                            )
                            .await;
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetInputSuppressed(false),
                            )
                            .await;
                        } else {
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::ReleaseAll,
                            )
                            .await;
                        }
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::ReturnControl {
                                position: MANUAL_RETURN_POSITION,
                            },
                        )
                        .await;
                    }
                    EngineCommand::SetControlEnabled(enabled) => {
                        let was_transitioning =
                            state.kvm.control != ControlState::Local;
                        state.kvm.control_enabled = enabled;
                        if !enabled {
                            keyboard_tx.send_modify(KeyboardMonitorState::clear_pressed);
                            state.kvm.control = ControlState::Local;
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetEdgeSwitchEnabled(false),
                            )
                            .await;
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetInputSuppressed(false),
                            )
                            .await;
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::ReleaseAll,
                            )
                            .await;
                            send_runtime_command(
                                runtime_handle.as_ref(),
                                RuntimeCommand::ReleaseAll,
                            )
                            .await;
                            if was_transitioning {
                                send_runtime_command(
                                    runtime_handle.as_ref(),
                                    RuntimeCommand::ReturnControl { position: 0.5 },
                                )
                                .await;
                            }
                        } else if windows_control_available(&state) {
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetEdgeSwitchEnabled(true),
                            )
                            .await;
                        }
                        publish_kvm_status(&mut state, &snapshot_tx);
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::SetControlEnabled(enabled),
                        )
                        .await;
                    }
                    EngineCommand::SetGameModeEnabled(enabled) => {
                        state.kvm.game_mode_enabled = enabled;
                        if enabled && state.kvm.foreground_fullscreen {
                            let was_transitioning =
                                state.kvm.control != ControlState::Local;
                            state.kvm.control = ControlState::Local;
                            keyboard_tx.send_modify(KeyboardMonitorState::clear_pressed);
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetEdgeSwitchEnabled(false),
                            )
                            .await;
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetInputSuppressed(false),
                            )
                            .await;
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::ReleaseAll,
                            )
                            .await;
                            send_runtime_command(
                                runtime_handle.as_ref(),
                                RuntimeCommand::ReleaseAll,
                            )
                            .await;
                            if was_transitioning {
                                send_runtime_command(
                                    runtime_handle.as_ref(),
                                    RuntimeCommand::ReturnControl { position: 0.5 },
                                )
                                .await;
                            }
                        } else if windows_control_available(&state) {
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetEdgeSwitchEnabled(true),
                            )
                            .await;
                        }
                        publish_kvm_status(&mut state, &snapshot_tx);
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::SetGameModeEnabled(enabled),
                        )
                        .await;
                    }
                    EngineCommand::ActivateClipboardHistory(id) => {
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::ActivateClipboardHistory(id),
                        )
                        .await;
                    }
                    EngineCommand::ClearClipboardHistory => {
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::ClearClipboardHistory,
                        )
                        .await;
                    }
                    EngineCommand::SetLocalPolicy(policy) => {
                        state.kvm.local_policy = policy;
                        if policy == LocalControlPolicy::Paused {
                            keyboard_tx.send_modify(KeyboardMonitorState::clear_pressed);
                            state.kvm.control = ControlState::Local;
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::SetInputSuppressed(false),
                            )
                            .await;
                            send_platform_command(
                                platform_handle.as_ref(),
                                PlatformCommand::ReleaseAll,
                            )
                            .await;
                        }
                        publish_kvm_status(&mut state, &snapshot_tx);
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::SetLocalPolicy(policy),
                        )
                        .await;
                    }
                    EngineCommand::PermissionUpdated(permission) => {
                        state.kvm.permission = permission;
                        publish_kvm_status(&mut state, &snapshot_tx);
                    }
                    EngineCommand::RefreshPermission => {
                        send_platform_command(
                            platform_handle.as_ref(),
                            PlatformCommand::RefreshPermission,
                        )
                        .await;
                    }
                    EngineCommand::OpenAccessibilitySettings => {
                        send_platform_command(
                            platform_handle.as_ref(),
                            PlatformCommand::OpenAccessibilitySettings,
                        )
                        .await;
                    }
                    EngineCommand::RecoverConnectionNow => {
                        tracing::info!(
                            component = "connection",
                            "system resumed; requesting immediate KVM recovery"
                        );
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::RecoverNow(RecoveryTrigger::SystemResumed),
                        )
                        .await;
                    }
                    EngineCommand::ShutdownRequested { reply } => {
                        keyboard_tx.send_modify(KeyboardMonitorState::clear_pressed);
                        send_platform_command(
                            platform_handle.as_ref(),
                            PlatformCommand::SetInputSuppressed(false),
                        )
                        .await;
                        send_platform_command(
                            platform_handle.as_ref(),
                            PlatformCommand::ReleaseAll,
                        )
                        .await;
                        send_platform_command(
                            platform_handle.as_ref(),
                            PlatformCommand::Shutdown,
                        )
                        .await;
                        send_runtime_command(
                            runtime_handle.as_ref(),
                            RuntimeCommand::Shutdown,
                        )
                        .await;
                        apply_event(&mut state, &snapshot_tx, EngineEvent::ShutdownRequested);
                        let result = persist_report(&monitor, store.as_ref()).await;
                        command_rx.close();
                        let _ = reply.send(result.map(|_| ()));
                        return;
                    }
                }
            }
        }
    }
}

fn record_runtime_validation(
    state: &mut RuntimeState,
    monitor: &mut ValidationMonitor,
    event: &RuntimeEvent,
) {
    if matches!(event, RuntimeEvent::Offline { .. })
        && state.role == EndpointRole::WindowsController
        && state.kvm.control == ControlState::EnteringRemote
    {
        state.validation = monitor.record_kvm(KvmValidationEvent::TransitionFailed);
    }
    let validation_event = match event {
        RuntimeEvent::EnterControlResult { accepted: true, .. }
            if state.role == EndpointRole::WindowsController =>
        {
            Some(KvmValidationEvent::EnteredRemote)
        }
        RuntimeEvent::EnterControlResult {
            accepted: false, ..
        } if state.role == EndpointRole::WindowsController => {
            Some(KvmValidationEvent::TransitionFailed)
        }
        RuntimeEvent::ReturnControlRequested { .. } | RuntimeEvent::ReturnControlAcknowledged
            if state.role == EndpointRole::WindowsController =>
        {
            Some(KvmValidationEvent::ReturnedLocal)
        }
        RuntimeEvent::Offline { .. }
            if state.role == EndpointRole::WindowsController
                && (state.kvm.link == LinkState::Online
                    || state.kvm.control != ControlState::Local) =>
        {
            Some(KvmValidationEvent::Disconnected)
        }
        RuntimeEvent::Online { .. } if state.role == EndpointRole::WindowsController => {
            Some(KvmValidationEvent::Reconnected)
        }
        RuntimeEvent::LatencyUpdated { milliseconds }
            if state.role == EndpointRole::WindowsController =>
        {
            Some(KvmValidationEvent::Latency(*milliseconds))
        }
        RuntimeEvent::RecoveryTriggered {
            trigger: RecoveryTrigger::NetworkChanged,
        } if state.role == EndpointRole::WindowsController => {
            Some(KvmValidationEvent::NetworkRecovery)
        }
        RuntimeEvent::RecoveryTriggered {
            trigger: RecoveryTrigger::SystemResumed,
        } if state.role == EndpointRole::WindowsController => {
            Some(KvmValidationEvent::WakeRecovery)
        }
        _ => None,
    };
    if let Some(event) = validation_event {
        state.validation = monitor.record_kvm(event);
    }
}

fn record_platform_validation(
    state: &mut RuntimeState,
    monitor: &mut ValidationMonitor,
    event: &PlatformEvent,
) {
    let validation_event = match event {
        PlatformEvent::EmergencyReturn { response_ms }
            if state.role == EndpointRole::WindowsController =>
        {
            Some(KvmValidationEvent::EmergencyReturn(*response_ms))
        }
        PlatformEvent::AdapterFailed(_) if state.kvm.control != ControlState::Local => {
            Some(KvmValidationEvent::TransitionFailed)
        }
        _ => None,
    };
    if let Some(event) = validation_event {
        state.validation = monitor.record_kvm(event);
    }
}

fn update_keyboard_monitor_for_runtime_event(
    keyboard_tx: &watch::Sender<KeyboardMonitorState>,
    event: &RuntimeEvent,
) {
    match event {
        RuntimeEvent::EnterControlRequested { .. } => {
            keyboard_tx.send_modify(KeyboardMonitorState::reset);
        }
        RuntimeEvent::InputReceived(InputEvent::Key { usage, action }) => {
            keyboard_tx.send_modify(|monitor| monitor.observe(*usage, *action));
        }
        RuntimeEvent::ConnectionFailed { .. }
        | RuntimeEvent::Offline { .. }
        | RuntimeEvent::ReturnControlRequested { .. }
        | RuntimeEvent::ReturnControlAcknowledged
        | RuntimeEvent::ReleaseAllRequested
        | RuntimeEvent::RuntimeFailed { .. }
        | RuntimeEvent::LocalPolicyChanged {
            policy: LocalControlPolicy::Paused,
        }
        | RuntimeEvent::RemotePauseChanged { paused: true } => {
            keyboard_tx.send_modify(KeyboardMonitorState::clear_pressed);
        }
        _ => {}
    }
}

fn update_keyboard_monitor_for_platform_event(
    state: &RuntimeState,
    keyboard_tx: &watch::Sender<KeyboardMonitorState>,
    event: &PlatformEvent,
) {
    match event {
        PlatformEvent::InputCaptured(InputEvent::Key { usage, action })
            if state.kvm.control == ControlState::Remote =>
        {
            keyboard_tx.send_modify(|monitor| monitor.observe(*usage, *action));
        }
        PlatformEvent::EmergencyReturn { .. } | PlatformEvent::AdapterFailed(_) => {
            keyboard_tx.send_modify(KeyboardMonitorState::clear_pressed);
        }
        _ => {}
    }
}

async fn receive_runtime_event(
    runtime_events: &mut Option<mpsc::Receiver<RuntimeEvent>>,
) -> Option<RuntimeEvent> {
    match runtime_events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

async fn receive_platform_event(
    platform_events: &mut Option<mpsc::Receiver<PlatformEvent>>,
) -> Option<PlatformEvent> {
    match platform_events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

async fn send_runtime_command(runtime: Option<&RuntimeHandle>, command: RuntimeCommand) {
    let Some(runtime) = runtime else {
        tracing::warn!(
            component = "engine",
            "KVM runtime command ignored because no runtime is attached"
        );
        return;
    };
    if let Err(error) = runtime.send(command).await {
        tracing::error!(component = "engine", %error, "failed to send KVM runtime command");
    }
}

async fn send_platform_command(platform: Option<&PlatformHandle>, command: PlatformCommand) {
    let Some(platform) = platform else {
        return;
    };
    if let Err(error) = platform.send(command).await {
        tracing::error!(component = "engine", %error, "failed to send platform adapter command");
    }
}

#[derive(Default)]
struct EngineActions {
    runtime: Vec<RuntimeCommand>,
    platform: Vec<PlatformCommand>,
}

fn apply_runtime_event(
    state: &mut RuntimeState,
    snapshot_tx: &watch::Sender<AppSnapshot>,
    event: RuntimeEvent,
) -> EngineActions {
    let publish_snapshot = !matches!(&event, RuntimeEvent::InputReceived(_));
    let mut actions = EngineActions::default();
    match event {
        RuntimeEvent::TrustedPeerLoaded { peer } => {
            tracing::info!(
                component = "connection",
                paired = peer.is_some(),
                "trusted peer state loaded"
            );
            state.kvm.trust = if peer.is_some() {
                TrustState::Paired
            } else {
                TrustState::Unpaired
            };
            state.kvm.peer_device_name = peer.map(|peer| peer.device_name);
        }
        RuntimeEvent::LocalPolicyChanged { policy } => {
            state.kvm.local_policy = policy;
            if policy == LocalControlPolicy::Paused {
                state.kvm.control = ControlState::Local;
                actions
                    .platform
                    .push(PlatformCommand::SetInputSuppressed(false));
                actions.platform.push(PlatformCommand::ReleaseAll);
            }
        }
        RuntimeEvent::ControlEnabledChanged { enabled } => {
            state.kvm.control_enabled = enabled;
            if !enabled {
                state.kvm.control = ControlState::Local;
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEnabled(false));
                actions
                    .platform
                    .push(PlatformCommand::SetInputSuppressed(false));
                actions.platform.push(PlatformCommand::ReleaseAll);
            } else if windows_control_available(state) {
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEnabled(true));
            }
        }
        RuntimeEvent::GameModeEnabledChanged { enabled } => {
            state.kvm.game_mode_enabled = enabled;
            apply_windows_protection(state, &mut actions);
        }
        RuntimeEvent::ClipboardTransferChanged {
            state: transfer_state,
            error,
            updated_at_unix_ms,
        } => {
            state.kvm.clipboard.transfer_state = transfer_state;
            state.kvm.clipboard.last_error = error;
            if updated_at_unix_ms.is_some() {
                state.kvm.clipboard.last_updated_unix_ms = updated_at_unix_ms;
            }
        }
        RuntimeEvent::ClipboardReceived(payload) => {
            tracing::info!(
                component = "clipboard",
                transfer_id = %payload.id,
                kind = ?payload.kind(),
                inline_bytes = payload.total_inline_bytes(),
                file_count = payload.files.len(),
                "remote clipboard transfer completed"
            );
            state.kvm.clipboard.transfer_state = ClipboardTransferState::Idle;
            state.kvm.clipboard.last_error = None;
            state.kvm.clipboard.last_updated_unix_ms = Some(payload.captured_at_unix_ms);
            actions
                .platform
                .push(PlatformCommand::ApplyClipboard(payload));
        }
        RuntimeEvent::ClipboardHistoryChanged(history) => {
            state.kvm.clipboard.history = history;
        }
        RuntimeEvent::ClipboardHistoryActivated(payload) => {
            state.kvm.clipboard.transfer_state = ClipboardTransferState::Sending;
            state.kvm.clipboard.last_error = None;
            state.kvm.clipboard.last_updated_unix_ms = Some(payload.captured_at_unix_ms);
            actions
                .platform
                .push(PlatformCommand::ApplyClipboard(payload));
        }
        RuntimeEvent::ScreenEdgeChanged { edge } => {
            state.kvm.screen_edge = edge;
            if state.role == EndpointRole::WindowsController {
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEdge(edge));
            }
        }
        RuntimeEvent::ListenerStarted { ip, port } => {
            state.kvm.link = LinkState::Listening;
            state.kvm.endpoint = Some(NetworkEndpoint {
                ip: ip.clone(),
                port,
            });
            state.kvm.last_error = None;
            tracing::info!(
                component = "connection",
                %ip,
                port,
                "Mac listener started"
            );
        }
        RuntimeEvent::ListenerFailed { port, message } => {
            state.kvm.link = LinkState::PortBindFailed;
            state.kvm.endpoint = Some(NetworkEndpoint {
                ip: "0.0.0.0".into(),
                port,
            });
            state.kvm.last_error = Some(connection_error(
                "KVM-PORT-001",
                ConnectionErrorStage::PortBind,
                message.clone(),
                "修改监听端口后重试",
                None,
                None,
            ));
            tracing::error!(
                component = "connection",
                port,
                error = %message,
                "Mac listener failed"
            );
        }
        RuntimeEvent::Connecting {
            attempt_id,
            endpoint,
        } => {
            tracing::info!(
                component = "connection",
                attempt_id,
                ip = %endpoint.ip,
                port = endpoint.port,
                "connecting to Mac endpoint"
            );
            state.kvm.link = LinkState::Connecting;
            state.kvm.endpoint = Some(endpoint);
            state.kvm.last_error = None;
            if state.role == EndpointRole::WindowsController {
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEnabled(false));
            }
        }
        RuntimeEvent::ConnectionFailed {
            attempt_id,
            failure_count,
            message,
        } => {
            tracing::warn!(
                component = "connection",
                attempt_id,
                failure_count,
                error = %message,
                "connection attempt failed"
            );
            state.kvm.link = LinkState::Backoff;
            state.kvm.control = ControlState::Local;
            state.kvm.consecutive_failures = failure_count;
            if state.role == EndpointRole::WindowsController {
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEnabled(false));
                actions
                    .platform
                    .push(PlatformCommand::SetInputSuppressed(false));
            }
            state.kvm.last_error = Some(connection_error(
                "KVM-NET-001",
                ConnectionErrorStage::Reachability,
                message,
                "确认 Mac IP、端口和防火墙设置",
                Some(attempt_id),
                None,
            ));
        }
        RuntimeEvent::Pairing {
            code,
            peer,
            local_confirmed,
            remote_confirmed,
            expires_at_unix_ms,
        } => {
            state.kvm.trust = TrustState::Pairing;
            state.kvm.link = LinkState::Authenticating;
            if state.role == EndpointRole::WindowsController {
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEnabled(false));
            }
            state.kvm.peer_device_name = Some(peer.device_name);
            state.kvm.pairing = Some(PairingStatus {
                code,
                local_confirmed,
                remote_confirmed,
                expires_at_unix_ms,
            });
            state.kvm.last_error = None;
        }
        RuntimeEvent::PairingRejected { local } => {
            state.kvm.trust = TrustState::Unpaired;
            state.kvm.pairing = None;
            if state.role == EndpointRole::WindowsController {
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEnabled(false));
            }
            state.kvm.last_error = Some(connection_error(
                "KVM-PAIR-002",
                ConnectionErrorStage::Pairing,
                if local {
                    "本机拒绝了配对"
                } else {
                    "远端拒绝了配对"
                },
                "重新发起连接并核对配对码",
                None,
                None,
            ));
        }
        RuntimeEvent::Online {
            session_id,
            peer,
            endpoint,
        } => {
            let peer_name = peer.device_name;
            state.kvm.trust = TrustState::Paired;
            state.kvm.link = LinkState::Online;
            state.kvm.peer_device_name = Some(peer_name.clone());
            if state.role == EndpointRole::WindowsController {
                state.kvm.endpoint = Some(endpoint.clone());
            }
            state.kvm.session_id = Some(session_id);
            state.kvm.pairing = None;
            state.kvm.consecutive_failures = 0;
            state.kvm.last_error = None;
            if state.role == EndpointRole::WindowsController {
                state.kvm.remote_control_available = true;
                actions.platform.push(PlatformCommand::SetEdgeSwitchEnabled(
                    windows_control_available(state),
                ));
            }
            tracing::info!(
                component = "connection",
                peer = %peer_name,
                endpoint = %format!("{}:{}", endpoint.ip, endpoint.port),
                "KVM session online"
            );
        }
        RuntimeEvent::Offline { reason, retrying } => {
            let offline_reason = reason.clone();
            state.kvm.link = if retrying {
                LinkState::Backoff
            } else if state.role == EndpointRole::MacEndpoint {
                LinkState::Listening
            } else {
                LinkState::Idle
            };
            state.kvm.control = ControlState::Local;
            state.kvm.remote_control_available = false;
            state.kvm.latency_ms = None;
            state.kvm.session_id = None;
            state.kvm.pairing = None;
            state.kvm.last_error = Some(connection_error(
                "KVM-NET-002",
                ConnectionErrorStage::Protocol,
                reason,
                if retrying {
                    "程序将在 5 秒后自动重连"
                } else {
                    "重新连接设备"
                },
                None,
                None,
            ));
            actions
                .platform
                .push(PlatformCommand::SetEdgeSwitchEnabled(false));
            actions
                .platform
                .push(PlatformCommand::SetInputSuppressed(false));
            actions.platform.push(PlatformCommand::ReleaseAll);
            tracing::warn!(
                component = "connection",
                retrying,
                reason = %offline_reason,
                "KVM session offline"
            );
        }
        RuntimeEvent::LatencyUpdated { milliseconds } => {
            state.kvm.latency_ms = Some(milliseconds);
        }
        RuntimeEvent::EnterControlRequested { edge, position } => {
            state.kvm.control = ControlState::EnteringRemote;
            let accepted = state.kvm.permission == PermissionStatus::Granted
                && state.kvm.local_policy == LocalControlPolicy::Allowed;
            let reason = (!accepted).then(|| {
                if state.kvm.permission != PermissionStatus::Granted {
                    "Mac 缺少辅助功能权限".into()
                } else {
                    "Mac 已暂停远程控制".into()
                }
            });
            tracing::info!(
                component = "control",
                ?edge,
                position,
                permission = ?state.kvm.permission,
                local_policy = ?state.kvm.local_policy,
                accepted,
                "Mac received remote-control request"
            );
            if accepted {
                actions
                    .platform
                    .push(PlatformCommand::PrepareRemotePointer { edge, position });
            } else {
                state.kvm.control = ControlState::Local;
                actions
                    .runtime
                    .push(RuntimeCommand::RespondEnterControl { accepted, reason });
            }
        }
        RuntimeEvent::EnterControlResult { accepted, reason } => {
            state.kvm.control = if accepted {
                ControlState::Remote
            } else {
                ControlState::Local
            };
            if accepted {
                state.kvm.remote_control_available = true;
                state.kvm.last_error = None;
            } else {
                let remote_paused = reason.as_deref() == Some("Mac 已暂停远程控制");
                if remote_paused {
                    state.kvm.remote_control_available = false;
                    actions
                        .platform
                        .push(PlatformCommand::SetEdgeSwitchEnabled(false));
                }
                state.kvm.last_error = Some(connection_error(
                    "KVM-CONTROL-001",
                    ConnectionErrorStage::Permission,
                    reason
                        .clone()
                        .unwrap_or_else(|| "Mac 拒绝进入远程控制".into()),
                    "检查 Mac 辅助功能权限和本机暂停状态",
                    None,
                    state.kvm.session_id.clone(),
                ));
            }
            tracing::info!(
                component = "control",
                accepted,
                reason = reason.as_deref().unwrap_or("none"),
                "Windows received remote-control result"
            );
            actions
                .platform
                .push(PlatformCommand::SetInputSuppressed(accepted));
        }
        RuntimeEvent::ReturnControlRequested { position } => {
            state.kvm.control = ControlState::Local;
            actions.runtime.push(RuntimeCommand::ReleaseAll);
            if state.role == EndpointRole::WindowsController {
                actions
                    .platform
                    .push(PlatformCommand::RestoreLocalPointer { position });
            }
            actions
                .platform
                .push(PlatformCommand::SetInputSuppressed(false));
            actions.platform.push(PlatformCommand::ReleaseAll);
        }
        RuntimeEvent::ReturnControlAcknowledged => {
            state.kvm.control = ControlState::Local;
            actions.runtime.push(RuntimeCommand::ReleaseAll);
            actions
                .platform
                .push(PlatformCommand::SetInputSuppressed(false));
            actions.platform.push(PlatformCommand::ReleaseAll);
        }
        RuntimeEvent::InputReceived(event) => {
            actions.platform.push(PlatformCommand::Inject(event));
        }
        RuntimeEvent::ReleaseAllRequested => {
            state.kvm.control = ControlState::Local;
            actions
                .platform
                .push(PlatformCommand::SetInputSuppressed(false));
            actions.platform.push(PlatformCommand::ReleaseAll);
        }
        RuntimeEvent::RemotePauseChanged { paused } => {
            state.kvm.remote_control_available = !paused;
            if paused {
                state.kvm.control = ControlState::Local;
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEnabled(false));
                actions
                    .platform
                    .push(PlatformCommand::SetInputSuppressed(false));
                actions.platform.push(PlatformCommand::ReleaseAll);
            } else if windows_control_available(state) {
                state.kvm.last_error = None;
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEnabled(true));
            }
        }
        RuntimeEvent::RecoveryTriggered { .. } | RuntimeEvent::ConfigUpdated => {}
        RuntimeEvent::RuntimeFailed { message } => {
            tracing::warn!(
                component = "connection",
                role = ?state.role,
                error = %message,
                "KVM runtime operation failed"
            );
            state.kvm.control = ControlState::Local;
            if state.role == EndpointRole::WindowsController {
                actions
                    .platform
                    .push(PlatformCommand::SetEdgeSwitchEnabled(false));
            }
            actions
                .platform
                .push(PlatformCommand::SetInputSuppressed(false));
            actions.platform.push(PlatformCommand::ReleaseAll);
            state.kvm.last_error = Some(connection_error(
                "KVM-RUNTIME-001",
                ConnectionErrorStage::Protocol,
                message,
                "查看日志后重试",
                None,
                state.kvm.session_id.clone(),
            ));
        }
    }
    if publish_snapshot {
        publish_kvm_status(state, snapshot_tx);
    }
    actions
}

fn apply_platform_event(
    state: &mut RuntimeState,
    snapshot_tx: &watch::Sender<AppSnapshot>,
    event: PlatformEvent,
) -> EngineActions {
    let publish_snapshot = !matches!(&event, PlatformEvent::InputCaptured(_));
    let mut actions = EngineActions::default();
    match event {
        PlatformEvent::InputCaptured(event) => {
            if state.kvm.control == ControlState::Remote {
                actions.runtime.push(RuntimeCommand::SendInput(event));
            }
        }
        PlatformEvent::TargetDisplayChanged { display_id } => {
            state.kvm.target_display_id = Some(display_id.clone());
            actions
                .runtime
                .push(RuntimeCommand::ConfigureTargetDisplay(display_id));
        }
        PlatformEvent::ForegroundFullscreenChanged { active } => {
            state.kvm.foreground_fullscreen = active;
            apply_windows_protection(state, &mut actions);
            tracing::info!(
                component = "game-mode",
                active,
                protection_enabled = state.kvm.game_mode_enabled,
                "Windows foreground fullscreen state changed"
            );
        }
        PlatformEvent::ClipboardChanged(payload) => {
            tracing::info!(
                component = "clipboard",
                transfer_id = %payload.id,
                kind = ?payload.kind(),
                inline_bytes = payload.total_inline_bytes(),
                file_count = payload.files.len(),
                "local clipboard changed"
            );
            state.kvm.clipboard.transfer_state = ClipboardTransferState::Sending;
            state.kvm.clipboard.last_error = None;
            state.kvm.clipboard.last_updated_unix_ms = Some(payload.captured_at_unix_ms);
            actions.runtime.push(RuntimeCommand::SetClipboard(payload));
        }
        PlatformEvent::ClipboardFailed(message) => {
            tracing::warn!(
                component = "clipboard",
                error = %message,
                "clipboard adapter operation failed"
            );
            state.kvm.clipboard.transfer_state = ClipboardTransferState::Failed;
            state.kvm.clipboard.last_error = Some(message);
        }
        PlatformEvent::LocalEdgeReached { edge, position } => {
            if windows_control_available(state) {
                state.kvm.control = ControlState::EnteringRemote;
                state.kvm.last_error = None;
                actions
                    .platform
                    .push(PlatformCommand::BeginEdgeTransition { edge, position });
                actions
                    .runtime
                    .push(RuntimeCommand::EnterControl { edge, position });
                tracing::info!(
                    component = "control",
                    ?edge,
                    position,
                    "Windows edge crossing requested remote control"
                );
            } else {
                actions
                    .platform
                    .push(PlatformCommand::SetInputSuppressed(false));
            }
        }
        PlatformEvent::RemoteEdgeReached { position } => {
            if state.role == EndpointRole::MacEndpoint && state.kvm.control == ControlState::Remote
            {
                state.kvm.control = ControlState::Local;
                actions
                    .runtime
                    .push(RuntimeCommand::ReturnControl { position });
                actions.platform.push(PlatformCommand::ReleaseAll);
                tracing::info!(
                    component = "control",
                    position,
                    "Mac return edge restored Windows control"
                );
            }
        }
        PlatformEvent::EmergencyReturn { .. } => {
            state.kvm.control = ControlState::Local;
            actions
                .platform
                .push(PlatformCommand::SetInputSuppressed(false));
            actions.runtime.push(RuntimeCommand::ReleaseAll);
            actions
                .runtime
                .push(RuntimeCommand::ReturnControl { position: 0.5 });
            tracing::warn!(
                component = "input",
                "emergency return restored Windows local input"
            );
        }
        PlatformEvent::PermissionChanged(permission) => {
            state.kvm.permission = permission;
            tracing::info!(
                component = "permission",
                ?permission,
                "macOS Accessibility permission refreshed"
            );
        }
        PlatformEvent::RemotePointerPrepared => {
            if state.role == EndpointRole::MacEndpoint
                && state.kvm.control == ControlState::EnteringRemote
            {
                state.kvm.control = ControlState::Remote;
                state.kvm.last_error = None;
                actions.runtime.push(RuntimeCommand::RespondEnterControl {
                    accepted: true,
                    reason: None,
                });
                tracing::info!(
                    component = "control",
                    "Mac input adapter prepared for remote control"
                );
            }
        }
        PlatformEvent::AdapterFailed(message) => {
            let entering_remote = state.role == EndpointRole::MacEndpoint
                && state.kvm.control == ControlState::EnteringRemote;
            state.kvm.control = ControlState::Local;
            state.kvm.last_error = Some(connection_error(
                "KVM-INPUT-001",
                ConnectionErrorStage::InputAdapter,
                message.clone(),
                "检查系统权限后重新连接",
                None,
                state.kvm.session_id.clone(),
            ));
            actions
                .platform
                .push(PlatformCommand::SetInputSuppressed(false));
            actions.platform.push(PlatformCommand::ReleaseAll);
            actions.runtime.push(RuntimeCommand::ReleaseAll);
            if entering_remote {
                actions.runtime.push(RuntimeCommand::RespondEnterControl {
                    accepted: false,
                    reason: Some(message.clone()),
                });
            }
            tracing::error!(
                component = "input",
                error = %message,
                entering_remote,
                "platform input adapter failed"
            );
        }
    }
    if publish_snapshot {
        publish_kvm_status(state, snapshot_tx);
    }
    actions
}

fn windows_control_available(state: &RuntimeState) -> bool {
    state.role == EndpointRole::WindowsController
        && state.kvm.control_enabled
        && !(state.kvm.game_mode_enabled && state.kvm.foreground_fullscreen)
        && state.kvm.link == LinkState::Online
        && state.kvm.control == ControlState::Local
        && state.kvm.remote_control_available
}

fn apply_windows_protection(state: &mut RuntimeState, actions: &mut EngineActions) {
    if state.role != EndpointRole::WindowsController {
        return;
    }
    if windows_control_available(state) {
        actions
            .platform
            .push(PlatformCommand::SetEdgeSwitchEnabled(true));
        return;
    }
    let was_transitioning = state.kvm.control != ControlState::Local;
    if state.kvm.game_mode_enabled && state.kvm.foreground_fullscreen {
        state.kvm.control = ControlState::Local;
    }
    actions
        .platform
        .push(PlatformCommand::SetEdgeSwitchEnabled(false));
    if was_transitioning && state.kvm.game_mode_enabled && state.kvm.foreground_fullscreen {
        actions
            .platform
            .push(PlatformCommand::SetInputSuppressed(false));
        actions.platform.push(PlatformCommand::ReleaseAll);
        actions.runtime.push(RuntimeCommand::ReleaseAll);
        actions
            .runtime
            .push(RuntimeCommand::ReturnControl { position: 0.5 });
    }
}

fn publish_kvm_status(state: &mut RuntimeState, snapshot_tx: &watch::Sender<AppSnapshot>) {
    apply_event(
        state,
        snapshot_tx,
        EngineEvent::KvmStatusUpdated {
            status: state.kvm.clone(),
        },
    );
}

fn connection_error(
    code: &str,
    stage: ConnectionErrorStage,
    message: impl Into<String>,
    recovery: impl Into<String>,
    attempt_id: Option<u64>,
    session_id: Option<String>,
) -> ConnectionError {
    ConnectionError {
        code: code.into(),
        stage,
        message: message.into(),
        recovery: recovery.into(),
        attempt_id,
        session_id,
    }
}

fn interval_after(period: Duration) -> tokio::time::Interval {
    let mut interval = tokio::time::interval_at(Instant::now() + period, period);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval
}

fn apply_event(
    state: &mut RuntimeState,
    snapshot_tx: &watch::Sender<AppSnapshot>,
    event: EngineEvent,
) -> bool {
    let mut should_stop = false;

    for effect in reduce(state, event) {
        match effect {
            EngineEffect::PublishSnapshot => {
                let _ = snapshot_tx.send(AppSnapshot::from(&*state));
            }
            EngineEffect::StopRuntime => should_stop = true,
        }
    }

    should_stop
}

async fn persist_report(
    monitor: &ValidationMonitor,
    store: &dyn ValidationReportStore,
) -> Result<PathBuf, EngineError> {
    let report = monitor.report(unix_time_ms());
    store.persist(&report).await.map_err(EngineError::from)
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod kvm_tests {
    use super::*;
    use kvmflow_domain::{
        ClipboardContentKind, ClipboardHistoryItem, ClipboardPayload, InputEvent, KeyAction,
        KeyUsage,
    };

    fn state(role: EndpointRole) -> (RuntimeState, watch::Sender<AppSnapshot>) {
        let state = RuntimeState::new(
            role,
            BuildInfo {
                version: "0.1.0".into(),
                build_id: "kvm-test".into(),
            },
            None,
        );
        let (snapshots, _) = watch::channel(AppSnapshot::from(&state));
        (state, snapshots)
    }

    #[test]
    fn accepted_enter_suppresses_windows_input() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);

        let actions = apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::EnterControlResult {
                accepted: true,
                reason: None,
            },
        );

        assert_eq!(state.kvm.control, ControlState::Remote);
        assert!(
            actions
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::SetInputSuppressed(true)))
        );
    }

    #[test]
    fn configured_screen_edge_updates_status_and_windows_detector() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);

        let actions = apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::ScreenEdgeChanged {
                edge: ScreenEdge::Left,
            },
        );

        assert_eq!(state.kvm.screen_edge, ScreenEdge::Left);
        assert!(actions.platform.iter().any(|command| matches!(
            command,
            PlatformCommand::SetEdgeSwitchEdge(ScreenEdge::Left)
        )));
    }

    #[test]
    fn clipboard_history_is_published_and_reapplied_locally() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);
        let item = ClipboardHistoryItem {
            id: "history-one".into(),
            captured_at_unix_ms: 42,
            kind: ClipboardContentKind::Text,
            preview: "hello".into(),
            size_bytes: 5,
            available: true,
        };

        apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::ClipboardHistoryChanged(vec![item.clone()]),
        );
        assert_eq!(state.kvm.clipboard.history, vec![item]);

        let payload = ClipboardPayload {
            id: "history-activated".into(),
            captured_at_unix_ms: 84,
            text: Some("hello".into()),
            html: None,
            rich_text: None,
            image_png: None,
            files: Vec::new(),
        };
        let actions = apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::ClipboardHistoryActivated(payload.clone()),
        );

        assert!(actions.platform.iter().any(|command| matches!(
            command,
            PlatformCommand::ApplyClipboard(applied) if applied == &payload
        )));
        assert_eq!(
            state.kvm.clipboard.transfer_state,
            ClipboardTransferState::Sending
        );
    }

    #[test]
    fn selected_windows_display_updates_status_and_persists_actual_target() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);

        let actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::TargetDisplayChanged {
                display_id: r"\\.\DISPLAY2".into(),
            },
        );

        assert_eq!(
            state.kvm.target_display_id.as_deref(),
            Some(r"\\.\DISPLAY2")
        );
        assert!(actions.runtime.iter().any(|command| matches!(
            command,
            RuntimeCommand::ConfigureTargetDisplay(display_id)
                if display_id == r"\\.\DISPLAY2"
        )));
    }

    #[test]
    fn third_connection_failure_is_exposed_until_online_recovery() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);

        apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::ConnectionFailed {
                attempt_id: 3,
                failure_count: 3,
                message: "connection refused".into(),
            },
        );
        assert_eq!(state.kvm.consecutive_failures, 3);

        apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::Online {
                session_id: "session".into(),
                peer: kvmflow_transport::TrustedPeer {
                    role: EndpointRole::MacEndpoint,
                    device_id: "mac".into(),
                    device_name: "Mac".into(),
                    public_key: vec![0; 32],
                },
                endpoint: NetworkEndpoint {
                    ip: "192.168.1.20".into(),
                    port: 45_888,
                },
            },
        );
        assert_eq!(state.kvm.consecutive_failures, 0);
    }

    #[test]
    fn mac_accepts_control_only_after_the_pointer_is_prepared() {
        let (mut state, snapshots) = state(EndpointRole::MacEndpoint);
        state.kvm.permission = PermissionStatus::Granted;
        state.kvm.local_policy = LocalControlPolicy::Allowed;

        let requested = apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::EnterControlRequested {
                edge: ScreenEdge::Right,
                position: 0.5,
            },
        );

        assert_eq!(state.kvm.control, ControlState::EnteringRemote);
        assert!(requested.runtime.is_empty());
        assert!(requested.platform.iter().any(|command| matches!(
            command,
            PlatformCommand::PrepareRemotePointer {
                edge: ScreenEdge::Right,
                position
            } if *position == 0.5
        )));

        let prepared =
            apply_platform_event(&mut state, &snapshots, PlatformEvent::RemotePointerPrepared);

        assert_eq!(state.kvm.control, ControlState::Remote);
        assert!(prepared.runtime.iter().any(|command| matches!(
            command,
            RuntimeCommand::RespondEnterControl {
                accepted: true,
                reason: None
            }
        )));
    }

    #[test]
    fn mac_reports_pointer_preparation_failure_to_windows() {
        let (mut state, snapshots) = state(EndpointRole::MacEndpoint);
        state.kvm.control = ControlState::EnteringRemote;

        let actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::AdapterFailed("event source unavailable".into()),
        );

        assert_eq!(state.kvm.control, ControlState::Local);
        assert!(actions.runtime.iter().any(|command| matches!(
            command,
            RuntimeCommand::RespondEnterControl {
                accepted: false,
                reason: Some(reason)
            } if reason == "event source unavailable"
        )));
    }

    #[test]
    fn captured_input_is_forwarded_only_while_remote() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);
        let snapshot_updates = snapshots.subscribe();
        let event = InputEvent::Key {
            usage: KeyUsage::A,
            action: KeyAction::Down,
        };

        let local_actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::InputCaptured(event.clone()),
        );
        state.kvm.control = ControlState::Remote;
        let remote_actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::InputCaptured(event.clone()),
        );

        assert!(local_actions.runtime.is_empty());
        assert!(remote_actions.runtime.iter().any(|command| matches!(
            command,
            RuntimeCommand::SendInput(captured) if captured == &event
        )));
        assert!(!snapshot_updates.has_changed().expect("snapshot channel"));
    }

    #[test]
    fn online_windows_edge_crossing_starts_remote_control() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);
        state.kvm.link = LinkState::Online;

        let actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::LocalEdgeReached {
                edge: ScreenEdge::Bottom,
                position: 0.25,
            },
        );

        assert_eq!(state.kvm.control, ControlState::EnteringRemote);
        assert!(actions.runtime.iter().any(|command| matches!(
            command,
            RuntimeCommand::EnterControl {
                edge: ScreenEdge::Bottom,
                position
            } if *position == 0.25
        )));
        assert!(actions.platform.iter().any(|command| matches!(
            command,
            PlatformCommand::BeginEdgeTransition {
                edge: ScreenEdge::Bottom,
                position
            } if *position == 0.25
        )));
    }

    #[test]
    fn disabled_windows_control_blocks_manual_and_edge_entry() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);
        state.kvm.link = LinkState::Online;
        state.kvm.control_enabled = false;

        let actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::LocalEdgeReached {
                edge: ScreenEdge::Bottom,
                position: 0.25,
            },
        );

        assert_eq!(state.kvm.control, ControlState::Local);
        assert!(actions.runtime.is_empty());
        assert!(
            !actions
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::BeginEdgeTransition { .. }))
        );
    }

    #[test]
    fn foreground_fullscreen_releases_remote_control_until_it_leaves_foreground() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);
        state.kvm.link = LinkState::Online;
        state.kvm.control = ControlState::Remote;

        let protected = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::ForegroundFullscreenChanged { active: true },
        );

        assert!(state.kvm.foreground_fullscreen);
        assert_eq!(state.kvm.control, ControlState::Local);
        assert!(protected.runtime.iter().any(
            |command| matches!(command, RuntimeCommand::ReturnControl { position } if *position == 0.5)
        ));
        assert!(
            protected
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::SetEdgeSwitchEnabled(false)))
        );

        let restored = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::ForegroundFullscreenChanged { active: false },
        );

        assert!(!state.kvm.foreground_fullscreen);
        assert!(
            restored
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::SetEdgeSwitchEnabled(true)))
        );
    }

    #[test]
    fn remote_pause_disables_windows_edge_switch_until_resumed() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);
        state.kvm.link = LinkState::Online;

        let paused_actions = apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::RemotePauseChanged { paused: true },
        );

        assert!(!state.kvm.remote_control_available);
        assert!(
            paused_actions
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::SetEdgeSwitchEnabled(false)))
        );

        let resumed_actions = apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::RemotePauseChanged { paused: false },
        );

        assert!(state.kvm.remote_control_available);
        assert!(
            resumed_actions
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::SetEdgeSwitchEnabled(true)))
        );
    }

    #[test]
    fn unavailable_remote_blocks_stale_windows_edge_events() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);
        state.kvm.link = LinkState::Online;
        state.kvm.remote_control_available = false;

        let actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::LocalEdgeReached {
                edge: ScreenEdge::Bottom,
                position: 0.5,
            },
        );

        assert_eq!(state.kvm.control, ControlState::Local);
        assert!(actions.runtime.is_empty());
        assert!(
            !actions
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::BeginEdgeTransition { .. }))
        );
    }

    #[test]
    fn offline_edge_event_cancels_transition_without_requesting_control() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);

        let actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::LocalEdgeReached {
                edge: ScreenEdge::Bottom,
                position: 0.5,
            },
        );

        assert_eq!(state.kvm.control, ControlState::Local);
        assert!(actions.runtime.is_empty());
        assert!(
            actions
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::SetInputSuppressed(false)))
        );
    }

    #[test]
    fn mac_return_edge_releases_inputs_and_returns_at_the_same_position() {
        let (mut state, snapshots) = state(EndpointRole::MacEndpoint);
        state.kvm.control = ControlState::Remote;

        let actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::RemoteEdgeReached { position: 0.75 },
        );

        assert_eq!(state.kvm.control, ControlState::Local);
        assert!(actions.runtime.iter().any(|command| matches!(
            command,
            RuntimeCommand::ReturnControl { position } if *position == 0.75
        )));
        assert!(
            actions
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::ReleaseAll))
        );
    }

    #[test]
    fn windows_return_request_restores_the_reported_edge_position() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);
        state.kvm.control = ControlState::Remote;

        let actions = apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::ReturnControlRequested { position: 0.4 },
        );

        assert_eq!(state.kvm.control, ControlState::Local);
        assert!(actions.platform.iter().any(|command| matches!(
            command,
            PlatformCommand::RestoreLocalPointer { position } if *position == 0.4
        )));
        assert!(
            actions
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::SetInputSuppressed(false)))
        );
    }

    #[test]
    fn received_input_is_injected_without_publishing_a_snapshot() {
        let (mut state, snapshots) = state(EndpointRole::MacEndpoint);
        let snapshot_updates = snapshots.subscribe();
        let event = InputEvent::MouseMove { dx: 4, dy: -2 };

        let actions = apply_runtime_event(
            &mut state,
            &snapshots,
            RuntimeEvent::InputReceived(event.clone()),
        );

        assert!(actions.platform.iter().any(|command| matches!(
            command,
            PlatformCommand::Inject(received) if received == &event
        )));
        assert!(!snapshot_updates.has_changed().expect("snapshot channel"));
    }

    #[test]
    fn runtime_keyboard_monitor_tracks_received_keys_but_ignores_mouse_input() {
        let (_, snapshots) = state(EndpointRole::MacEndpoint);
        let (keyboard_tx, mut keyboard_rx) = watch::channel(KeyboardMonitorState::default());

        update_keyboard_monitor_for_runtime_event(
            &keyboard_tx,
            &RuntimeEvent::InputReceived(InputEvent::MouseMove { dx: 4, dy: -2 }),
        );
        assert!(!keyboard_rx.has_changed().expect("keyboard channel"));

        update_keyboard_monitor_for_runtime_event(
            &keyboard_tx,
            &RuntimeEvent::InputReceived(InputEvent::Key {
                usage: KeyUsage::LEFT_GUI,
                action: KeyAction::Down,
            }),
        );

        assert!(keyboard_rx.has_changed().expect("keyboard channel"));
        let monitor = keyboard_rx.borrow_and_update().clone();
        assert_eq!(monitor.pressed, vec![KeyUsage::LEFT_GUI]);
        assert_eq!(monitor.recent.len(), 1);
        assert!(
            !snapshots
                .subscribe()
                .has_changed()
                .expect("snapshot channel")
        );
    }

    #[test]
    fn platform_keyboard_monitor_tracks_only_forwarded_remote_keys() {
        let (mut state, _) = state(EndpointRole::WindowsController);
        let (keyboard_tx, mut keyboard_rx) = watch::channel(KeyboardMonitorState::default());
        let event = PlatformEvent::InputCaptured(InputEvent::Key {
            usage: KeyUsage::UP_ARROW,
            action: KeyAction::Down,
        });

        update_keyboard_monitor_for_platform_event(&state, &keyboard_tx, &event);
        assert!(!keyboard_rx.has_changed().expect("keyboard channel"));

        state.kvm.control = ControlState::Remote;
        update_keyboard_monitor_for_platform_event(&state, &keyboard_tx, &event);

        assert!(keyboard_rx.has_changed().expect("keyboard channel"));
        assert_eq!(
            keyboard_rx.borrow_and_update().pressed,
            vec![KeyUsage::UP_ARROW]
        );
    }

    #[test]
    fn return_control_clears_pressed_keys_without_erasing_recent_history() {
        let (keyboard_tx, mut keyboard_rx) = watch::channel(KeyboardMonitorState::default());
        keyboard_tx.send_modify(|monitor| {
            monitor.observe(KeyUsage::LEFT_CONTROL, KeyAction::Down);
            monitor.observe(KeyUsage::SPACE, KeyAction::Down);
        });
        let _ = keyboard_rx.borrow_and_update();

        update_keyboard_monitor_for_runtime_event(
            &keyboard_tx,
            &RuntimeEvent::ReturnControlAcknowledged,
        );

        let monitor = keyboard_rx.borrow_and_update().clone();
        assert!(monitor.pressed.is_empty());
        assert_eq!(monitor.recent.len(), 2);
    }

    #[test]
    fn emergency_return_restores_local_state_and_releases_remote_inputs() {
        let (mut state, snapshots) = state(EndpointRole::WindowsController);
        state.kvm.control = ControlState::Remote;

        let actions = apply_platform_event(
            &mut state,
            &snapshots,
            PlatformEvent::EmergencyReturn {
                response_ms: Some(0.5),
            },
        );

        assert_eq!(state.kvm.control, ControlState::Local);
        assert!(
            actions
                .runtime
                .iter()
                .any(|command| matches!(command, RuntimeCommand::ReturnControl { .. }))
        );
        assert!(
            actions
                .runtime
                .iter()
                .any(|command| matches!(command, RuntimeCommand::ReleaseAll))
        );
        assert!(
            actions
                .platform
                .iter()
                .any(|command| matches!(command, PlatformCommand::SetInputSuppressed(false)))
        );
    }
}
