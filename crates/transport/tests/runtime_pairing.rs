use std::{net::TcpListener, time::Duration};

use kvmflow_domain::{EndpointRole, InputEvent, KeyAction, KeyUsage, NetworkEndpoint, ScreenEdge};
use kvmflow_transport::{
    ConfigStore, DeviceIdentity, MacRuntimeConfig, RuntimeBridge, RuntimeCommand, RuntimeEvent,
    WindowsRuntimeConfig, spawn_macos_runtime, spawn_windows_runtime,
};

#[tokio::test]
async fn manual_pairing_reaches_online_and_forwards_input_without_reconnecting() {
    let port = available_port();
    let mac_directory = tempfile::tempdir().expect("create Mac data directory");
    let windows_directory = tempfile::tempdir().expect("create Windows data directory");
    let mac_store = ConfigStore::new(mac_directory.path());
    let windows_store = ConfigStore::new(windows_directory.path());
    let mac_identity = DeviceIdentity::generate(EndpointRole::MacEndpoint, "Loopback Mac");
    let windows_identity =
        DeviceIdentity::generate(EndpointRole::WindowsController, "Loopback Windows");
    let mac_config = MacRuntimeConfig {
        listen_port: port,
        ..MacRuntimeConfig::default()
    };
    let windows_config = WindowsRuntimeConfig::default();

    let mut mac = spawn_macos_runtime(mac_identity, mac_store.clone(), mac_config);
    assert!(matches!(
        next_matching(&mut mac, |event| matches!(
            event,
            RuntimeEvent::ListenerStarted { port: actual, .. } if *actual == port
        ))
        .await,
        RuntimeEvent::ListenerStarted { .. }
    ));

    let mut windows =
        spawn_windows_runtime(windows_identity, windows_store.clone(), windows_config);
    windows
        .handle
        .send(RuntimeCommand::ConfigureEndpoint(NetworkEndpoint {
            ip: "127.0.0.1".into(),
            port,
        }))
        .await
        .expect("configure endpoint");

    let windows_pairing = next_matching(&mut windows, |event| {
        matches!(event, RuntimeEvent::Pairing { .. })
    })
    .await;
    let mac_pairing = next_matching(&mut mac, |event| {
        matches!(event, RuntimeEvent::Pairing { .. })
    })
    .await;
    let RuntimeEvent::Pairing {
        code: windows_code, ..
    } = windows_pairing
    else {
        unreachable!()
    };
    let RuntimeEvent::Pairing { code: mac_code, .. } = mac_pairing else {
        unreachable!()
    };
    assert_eq!(windows_code, mac_code);

    windows
        .handle
        .send(RuntimeCommand::ConfirmPairing)
        .await
        .expect("confirm on Windows");
    mac.handle
        .send(RuntimeCommand::ConfirmPairing)
        .await
        .expect("confirm on Mac");

    let windows_online = next_matching(&mut windows, |event| {
        matches!(event, RuntimeEvent::Online { .. })
    })
    .await;
    let mac_online = next_matching(&mut mac, |event| {
        matches!(event, RuntimeEvent::Online { .. })
    })
    .await;
    let RuntimeEvent::Online {
        session_id: windows_session,
        ..
    } = windows_online
    else {
        unreachable!()
    };
    let RuntimeEvent::Online {
        session_id: mac_session,
        ..
    } = mac_online
    else {
        unreachable!()
    };
    assert_eq!(windows_session, mac_session);

    windows
        .handle
        .send(RuntimeCommand::EnterControl {
            edge: ScreenEdge::Bottom,
            position: 0.5,
        })
        .await
        .expect("request remote control");
    assert_eq!(
        next_matching(&mut mac, |event| matches!(
            event,
            RuntimeEvent::EnterControlRequested { .. }
        ))
        .await,
        RuntimeEvent::EnterControlRequested {
            edge: ScreenEdge::Bottom,
            position: 0.5,
        }
    );
    mac.handle
        .send(RuntimeCommand::RespondEnterControl {
            accepted: true,
            reason: None,
        })
        .await
        .expect("accept remote control");
    assert_eq!(
        next_matching(&mut windows, |event| matches!(
            event,
            RuntimeEvent::EnterControlResult { .. }
        ))
        .await,
        RuntimeEvent::EnterControlResult {
            accepted: true,
            reason: None,
        }
    );

    let expected = InputEvent::Key {
        usage: KeyUsage::A,
        action: KeyAction::Down,
    };
    windows
        .handle
        .send(RuntimeCommand::SendInput(expected.clone()))
        .await
        .expect("send input");
    assert_eq!(
        next_matching(&mut mac, |event| matches!(
            event,
            RuntimeEvent::InputReceived(_)
        ))
        .await,
        RuntimeEvent::InputReceived(expected)
    );
    assert!(matches!(
        next_matching(&mut windows, |event| matches!(
            event,
            RuntimeEvent::LatencyUpdated { .. }
        ))
        .await,
        RuntimeEvent::LatencyUpdated { .. }
    ));

    mac.handle
        .send(RuntimeCommand::ReturnControl { position: 0.25 })
        .await
        .expect("return control from Mac edge");
    assert_eq!(
        next_matching(&mut windows, |event| matches!(
            event,
            RuntimeEvent::ReturnControlRequested { .. }
        ))
        .await,
        RuntimeEvent::ReturnControlRequested { position: 0.25 }
    );
    assert_eq!(
        next_matching(&mut mac, |event| matches!(
            event,
            RuntimeEvent::ReturnControlAcknowledged
        ))
        .await,
        RuntimeEvent::ReturnControlAcknowledged
    );

    assert!(
        windows_store
            .load_windows()
            .expect("load Windows config")
            .trusted_peer
            .is_some()
    );
    assert!(
        mac_store
            .load_macos()
            .expect("load Mac config")
            .trusted_peer
            .is_some()
    );

    windows
        .handle
        .send(RuntimeCommand::Shutdown)
        .await
        .expect("stop Windows runtime");
    mac.handle
        .send(RuntimeCommand::Shutdown)
        .await
        .expect("stop Mac runtime");
}

fn available_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind temporary port")
        .local_addr()
        .expect("temporary address")
        .port()
}

async fn next_matching(
    runtime: &mut RuntimeBridge,
    predicate: impl Fn(&RuntimeEvent) -> bool,
) -> RuntimeEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = runtime.events.recv().await.expect("runtime event stream");
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("timed out waiting for runtime event")
}
