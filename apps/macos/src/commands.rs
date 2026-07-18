use kvmflow_domain::{AppSnapshot, LocalControlPolicy};
use kvmflow_engine::{EngineCommand, EngineHandle};

#[tauri::command]
pub fn get_snapshot(engine: tauri::State<'_, EngineHandle>) -> AppSnapshot {
    engine.current_snapshot()
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
pub async fn configure_listen_port(
    port: u16,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    if port < 1024 {
        return Err("端口必须在 1024-65535 范围内".into());
    }
    engine
        .send(EngineCommand::ConfigureListenPort(port))
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
pub async fn set_local_control_allowed(
    allowed: bool,
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    engine
        .send(EngineCommand::SetLocalPolicy(if allowed {
            LocalControlPolicy::Allowed
        } else {
            LocalControlPolicy::Paused
        }))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn refresh_accessibility_permission(
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    engine
        .send(EngineCommand::RefreshPermission)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn open_accessibility_settings(
    engine: tauri::State<'_, EngineHandle>,
) -> Result<(), String> {
    engine
        .send(EngineCommand::OpenAccessibilitySettings)
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

#[cfg(test)]
mod tests {
    use kvmflow_domain::{AppSnapshot, EndpointRole};
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
        assert_eq!(response.schema_version, 2);
        assert_eq!(response.role, EndpointRole::MacEndpoint);
        tauri::async_runtime::block_on(engine.shutdown()).expect("shut down engine");
    }
}
