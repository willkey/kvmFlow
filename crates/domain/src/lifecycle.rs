use serde::{Deserialize, Serialize};

use crate::KvmStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EndpointRole {
    WindowsController,
    MacEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimePhase {
    Starting,
    Running,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateStatus {
    NotObserved,
    Collecting,
    Passed,
    Failed,
    ManualConfirmationRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticNoticeCode {
    StorageFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    pub version: String,
    pub build_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticNotice {
    pub code: DiagnosticNoticeCode,
    pub message: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSample {
    pub observed_at_unix_ms: u64,
    pub uptime_seconds: u64,
    pub window_state: WindowState,
    pub process_count: u32,
    pub resident_memory_bytes: u64,
    pub cpu_percent: f32,
    pub cpu_valid: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricProgress {
    pub status: GateStatus,
    pub current: Option<f64>,
    pub target: f64,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationProgress {
    pub background_memory: MetricProgress,
    pub visible_memory: MetricProgress,
    pub background_cpu: MetricProgress,
    pub continuous_uptime_seconds: u64,
    pub sample_count: u64,
    pub valid_cpu_sample_count: u64,
    pub peak_memory_bytes: Option<u64>,
    pub average_cpu_percent: Option<f64>,
    pub peak_cpu_percent: Option<f64>,
    pub kvm: KvmValidationProgress,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvmValidationProgress {
    pub successful_entries: u64,
    pub successful_returns: u64,
    pub transition_failures: u64,
    pub emergency_returns: u64,
    pub emergency_return_sample_count: u64,
    pub peak_emergency_return_ms: Option<f64>,
    pub disconnects: u64,
    pub reconnects: u64,
    pub network_recoveries: u64,
    pub wake_recoveries: u64,
    pub latency_sample_count: u64,
    pub average_latency_ms: Option<f64>,
    pub p95_latency_ms: Option<f64>,
    pub peak_latency_ms: Option<f64>,
}

impl Default for ValidationProgress {
    fn default() -> Self {
        Self {
            background_memory: MetricProgress {
                status: GateStatus::NotObserved,
                current: None,
                target: 52_428_800.0,
                unit: "bytes".into(),
            },
            visible_memory: MetricProgress {
                status: GateStatus::NotObserved,
                current: None,
                target: 188_743_680.0,
                unit: "bytes".into(),
            },
            background_cpu: MetricProgress {
                status: GateStatus::NotObserved,
                current: None,
                target: 0.5,
                unit: "percent".into(),
            },
            continuous_uptime_seconds: 0,
            sample_count: 0,
            valid_cpu_sample_count: 0,
            peak_memory_bytes: None,
            average_cpu_percent: None,
            peak_cpu_percent: None,
            kvm: KvmValidationProgress::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    pub role: EndpointRole,
    pub phase: RuntimePhase,
    pub window_state: WindowState,
    pub build: BuildInfo,
    pub diagnostic_notice: Option<DiagnosticNotice>,
    pub latest_resource: Option<ResourceSample>,
    pub validation: ValidationProgress,
    pub kvm: KvmStatus,
}

impl RuntimeState {
    pub fn new(
        role: EndpointRole,
        build: BuildInfo,
        diagnostic_notice: Option<DiagnosticNotice>,
    ) -> Self {
        Self {
            role,
            phase: RuntimePhase::Starting,
            window_state: WindowState::Closed,
            build,
            diagnostic_notice,
            latest_resource: None,
            validation: ValidationProgress::default(),
            kvm: KvmStatus::default(),
        }
    }
}

// The locked public V2 contract requires these event payloads to remain inline.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineEvent {
    Started,
    WindowOpened,
    WindowClosed,
    ResourceUpdated {
        sample: ResourceSample,
        validation: ValidationProgress,
    },
    KvmStatusUpdated {
        status: KvmStatus,
    },
    ShutdownRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineEffect {
    PublishSnapshot,
    StopRuntime,
}

pub fn reduce(state: &mut RuntimeState, event: EngineEvent) -> Vec<EngineEffect> {
    match event {
        EngineEvent::Started => state.phase = RuntimePhase::Running,
        EngineEvent::WindowOpened => state.window_state = WindowState::Open,
        EngineEvent::WindowClosed => state.window_state = WindowState::Closed,
        EngineEvent::ResourceUpdated { sample, validation } => {
            state.latest_resource = Some(sample);
            state.validation = validation;
        }
        EngineEvent::KvmStatusUpdated { status } => state.kvm = status,
        EngineEvent::ShutdownRequested => {
            state.phase = RuntimePhase::Stopping;
            return vec![EngineEffect::PublishSnapshot, EngineEffect::StopRuntime];
        }
    }

    vec![EngineEffect::PublishSnapshot]
}
