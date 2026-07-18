use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrustState {
    Unpaired,
    Pairing,
    Paired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkState {
    Idle,
    Listening,
    Connecting,
    Authenticating,
    Online,
    Backoff,
    PortBindFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlState {
    Local,
    EnteringRemote,
    Remote,
    ReturningLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionStatus {
    Unknown,
    Missing,
    Granted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalControlPolicy {
    Allowed,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScreenEdge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkEndpoint {
    pub ip: String,
    pub port: u16,
}

impl NetworkEndpoint {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.ip.parse::<std::net::Ipv4Addr>().is_err() {
            return Err("IP 地址必须是有效的 IPv4 地址");
        }
        if self.port < 1024 {
            return Err("端口必须在 1024-65535 范围内");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStatus {
    pub code: String,
    pub local_confirmed: bool,
    pub remote_confirmed: bool,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionErrorStage {
    PortBind,
    Reachability,
    Tls,
    Pairing,
    Authentication,
    Permission,
    InputAdapter,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionError {
    pub code: String,
    pub stage: ConnectionErrorStage,
    pub message: String,
    pub recovery: String,
    pub attempt_id: Option<u64>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvmStatus {
    pub trust: TrustState,
    pub link: LinkState,
    pub control: ControlState,
    pub permission: PermissionStatus,
    pub local_policy: LocalControlPolicy,
    pub screen_edge: ScreenEdge,
    pub endpoint: Option<NetworkEndpoint>,
    pub peer_device_name: Option<String>,
    pub pairing: Option<PairingStatus>,
    pub latency_ms: Option<f32>,
    pub session_id: Option<String>,
    pub last_error: Option<ConnectionError>,
}

impl Default for KvmStatus {
    fn default() -> Self {
        Self {
            trust: TrustState::Unpaired,
            link: LinkState::Idle,
            control: ControlState::Local,
            permission: PermissionStatus::Unknown,
            local_policy: LocalControlPolicy::Allowed,
            screen_edge: ScreenEdge::Bottom,
            endpoint: None,
            peer_device_name: None,
            pairing: None,
            latency_ms: None,
            session_id: None,
            last_error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_accepts_only_ipv4_and_non_privileged_ports() {
        assert!(
            NetworkEndpoint {
                ip: "192.168.1.8".into(),
                port: 45_888,
            }
            .validate()
            .is_ok()
        );
        assert!(
            NetworkEndpoint {
                ip: "mac.local".into(),
                port: 45_888,
            }
            .validate()
            .is_err()
        );
        assert!(
            NetworkEndpoint {
                ip: "192.168.1.8".into(),
                port: 443,
            }
            .validate()
            .is_err()
        );
    }
}
