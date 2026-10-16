use std::{net::TcpListener, time::Duration};

use kvmflow_domain::{
    ClipboardFile, ClipboardPayload, EndpointRole, InputEvent, KeyAction, KeyUsage,
    NetworkEndpoint, ScreenEdge,
};
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
        .send(RuntimeCommand::ConfigureEndpoint {
            endpoint: NetworkEndpoint {
                ip: "127.0.0.1".into(),
                port,
            },
            automatic: false,
        })
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

    let source_file = windows_directory.path().join("clipboard-note.txt");
    std::fs::write(&source_file, b"file clipboard payload").expect("write clipboard source");
    let source_folder = windows_directory.path().join("clipboard-folder");
    let source_nested_folder = source_folder.join("nested");
    std::fs::create_dir_all(&source_nested_folder).expect("create clipboard source folder");
    std::fs::write(
        source_nested_folder.join("payload.bin"),
        b"nested clipboard folder payload",
    )
    .expect("write nested clipboard source");
    let windows_clipboard = ClipboardPayload {
        id: "windows-clipboard-1".into(),
        captured_at_unix_ms: 100,
        text: Some("x".repeat(40 * 1024)),
        html: Some("<strong>clipboard</strong>".into()),
        rich_text: None,
        image_png: None,
        files: vec![
            ClipboardFile {
                name: "clipboard-note.txt".into(),
                path: source_file,
            },
            ClipboardFile {
                name: "clipboard-folder".into(),
                path: source_folder,
            },
        ],
    };
    windows
        .handle
        .send(RuntimeCommand::SetClipboard(windows_clipboard.clone()))
        .await
        .expect("send Windows clipboard");
    let RuntimeEvent::ClipboardReceived(mac_clipboard) = next_matching(&mut mac, |event| {
        matches!(event, RuntimeEvent::ClipboardReceived(_))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(mac_clipboard.text, windows_clipboard.text);
    assert_eq!(mac_clipboard.html, windows_clipboard.html);
    assert_eq!(mac_clipboard.files.len(), 2);
    let transferred_file = mac_clipboard
        .files
        .iter()
        .find(|file| file.name == "clipboard-note.txt")
        .expect("transferred clipboard file");
    assert_eq!(
        std::fs::read(&transferred_file.path).expect("read transferred clipboard file"),
        b"file clipboard payload"
    );
    let transferred_folder = mac_clipboard
        .files
        .iter()
        .find(|file| file.name == "clipboard-folder")
        .expect("transferred clipboard folder");
    assert_eq!(
        std::fs::read(transferred_folder.path.join("nested/payload.bin"))
            .expect("read transferred nested clipboard file"),
        b"nested clipboard folder payload"
    );

    let mac_clipboard = ClipboardPayload {
        id: "mac-clipboard-1".into(),
        captured_at_unix_ms: 101,
        text: Some("back from Mac".into()),
        html: None,
        rich_text: None,
        image_png: None,
        files: Vec::new(),
    };
    mac.handle
        .send(RuntimeCommand::SetClipboard(mac_clipboard.clone()))
        .await
        .expect("send Mac clipboard");
    assert_eq!(
        next_matching(&mut windows, |event| matches!(
            event,
            RuntimeEvent::ClipboardReceived(_)
        ))
        .await,
        RuntimeEvent::ClipboardReceived(mac_clipboard)
    );

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

    let large_source = windows_directory.path().join("large-clipboard.bin");
    std::fs::write(&large_source, vec![0x5a; 32 * 1024 * 1024])
        .expect("write large clipboard source");
    windows
        .handle
        .send(RuntimeCommand::SetClipboard(ClipboardPayload {
            id: "large-windows-clipboard".into(),
            captured_at_unix_ms: 102,
            text: None,
            html: None,
            rich_text: None,
            image_png: None,
            files: vec![ClipboardFile {
                name: "large-clipboard.bin".into(),
                path: large_source,
            }],
        }))
        .await
        .expect("start large clipboard transfer");
    assert!(matches!(
        next_matching(&mut mac, |event| matches!(
            event,
            RuntimeEvent::ClipboardTransferChanged {
                state: kvmflow_domain::ClipboardTransferState::Receiving,
                ..
            }
        ))
        .await,
        RuntimeEvent::ClipboardTransferChanged { .. }
    ));
    let control_during_transfer = key_event(KeyUsage::A, KeyAction::Down);
    windows
        .handle
        .send(RuntimeCommand::SendInput(control_during_transfer.clone()))
        .await
        .expect("send control during clipboard transfer");
    next_input_before_clipboard(&mut mac, &control_during_transfer).await;
    let RuntimeEvent::ClipboardReceived(large_clipboard) = next_matching(&mut mac, |event| {
        matches!(event, RuntimeEvent::ClipboardReceived(_))
    })
    .await
    else {
        unreachable!()
    };
    assert_eq!(
        std::fs::metadata(&large_clipboard.files[0].path)
            .expect("large transferred file")
            .len(),
        32 * 1024 * 1024
    );
    let control_release = key_event(KeyUsage::A, KeyAction::Up);
    windows
        .handle
        .send(RuntimeCommand::SendInput(control_release.clone()))
        .await
        .expect("release control key");
    assert_eq!(
        next_matching(&mut mac, |event| matches!(
            event,
            RuntimeEvent::InputReceived(_)
        ))
        .await,
        RuntimeEvent::InputReceived(control_release)
    );

    let expected = [
        key_event(KeyUsage::LEFT_GUI, KeyAction::Down),
        key_event(KeyUsage::LEFT_SHIFT, KeyAction::Down),
        key_event(KeyUsage::UP_ARROW, KeyAction::Down),
        key_event(KeyUsage::UP_ARROW, KeyAction::Up),
        key_event(KeyUsage::LEFT_SHIFT, KeyAction::Up),
        key_event(KeyUsage::LEFT_GUI, KeyAction::Up),
    ];
    for event in &expected {
        windows
            .handle
            .send(RuntimeCommand::SendInput(event.clone()))
            .await
            .expect("send input transition");
    }
    for event in expected {
        assert_eq!(
            next_matching(&mut mac, |event| matches!(
                event,
                RuntimeEvent::InputReceived(_)
            ))
            .await,
            RuntimeEvent::InputReceived(event)
        );
    }
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

fn key_event(usage: KeyUsage, action: KeyAction) -> InputEvent {
    InputEvent::Key { usage, action }
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

async fn next_input_before_clipboard(runtime: &mut RuntimeBridge, expected: &InputEvent) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match runtime.events.recv().await.expect("runtime event stream") {
                RuntimeEvent::InputReceived(event) if &event == expected => return,
                RuntimeEvent::ClipboardReceived(_) => {
                    panic!("clipboard transfer completed before control input arrived")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("timed out waiting for control input during clipboard transfer");
}
