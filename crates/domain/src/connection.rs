use serde::{Deserialize, Serialize};

use crate::ClipboardStatus;

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
pub struct DisplayTarget {
    pub id: String,
    pub primary: bool,
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl DisplayTarget {
    pub fn edge_is_occupied<'a>(
        &self,
        edge: ScreenEdge,
        displays: impl IntoIterator<Item = &'a DisplayTarget>,
    ) -> bool {
        let left = i64::from(self.left);
        let top = i64::from(self.top);
        let right = left + i64::from(self.width);
        let bottom = top + i64::from(self.height);

        displays.into_iter().any(|other| {
            if other.id == self.id {
                return false;
            }

            let other_left = i64::from(other.left);
            let other_top = i64::from(other.top);
            let other_right = other_left + i64::from(other.width);
            let other_bottom = other_top + i64::from(other.height);
            let vertical_overlap = top < other_bottom && other_top < bottom;
            let horizontal_overlap = left < other_right && other_left < right;

            match edge {
                ScreenEdge::Left => other_right == left && vertical_overlap,
                ScreenEdge::Right => other_left == right && vertical_overlap,
                ScreenEdge::Top => other_bottom == top && horizontal_overlap,
                ScreenEdge::Bottom => other_top == bottom && horizontal_overlap,
            }
        })
    }

    pub fn occupied_edges(&self, displays: &[DisplayTarget]) -> Vec<ScreenEdge> {
        [
            ScreenEdge::Left,
            ScreenEdge::Right,
            ScreenEdge::Top,
            ScreenEdge::Bottom,
        ]
        .into_iter()
        .filter(|edge| self.edge_is_occupied(*edge, displays))
        .collect()
    }
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
    pub control_enabled: bool,
    pub game_mode_enabled: bool,
    pub foreground_fullscreen: bool,
    pub remote_control_available: bool,
    pub clipboard: ClipboardStatus,
    pub screen_edge: ScreenEdge,
    pub target_display_id: Option<String>,
    pub endpoint: Option<NetworkEndpoint>,
    pub peer_device_name: Option<String>,
    pub pairing: Option<PairingStatus>,
    pub latency_ms: Option<f32>,
    pub session_id: Option<String>,
    pub consecutive_failures: u32,
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
            control_enabled: true,
            game_mode_enabled: true,
            foreground_fullscreen: false,
            remote_control_available: true,
            clipboard: ClipboardStatus::default(),
            screen_edge: ScreenEdge::Bottom,
            target_display_id: None,
            endpoint: None,
            peer_device_name: None,
            pairing: None,
            latency_ms: None,
            session_id: None,
            consecutive_failures: 0,
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

    #[test]
    fn adjacent_windows_displays_occupy_the_shared_edges() {
        let displays = vec![
            DisplayTarget {
                id: "primary".into(),
                primary: true,
                left: 0,
                top: 0,
                width: 1920,
                height: 1080,
            },
            DisplayTarget {
                id: "secondary".into(),
                primary: false,
                left: 1920,
                top: -180,
                width: 2560,
                height: 1440,
            },
        ];

        assert_eq!(
            displays[0].occupied_edges(&displays),
            vec![ScreenEdge::Right]
        );
        assert_eq!(
            displays[1].occupied_edges(&displays),
            vec![ScreenEdge::Left]
        );
    }

    #[test]
    fn corner_contact_and_desktop_gaps_leave_edges_available() {
        let target = DisplayTarget {
            id: "target".into(),
            primary: true,
            left: 0,
            top: 0,
            width: 1920,
            height: 1080,
        };
        let displays = vec![
            target.clone(),
            DisplayTarget {
                id: "corner".into(),
                primary: false,
                left: 1920,
                top: 1080,
                width: 800,
                height: 600,
            },
            DisplayTarget {
                id: "gap".into(),
                primary: false,
                left: -1281,
                top: 0,
                width: 1280,
                height: 720,
            },
        ];

        assert!(target.occupied_edges(&displays).is_empty());
    }
}
