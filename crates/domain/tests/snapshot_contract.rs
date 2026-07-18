use kvmflow_domain::{
    AppSnapshot, BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole, RuntimeState,
};

const APP_SNAPSHOT_FIXTURE: &str = include_str!("../../../fixtures/app-snapshot.json");

#[test]
fn fixture_deserializes_and_round_trips() {
    let snapshot: AppSnapshot = serde_json::from_str(APP_SNAPSHOT_FIXTURE).unwrap();

    assert_eq!(snapshot.schema_version, 2);
    assert_eq!(snapshot.role, EndpointRole::WindowsController);
    assert_eq!(snapshot.diagnostic_notice, None);

    let serialized = serde_json::to_string(&snapshot).unwrap();
    let round_tripped: AppSnapshot = serde_json::from_str(&serialized).unwrap();
    assert_eq!(round_tripped, snapshot);
}

#[test]
fn diagnostic_notice_round_trips_without_losing_the_fallback_warning() {
    let state = RuntimeState::new(
        EndpointRole::MacEndpoint,
        BuildInfo {
            version: "0.1.0".into(),
            build_id: "warning-build".into(),
        },
        Some(DiagnosticNotice {
            code: DiagnosticNoticeCode::StorageFallback,
            message: "安装目录不可写，日志已切换到备用目录".into(),
            path: Some("/tmp/kvmflow-emergency-42".into()),
        }),
    );
    let snapshot = AppSnapshot::from(&state);

    assert_eq!(snapshot.schema_version, 2);
    let notice = snapshot.diagnostic_notice.as_ref().unwrap();
    assert_eq!(notice.code, DiagnosticNoticeCode::StorageFallback);
    assert_eq!(notice.message, "安装目录不可写，日志已切换到备用目录");
    assert_eq!(notice.path.as_deref(), Some("/tmp/kvmflow-emergency-42"));

    let serialized = serde_json::to_string(&snapshot).unwrap();
    let round_tripped: AppSnapshot = serde_json::from_str(&serialized).unwrap();
    assert_eq!(round_tripped, snapshot);
}
