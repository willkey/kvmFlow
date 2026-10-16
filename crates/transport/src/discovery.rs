use std::{collections::BTreeMap, net::Ipv4Addr};

use kvmflow_domain::NetworkEndpoint;
use mdns_sd::{DaemonEvent, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;
use tokio::{sync::watch, task::JoinHandle};

use crate::{DeviceIdentity, TrustedPeer};

const SERVICE_TYPE: &str = "_kvmflow._tcp.local.";
const MAX_DISCOVERED_DEVICES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub device_name: String,
    pub endpoint: NetworkEndpoint,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverySnapshot {
    pub available: bool,
    pub devices: Vec<DiscoveredDevice>,
    pub error: Option<String>,
}

impl DiscoverySnapshot {
    pub(crate) fn trusted_endpoint(&self, peer: Option<&TrustedPeer>) -> Option<NetworkEndpoint> {
        if !self.available {
            return None;
        }
        let peer = peer?;
        self.devices
            .iter()
            .find(|device| device.device_id == peer.device_id)
            .map(|device| device.endpoint.clone())
    }
}

pub(crate) struct LanService {
    daemon: ServiceDaemon,
    tasks: Vec<JoinHandle<()>>,
    announced: Option<(String, u16, Vec<Ipv4Addr>)>,
}

impl LanService {
    fn create(
        daemon: ServiceDaemon,
        snapshot: Option<watch::Sender<DiscoverySnapshot>>,
    ) -> Result<Self, mdns_sd::Error> {
        let monitor = match daemon.monitor() {
            Ok(monitor) => monitor,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(error);
            }
        };
        let task = tokio::spawn(async move {
            while let Ok(event) = monitor.recv_async().await {
                if let DaemonEvent::Error(error) = event {
                    tracing::warn!(component = "discovery", %error, "LAN discovery daemon failed");
                    if let Some(snapshot) = snapshot.as_ref() {
                        snapshot.send_modify(|state| {
                            state.available = false;
                            state.devices.clear();
                            state.error =
                                Some(format!("局域网发现不可用：{error}，可使用手动 IP 连接"));
                        });
                    }
                }
            }
        });
        Ok(Self {
            daemon,
            tasks: vec![task],
            announced: None,
        })
    }

    pub(crate) fn browse(
        snapshot: watch::Sender<DiscoverySnapshot>,
    ) -> Result<Self, mdns_sd::Error> {
        Self::browse_on(ServiceDaemon::new()?, snapshot)
    }

    fn browse_on(
        daemon: ServiceDaemon,
        snapshot: watch::Sender<DiscoverySnapshot>,
    ) -> Result<Self, mdns_sd::Error> {
        let mut service = Self::create(daemon, Some(snapshot.clone()))?;
        let receiver = service.daemon.browse(SERVICE_TYPE)?;
        snapshot.send_replace(DiscoverySnapshot {
            available: true,
            ..Default::default()
        });
        service.tasks.push(tokio::spawn(async move {
            let mut devices = BTreeMap::new();
            while let Ok(event) = receiver.recv_async().await {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let parsed = parse_device(
                            info.get_property_val_str("role"),
                            info.get_property_val_str("version"),
                            info.get_property_val_str("id"),
                            info.get_property_val_str("name"),
                            info.get_port(),
                            info.get_addresses_v4().into_iter().collect(),
                        );
                        if let Some(device) = parsed {
                            if devices.len() < MAX_DISCOVERED_DEVICES
                                || devices.contains_key(info.get_fullname())
                            {
                                devices.insert(info.get_fullname().to_owned(), device);
                            }
                        } else {
                            devices.remove(info.get_fullname());
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, name) => {
                        devices.remove(&name);
                    }
                    _ => continue,
                }
                let mut visible: Vec<DiscoveredDevice> = devices.values().cloned().collect();
                visible.sort_by(|a, b| {
                    (&a.device_id, &a.endpoint.ip).cmp(&(&b.device_id, &b.endpoint.ip))
                });
                visible.dedup_by(|a, b| a.device_id == b.device_id);
                snapshot.send_if_modified(|state| {
                    let next = DiscoverySnapshot {
                        available: true,
                        devices: visible,
                        error: None,
                    };
                    if *state == next {
                        false
                    } else {
                        *state = next;
                        true
                    }
                });
            }
            snapshot.send_modify(|state| {
                state.available = false;
                state.devices.clear();
                state.error = Some("局域网发现已停止，可使用手动 IP 连接".into());
            });
        }));
        Ok(service)
    }

    pub(crate) fn advertiser() -> Result<Self, mdns_sd::Error> {
        Self::create(ServiceDaemon::new()?, None)
    }

    pub(crate) fn announce(
        &mut self,
        identity: &DeviceIdentity,
        port: Option<u16>,
        addresses: Vec<Ipv4Addr>,
    ) {
        let Some(port) = port.filter(|_| !addresses.is_empty()) else {
            self.withdraw();
            return;
        };
        if self
            .announced
            .as_ref()
            .is_some_and(|(_, previous_port, previous_addresses)| {
                *previous_port == port && *previous_addresses == addresses
            })
        {
            return;
        }
        let id = identity.device_id();
        let properties = [
            ("role", "mac"),
            ("version", "1"),
            ("id", id.as_str()),
            ("name", identity.device_name()),
        ];
        let addresses = addresses
            .into_iter()
            .filter(|address| usable_ipv4(*address))
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            self.withdraw();
            return;
        }
        let wire_addresses = addresses
            .iter()
            .copied()
            .map(std::net::IpAddr::V4)
            .collect::<Vec<_>>();
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &format!("kvmFlow-{id}"),
            &format!("kvmflow-{id}.local."),
            wire_addresses.as_slice(),
            port,
            &properties[..],
        );
        match info.and_then(|info| {
            let name = info.get_fullname().to_owned();
            self.daemon.register(info)?;
            Ok(name)
        }) {
            Ok(name) => {
                self.announced = Some((name, port, addresses));
                tracing::info!(
                    component = "discovery",
                    port,
                    "Mac LAN service announcement updated"
                );
            }
            Err(error) => {
                tracing::warn!(component = "discovery", %error, "Mac LAN announcement failed; manual connection remains available")
            }
        }
    }

    pub(crate) fn withdraw(&mut self) {
        if let Some((name, _, _)) = self.announced.take() {
            let _ = self.daemon.unregister(&name);
        }
    }
}

impl Drop for LanService {
    fn drop(&mut self) {
        self.withdraw();
        let _ = self.daemon.shutdown();
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn usable_ipv4(address: Ipv4Addr) -> bool {
    (address.is_private() || address.is_link_local()) && !address.is_unspecified()
}

fn parse_device(
    role: Option<&str>,
    version: Option<&str>,
    id: Option<&str>,
    name: Option<&str>,
    port: u16,
    addresses: Vec<Ipv4Addr>,
) -> Option<DiscoveredDevice> {
    if role != Some("mac") || version != Some("1") || port < 1024 {
        return None;
    }
    let id = id.filter(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))?;
    let name = name.filter(|name| {
        !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
    })?;
    let interfaces = if_addrs::get_if_addrs().unwrap_or_default();
    let address = addresses
        .into_iter()
        .filter(|address| usable_ipv4(*address))
        .min_by_key(|address| {
            let same_subnet = interfaces.iter().any(|interface| match &interface.addr {
                if_addrs::IfAddr::V4(local) => {
                    (u32::from(local.ip) & u32::from(local.netmask))
                        == (u32::from(*address) & u32::from(local.netmask))
                }
                _ => false,
            });
            (!same_subnet, u32::from(*address))
        })?;
    Some(DiscoveredDevice {
        device_id: id.into(),
        device_name: name.into(),
        endpoint: NetworkEndpoint {
            ip: address.to_string(),
            port,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvmflow_domain::EndpointRole;

    #[tokio::test]
    async fn announcement_discovery_port_updates_and_withdrawal_round_trip() {
        let address = if_addrs::get_if_addrs()
            .unwrap()
            .into_iter()
            .find_map(|interface| match interface.addr {
                if_addrs::IfAddr::V4(address) if usable_ipv4(address.ip) => Some(address.ip),
                _ => None,
            })
            .expect("a LAN interface for multicast discovery");
        let (snapshot_tx, mut snapshots) = watch::channel(DiscoverySnapshot::default());
        let browser_daemon = ServiceDaemon::new().unwrap();
        browser_daemon.set_multicast_loop_v4(true).unwrap();
        let _browser = LanService::browse_on(browser_daemon, snapshot_tx).unwrap();
        let publisher_daemon = ServiceDaemon::new().unwrap();
        publisher_daemon.set_multicast_loop_v4(true).unwrap();
        let mut publisher = LanService::create(publisher_daemon, None).unwrap();
        let identity = DeviceIdentity::generate(EndpointRole::MacEndpoint, "Discovery test Mac");
        publisher.announce(&identity, Some(49991), vec![address]);
        wait_for_port(&mut snapshots, &identity.device_id(), Some(49991)).await;
        publisher.announce(&identity, Some(49992), vec![address]);
        wait_for_port(&mut snapshots, &identity.device_id(), Some(49992)).await;
        publisher.withdraw();
        wait_for_port(&mut snapshots, &identity.device_id(), None).await;
    }

    async fn wait_for_port(
        snapshots: &mut watch::Receiver<DiscoverySnapshot>,
        id: &str,
        port: Option<u16>,
    ) {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let actual = snapshots
                    .borrow_and_update()
                    .devices
                    .iter()
                    .find(|device| device.device_id == id)
                    .map(|device| device.endpoint.port);
                if actual == port {
                    return;
                }
                snapshots.changed().await.unwrap();
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "expected multicast port {port:?}, got {:?}",
                snapshots.borrow()
            )
        });
    }

    #[test]
    fn discovery_filters_invalid_advertisements_and_uses_the_announced_port() {
        let identity = DeviceIdentity::generate(EndpointRole::MacEndpoint, "Mac");
        let id = identity.device_id();
        let addresses = vec![
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::new(198, 18, 0, 1),
            Ipv4Addr::new(192, 168, 1, 8),
        ];
        let device = parse_device(
            Some("mac"),
            Some("1"),
            Some(&id),
            Some("Mac"),
            49999,
            addresses.clone(),
        )
        .unwrap();
        assert_eq!(
            device.endpoint,
            NetworkEndpoint {
                ip: "192.168.1.8".into(),
                port: 49999
            }
        );
        assert!(
            parse_device(
                Some("windows"),
                Some("1"),
                Some(&id),
                Some("Mac"),
                49999,
                addresses.clone()
            )
            .is_none()
        );
        assert!(
            parse_device(
                Some("mac"),
                Some("2"),
                Some(&id),
                Some("Mac"),
                49999,
                addresses.clone()
            )
            .is_none()
        );
        assert!(
            parse_device(
                Some("mac"),
                Some("1"),
                Some("invalid"),
                Some("Mac"),
                49999,
                addresses.clone()
            )
            .is_none()
        );
        assert!(
            parse_device(
                Some("mac"),
                Some("1"),
                Some(&id),
                Some("Mac"),
                80,
                addresses
            )
            .is_none()
        );
        let snapshot = DiscoverySnapshot {
            available: true,
            devices: vec![device],
            error: None,
        };
        assert!(snapshot.trusted_endpoint(None).is_none());
        assert_eq!(
            snapshot.trusted_endpoint(Some(&TrustedPeer::from_identity(&identity))),
            Some(NetworkEndpoint {
                ip: "192.168.1.8".into(),
                port: 49999
            })
        );
        let other = DeviceIdentity::generate(EndpointRole::MacEndpoint, "Other");
        assert!(
            snapshot
                .trusted_endpoint(Some(&TrustedPeer::from_identity(&other)))
                .is_none()
        );
    }
}
