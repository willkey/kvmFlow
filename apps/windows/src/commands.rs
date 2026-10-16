use kvmflow_domain::{AppSnapshot, DisplayTarget, NetworkEndpoint, ScreenEdge};
use kvmflow_engine::{EngineCommand, EngineHandle};
use kvmflow_platform_windows::available_windows_displays;
use tauri_plugin_autostart::AutoLaunchManager;

fn snapshot_payload(engine: &EngineHandle) -> AppSnapshot {
    engine.current_snapshot()
}

#[tauri::command]
pub fn get_discovery(
    engine: tauri::State<'_, EngineHandle>,
) -> kvmflow_transport::DiscoverySnapshot {
    engine.discovery_snapshot()
}

#[tauri::command]
pub fn get_snapshot(engine: tauri::State<'_, EngineHandle>) -> AppSnapshot {
    snapshot_payload(&engine)
}

#[tauri::command]
pub fn get_autostart_enabled(manager: tauri::State<'_, AutoLaunchManager>) -> Result<bool, String> {
    manager.is_enabled().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_autostart_enabled(
    enabled: bool,
    manager: tauri::State<'_, AutoLaunchManager>,
) -> Result<bool, String> {
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| error.to_string())?;
    let actual = manager.is_enabled().map_err(|error| error.to_string())?;
    if actual != enabled {
        return Err("系统未能保存开机自启动设置".into());
    }
    tracing::info!(
        component = "autostart",
        enabled = actual,
        "Windows autostart preference changed"
    );
    Ok(actual)
}

#[tauri::command]
pub async fn export_foundation_report(
    engine: tauri::State<'_, EngineHandle>,
) -> Result<String, String> {
    engine
        .export_report()
        .await
        .map(|path| path.display().to_string())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn connect_discovered(
    device_id: String,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    let discovery = engine.discovery_snapshot();
    let device = discovery
        .devices
        .into_iter()
        .find(|device| device.device_id == device_id)
        .filter(|_| discovery.available)
        .ok_or("设备已离线，请等待重新发现或手动连接")?;
    device.endpoint.validate().map_err(str::to_owned)?;
    engine
        .send(EngineCommand::ConfigureDiscoveredEndpoint(device.endpoint))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn connect_manual(
    ip: String,
    port: u16,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    let endpoint = NetworkEndpoint { ip, port };
    endpoint.validate().map_err(str::to_owned)?;
    engine
        .send(EngineCommand::ConfigureEndpoint(endpoint))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn confirm_pairing(engine: tauri::State<'_, EngineHandle>) -> Result<(), String> {
    engine
        .send(EngineCommand::ConfirmPairing)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn reject_pairing(engine: tauri::State<'_, EngineHandle>) -> Result<(), String> {
    engine
        .send(EngineCommand::RejectPairing)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn enter_control(
    edge: ScreenEdge,
    position: f32,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    engine
        .send(EngineCommand::EnterControl { edge, position })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn configure_screen_edge(
    edge: ScreenEdge,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    engine
        .send(EngineCommand::ConfigureScreenEdge(edge))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_displays() -> Result<Vec<DisplayTarget>, String> {
    available_windows_displays().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn configure_target_display(
    display_id: String,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    let displays = available_windows_displays().map_err(|error| error.to_string())?;
    if !displays.iter().any(|display| display.id == display_id) {
        return Err("选择的显示器当前不可用".into());
    }
    engine
        .send(EngineCommand::ConfigureTargetDisplay(display_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn reset_input(engine: tauri::State<'_, EngineHandle>) -> Result<(), String> {
    engine
        .send(EngineCommand::ResetInput)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn return_control(engine: tauri::State<'_, EngineHandle>) -> Result<(), String> {
    engine
        .send(EngineCommand::ReturnControl)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_control_enabled(
    enabled: bool,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    engine
        .send(EngineCommand::SetControlEnabled(enabled))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_game_mode_enabled(
    enabled: bool,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    engine
        .send(EngineCommand::SetGameModeEnabled(enabled))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn activate_clipboard_history(
    id: String,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    engine
        .send(EngineCommand::ActivateClipboardHistory(id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn clear_clipboard_history(engine: tauri::State<'_, EngineHandle>) -> Result<(), String> {
    engine
        .send(EngineCommand::ClearClipboardHistory)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use kvmflow_domain::{AppSnapshot, EndpointRole};
    use serde_json::json;
    use tauri::WebviewWindowBuilder;

    use crate::{configure_builder, test_support::test_engine};

    #[test]
    fn get_snapshot_is_registered_and_invokes_managed_engine_through_ipc() {
        let (_diagnostics, engine) = test_engine("ipc-command-test");
        let app = configure_builder(tauri::test::mock_builder())
            .manage(engine.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mock Tauri app");
        let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("build mock webview");
        let response = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "get_snapshot".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .expect("parse IPC origin"),
                body: tauri::ipc::InvokeBody::default(),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .expect("get_snapshot IPC response")
        .deserialize::<AppSnapshot>()
        .expect("deserialize snapshot response");

        assert_eq!(response, engine.current_snapshot());
        let snapshot = response;
        assert_eq!(snapshot.schema_version, 6);
        assert_eq!(snapshot.role, EndpointRole::WindowsController);
        tauri::async_runtime::block_on(engine.shutdown()).expect("shut down engine");
    }

    #[test]
    fn enter_control_accepts_the_frontend_ipc_payload() {
        let (_diagnostics, engine) = test_engine("enter-control-ipc-test");
        let app = configure_builder(tauri::test::mock_builder())
            .manage(engine.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build mock Tauri app");
        let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("build mock webview");

        let response = tauri::test::get_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "enter_control".into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .expect("parse IPC origin"),
                body: tauri::ipc::InvokeBody::Json(json!({
                    "edge": "bottom",
                    "position": 0.5
                })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );

        assert!(response.is_ok(), "enter_control IPC failed: {response:?}");
        tauri::async_runtime::block_on(engine.shutdown()).expect("shut down engine");
    }
}
