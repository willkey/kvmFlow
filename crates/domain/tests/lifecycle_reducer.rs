use kvmflow_domain::{
    BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole, EngineEffect, EngineEvent,
    GateStatus, MetricProgress, ResourceSample, RuntimePhase, RuntimeState, ValidationProgress,
    WindowState, reduce,
};

fn state() -> RuntimeState {
    RuntimeState::new(
        EndpointRole::WindowsController,
        BuildInfo {
            version: "0.1.0".into(),
            build_id: "test-build".into(),
        },
        None,
    )
}

#[test]
fn runtime_state_new_uses_locked_starting_defaults_and_notice_contract() {
    let notice = DiagnosticNotice {
        code: DiagnosticNoticeCode::StorageFallback,
        message: "安装目录不可写，日志已切换到备用目录".into(),
        path: Some("/tmp/kvmflow-emergency-42".into()),
    };
    let state = RuntimeState::new(
        EndpointRole::WindowsController,
        BuildInfo {
            version: "0.1.0".into(),
            build_id: "defaults-build".into(),
        },
        Some(notice.clone()),
    );
    let validation = ValidationProgress::default();

    assert_eq!(state.phase, RuntimePhase::Starting);
    assert_eq!(state.window_state, WindowState::Closed);
    assert_eq!(state.latest_resource, None);
    assert_eq!(state.diagnostic_notice, Some(notice));
    assert_eq!(state.validation, validation);

    assert_eq!(
        state.validation.background_memory.status,
        GateStatus::NotObserved
    );
    assert_eq!(state.validation.background_memory.current, None);
    assert_eq!(state.validation.background_memory.target, 52_428_800.0);
    assert_eq!(state.validation.background_memory.unit, "bytes");
    assert_eq!(
        state.validation.visible_memory.status,
        GateStatus::NotObserved
    );
    assert_eq!(state.validation.visible_memory.current, None);
    assert_eq!(state.validation.visible_memory.target, 188_743_680.0);
    assert_eq!(state.validation.visible_memory.unit, "bytes");
    assert_eq!(
        state.validation.background_cpu.status,
        GateStatus::NotObserved
    );
    assert_eq!(state.validation.background_cpu.current, None);
    assert_eq!(state.validation.background_cpu.target, 0.5);
    assert_eq!(state.validation.background_cpu.unit, "percent");
    assert_eq!(state.validation.continuous_uptime_seconds, 0);

    let absent_notice = RuntimeState::new(
        EndpointRole::WindowsController,
        BuildInfo {
            version: "0.1.0".into(),
            build_id: "absent-notice-build".into(),
        },
        None,
    );
    assert_eq!(absent_notice.diagnostic_notice, None);
}

#[test]
fn started_enters_running_and_publishes() {
    let mut state = state();

    assert_eq!(
        reduce(&mut state, EngineEvent::Started),
        vec![EngineEffect::PublishSnapshot]
    );
    assert_eq!(state.phase, RuntimePhase::Running);
}

#[test]
fn window_events_only_change_window_state() {
    let mut state = state();

    reduce(&mut state, EngineEvent::Started);
    reduce(&mut state, EngineEvent::WindowOpened);
    assert_eq!(state.window_state, WindowState::Open);
    assert_eq!(state.phase, RuntimePhase::Running);

    reduce(&mut state, EngineEvent::WindowClosed);
    assert_eq!(state.window_state, WindowState::Closed);
    assert_eq!(state.phase, RuntimePhase::Running);
}

#[test]
fn resource_updates_replace_the_latest_sample_and_validation_progress() {
    let mut state = state();
    let sample = ResourceSample {
        observed_at_unix_ms: 42,
        uptime_seconds: 12,
        window_state: WindowState::Open,
        process_count: 3,
        resident_memory_bytes: 64,
        cpu_percent: 0.25,
        cpu_valid: true,
    };
    let validation = kvmflow_domain::ValidationProgress {
        background_memory: MetricProgress {
            status: kvmflow_domain::GateStatus::Collecting,
            current: Some(64.0),
            target: 50.0,
            unit: "bytes".into(),
        },
        ..Default::default()
    };

    assert_eq!(
        reduce(
            &mut state,
            EngineEvent::ResourceUpdated {
                sample,
                validation: validation.clone(),
            },
        ),
        vec![EngineEffect::PublishSnapshot]
    );
    assert_eq!(state.latest_resource, Some(sample));
    assert_eq!(state.validation, validation);
}

#[test]
fn shutdown_stops_after_publishing() {
    let mut state = state();

    assert_eq!(
        reduce(&mut state, EngineEvent::ShutdownRequested),
        vec![EngineEffect::PublishSnapshot, EngineEffect::StopRuntime],
    );
    assert_eq!(state.phase, RuntimePhase::Stopping);
}
