use serde::{Deserialize, Serialize};

use crate::{
    BuildInfo, DiagnosticNotice, EndpointRole, KvmStatus, ResourceSample, RuntimePhase,
    RuntimeState, ValidationProgress, WindowState,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub schema_version: u16,
    pub role: EndpointRole,
    pub phase: RuntimePhase,
    pub window_state: WindowState,
    pub build: BuildInfo,
    pub diagnostic_notice: Option<DiagnosticNotice>,
    pub latest_resource: Option<ResourceSample>,
    pub validation: ValidationProgress,
    pub kvm: KvmStatus,
}

impl From<&RuntimeState> for AppSnapshot {
    fn from(state: &RuntimeState) -> Self {
        Self {
            schema_version: 2,
            role: state.role,
            phase: state.phase,
            window_state: state.window_state,
            build: state.build.clone(),
            diagnostic_notice: state.diagnostic_notice.clone(),
            latest_resource: state.latest_resource,
            validation: state.validation.clone(),
            kvm: state.kvm.clone(),
        }
    }
}
