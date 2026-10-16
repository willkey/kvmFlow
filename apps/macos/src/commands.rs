use kvmflow_domain::{AppSnapshot, LocalControlPolicy};
use kvmflow_engine::{EngineCommand, EngineHandle};
use tauri_plugin_autostart::AutoLaunchManager;

const AUTOSTART_PERMISSION_MESSAGE: &str = "无法写入 Mac 登录项目录 ~/Library/LaunchAgents：该目录应属于当前登录用户。修复目录所有者后重试。";

fn mac_autostart_error(operation: &'static str, error: impl std::fmt::Display) -> String {
    let detail = error.to_string();
    tracing::error!(
        component = "autostart",
        operation,
        error = %detail,
        "Mac autostart operation failed"
    );
    if detail.contains("Permission denied") || detail.contains("os error 13") {
        AUTOSTART_PERMISSION_MESSAGE.into()
    } else {
        format!("{operation}开机自动启动失败：{detail}")
    }
}

#[tauri::command]
pub fn get_snapshot(engine: tauri::State<'_, EngineHandle>) -> AppSnapshot {
    engine.current_snapshot()
}

#[tauri::command]
pub fn get_autostart_enabled(manager: tauri::State<'_, AutoLaunchManager>) -> Result<bool, String> {
    manager
        .is_enabled()
        .map_err(|error| mac_autostart_error("读取", error))
}

#[tauri::command]
pub fn set_autostart_enabled(
    enabled: bool,
    manager: tauri::State<'_, AutoLaunchManager>,
) -> Result<bool, String> {
    let operation = if enabled { "开启" } else { "关闭" };
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| mac_autostart_error(operation, error))?;
    let actual = manager
        .is_enabled()
        .map_err(|error| mac_autostart_error("校验", error))?;
    if actual != enabled {
        tracing::error!(
            component = "autostart",
            requested = enabled,
            actual,
            "Mac autostart preference verification failed"
        );
        return Err("系统未能保存开机自启动设置".into());
    }
    tracing::info!(
        component = "autostart",
        enabled = actual,
        "Mac autostart preference changed"
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

#[cfg(test)]
mod tests {
    use kvmflow_domain::{AppSnapshot, EndpointRole};
    use tauri::WebviewWindowBuilder;

    use super::{AUTOSTART_PERMISSION_MESSAGE, mac_autostart_error};
    use crate::{configure_builder, test_support::test_engine};

    #[test]
    fn autostart_permission_error_explains_the_launch_agents_ownership_problem() {
        assert_eq!(
            mac_autostart_error("开启", "Permission denied (os error 13)"),
            AUTOSTART_PERMISSION_MESSAGE
        );
    }

    #[test]
    fn other_autostart_errors_keep_the_operation_and_original_detail() {
        assert_eq!(
            mac_autostart_error("关闭", "unexpected failure"),
            "关闭开机自动启动失败：unexpected failure"
        );
    }

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
        assert_eq!(response.schema_version, 6);
        assert_eq!(response.role, EndpointRole::MacEndpoint);
        tauri::async_runtime::block_on(engine.shutdown()).expect("shut down engine");
    }
}
