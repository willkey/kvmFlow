#![forbid(unsafe_code)]

use std::{path::PathBuf, sync::Mutex};

use kvmflow_diagnostics::{
    ExecutableKind, LoggingGuard, PERSIST_INTERVAL, PreparedStorage, RetentionPolicy,
    SAMPLE_INTERVAL, SysinfoResourceSampler, ValidationMonitor, ValidationStore, init_file_logging,
    prepare_storage,
};
use kvmflow_domain::{
    AppSnapshot, BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole,
};
#[cfg(test)]
use kvmflow_engine::spawn_engine;
use kvmflow_engine::{EngineConfig, EngineHandle, spawn_engine_with_kvm};
use kvmflow_platform_windows::spawn_windows_platform_adapter;
use kvmflow_transport::{ConfigStore, load_or_create_identity, spawn_windows_runtime};
use tauri::{Emitter, Manager};

mod commands;
mod tray;
mod window;

struct LoggingGuardOwner {
    _guard: Mutex<LoggingGuard>,
}

fn storage_notice(prepared: &PreparedStorage) -> Option<DiagnosticNotice> {
    prepared.warning.as_ref().map(|_| DiagnosticNotice {
        code: DiagnosticNoticeCode::StorageFallback,
        message: "安装目录不可写，日志已切换到备用目录".into(),
        path: Some(prepared.layout.root.display().to_string()),
    })
}

fn with_tauri_runtime<T>(operation: impl FnOnce() -> T) -> T {
    let runtime = tauri::async_runtime::handle();
    let _runtime_guard = runtime.inner().enter();
    operation()
}

async fn forward_snapshots(
    mut snapshots: tokio::sync::watch::Receiver<AppSnapshot>,
    mut publish: impl FnMut(AppSnapshot),
) {
    while snapshots.changed().await.is_ok() {
        publish(snapshots.borrow_and_update().clone());
    }
}

fn start_snapshot_emitter(app: &tauri::AppHandle, engine: &EngineHandle) {
    let app = app.clone();
    let engine = engine.clone();
    let snapshots = engine.subscribe();
    let mut recovery_notice_shown = false;
    tauri::async_runtime::spawn(forward_snapshots(snapshots, move |snapshot| {
        if snapshot.kvm.consecutive_failures == 0 {
            recovery_notice_shown = false;
        } else if snapshot.kvm.consecutive_failures >= 3 && !recovery_notice_shown {
            recovery_notice_shown = true;
            if let Err(error) = window::ensure_main_window(&app, &engine) {
                tracing::error!(
                    component = "notification",
                    %error,
                    "failed to show persistent reconnect failure notice"
                );
            }
        }
        if let Err(error) = app.emit("engine://snapshot", snapshot) {
            tracing::error!(component = "snapshot-emitter", %error, "failed to emit engine snapshot");
        }
    }));
}

#[cfg(test)]
fn spawn_startup_engine(
    build: BuildInfo,
    diagnostic_notice: Option<DiagnosticNotice>,
    diagnostics_dir: PathBuf,
) -> EngineHandle {
    let sampler = SysinfoResourceSampler::new(std::process::id(), "com.kvmflow.controller");
    let monitor = ValidationMonitor::new(build.clone());
    let store = ValidationStore::new(diagnostics_dir);

    with_tauri_runtime(|| {
        spawn_engine(
            EngineConfig {
                role: EndpointRole::WindowsController,
                build,
                diagnostic_notice,
                sample_interval: SAMPLE_INTERVAL,
                persist_interval: PERSIST_INTERVAL,
            },
            Box::new(sampler),
            monitor,
            Box::new(store),
        )
    })
}

fn spawn_production_engine(
    build: BuildInfo,
    diagnostic_notice: Option<DiagnosticNotice>,
    data_dir: PathBuf,
    diagnostics_dir: PathBuf,
) -> Result<EngineHandle, Box<dyn std::error::Error>> {
    let identity =
        load_or_create_identity(&data_dir, EndpointRole::WindowsController, "Windows 控制端")?;
    let config_store = ConfigStore::new(data_dir);
    let mut config = config_store.load_windows()?;
    if config_store.reconcile_windows_identity(&mut config, &identity.device_id())? {
        tracing::warn!(
            component = "identity",
            "Local file identity changed; peer trust was reset"
        );
    }
    let edge = config.edge;
    let target_display_id = config.target_display_id.clone();
    let sampler = SysinfoResourceSampler::new(std::process::id(), "com.kvmflow.controller");
    let monitor = ValidationMonitor::new(build.clone());
    let report_store = ValidationStore::new(diagnostics_dir);

    with_tauri_runtime(|| {
        let runtime = spawn_windows_runtime(identity, config_store, config);
        let platform = spawn_windows_platform_adapter(edge, target_display_id)?;
        Ok(spawn_engine_with_kvm(
            EngineConfig {
                role: EndpointRole::WindowsController,
                build,
                diagnostic_notice,
                sample_interval: SAMPLE_INTERVAL,
                persist_interval: PERSIST_INTERVAL,
            },
            Box::new(sampler),
            monitor,
            Box::new(report_store),
            runtime,
            platform,
        ))
    })
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let build = BuildInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        build_id: option_env!("KVMFLOW_BUILD_ID").unwrap_or("dev").into(),
    };
    let executable = std::env::current_exe().map_err(|error| {
        eprintln!("failed to resolve executable path: {error}");
        error
    })?;
    let fallback_base = std::env::temp_dir();
    let prepared = prepare_storage(&executable, ExecutableKind::WindowsExe, &fallback_base)
        .map_err(|error| {
            eprintln!("failed to prepare application storage: {error}");
            error
        })?;
    let logging_guard =
        init_file_logging(&prepared.layout, RetentionPolicy::default()).map_err(|error| {
            eprintln!("failed to initialize file logging: {error}");
            error
        })?;
    tracing::info!(
        component = "application",
        platform = "windows",
        version = %build.version,
        build_id = %build.build_id,
        storage_root = %prepared.layout.root.display(),
        storage_fallback = prepared.layout.fallback_used,
        "kvmFlow started"
    );
    let diagnostic_notice = storage_notice(&prepared);
    let engine = spawn_production_engine(
        build,
        diagnostic_notice,
        prepared.layout.data_dir.clone(),
        prepared.layout.diagnostics_dir.clone(),
    )?;

    app.manage(LoggingGuardOwner {
        _guard: Mutex::new(logging_guard),
    });
    app.manage(engine.clone());
    start_snapshot_emitter(app.handle(), &engine);
    tray::create_tray(app.handle(), &engine)?;
    window::ensure_main_window(app.handle(), &engine)?;

    Ok(())
}

fn configure_builder<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        commands::get_snapshot,
        commands::export_foundation_report,
        commands::connect_manual,
        commands::confirm_pairing,
        commands::reject_pairing,
        commands::list_displays,
        commands::configure_target_display,
        commands::configure_screen_edge,
        commands::enter_control,
        commands::return_control,
        commands::set_control_enabled,
        commands::set_game_mode_enabled,
        commands::activate_clipboard_history,
        commands::clear_clipboard_history
    ])
}

fn show_existing_instance(app: &tauri::AppHandle) {
    let Some(engine) = app.try_state::<EngineHandle>() else {
        tracing::warn!(component = "single-instance", "engine state is not ready");
        return;
    };
    match window::ensure_main_window(app, engine.inner()) {
        Ok(window) => {
            if let Err(error) = window.show().and_then(|_| window.set_focus()) {
                tracing::error!(
                    component = "single-instance",
                    %error,
                    "failed to focus the existing Windows instance"
                );
            }
        }
        Err(error) => {
            tracing::error!(
                component = "single-instance",
                %error,
                "failed to open the existing Windows instance"
            );
        }
    }
}

pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_single_instance::init(
        |app, _arguments, _working_directory| show_existing_instance(app),
    ));
    let app = configure_builder(builder.setup(setup))
        .build(tauri::generate_context!())
        .expect("failed to build kvmFlow Windows application");

    app.run(|app, event| match event {
        tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        tauri::RunEvent::Resumed => {
            let engine = app.state::<EngineHandle>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let _ = engine
                    .send(kvmflow_engine::EngineCommand::RecoverConnectionNow)
                    .await;
            });
        }
        _ => {}
    });
}

#[cfg(test)]
pub(crate) mod test_support {
    use kvmflow_diagnostics::{
        PERSIST_INTERVAL, SAMPLE_INTERVAL, SysinfoResourceSampler, ValidationMonitor,
        ValidationStore,
    };
    use kvmflow_domain::{AppSnapshot, BuildInfo, EndpointRole, RuntimeState};
    use kvmflow_engine::{EngineConfig, EngineHandle, spawn_engine};
    use tempfile::TempDir;

    use super::with_tauri_runtime;

    pub(crate) fn test_engine(build_id: &str) -> (TempDir, EngineHandle) {
        let diagnostics = tempfile::tempdir().expect("create diagnostics directory");
        let build = BuildInfo {
            version: "0.1.0".into(),
            build_id: build_id.into(),
        };
        let engine = with_tauri_runtime(|| {
            spawn_engine(
                EngineConfig {
                    role: EndpointRole::WindowsController,
                    build: build.clone(),
                    diagnostic_notice: None,
                    sample_interval: SAMPLE_INTERVAL,
                    persist_interval: PERSIST_INTERVAL,
                },
                Box::new(SysinfoResourceSampler::new(
                    std::process::id(),
                    "com.kvmflow.controller",
                )),
                ValidationMonitor::new(build),
                Box::new(ValidationStore::new(diagnostics.path().to_path_buf())),
            )
        });

        (diagnostics, engine)
    }

    pub(crate) fn snapshot(build_id: &str) -> AppSnapshot {
        AppSnapshot::from(&RuntimeState::new(
            EndpointRole::WindowsController,
            BuildInfo {
                version: "0.1.0".into(),
                build_id: build_id.into(),
            },
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use kvmflow_diagnostics::{PreparedStorage, StorageLayout, StorageWarning};
    use kvmflow_domain::{BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole};
    use serde_json::{Value, json};
    use tokio::sync::{mpsc, watch};

    use super::{
        LoggingGuardOwner, forward_snapshots, spawn_startup_engine, storage_notice,
        test_support::snapshot,
    };

    fn project_file(path: &str) -> String {
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
            .unwrap_or_else(|error| panic!("read {path}: {error}"))
    }

    #[test]
    fn startup_engine_composition_applies_windows_role_build_and_notice() {
        let build = BuildInfo {
            version: "0.1.0".into(),
            build_id: "startup-composition-test".into(),
        };
        let notice = DiagnosticNotice {
            code: DiagnosticNoticeCode::StorageFallback,
            message: "fallback".into(),
            path: Some("fallback-root".into()),
        };
        let diagnostics = tempfile::tempdir().expect("create diagnostics directory");
        let engine = spawn_startup_engine(
            build.clone(),
            Some(notice.clone()),
            diagnostics.path().to_path_buf(),
        );

        let snapshot = engine.current_snapshot();

        assert_eq!(snapshot.role, EndpointRole::WindowsController);
        assert_eq!(snapshot.build, build);
        assert_eq!(snapshot.diagnostic_notice, Some(notice));
        tauri::async_runtime::block_on(engine.shutdown()).expect("shut down engine");
    }

    #[tokio::test]
    async fn snapshot_forwarding_emits_each_change_and_stops_when_sender_closes() {
        let initial = snapshot("initial");
        let first = snapshot("first");
        let second = snapshot("second");
        let (snapshot_tx, snapshot_rx) = watch::channel(initial);
        let (emitted_tx, mut emitted_rx) = mpsc::unbounded_channel();
        let forwarding = tokio::spawn(forward_snapshots(snapshot_rx, move |snapshot| {
            emitted_tx
                .send(snapshot)
                .expect("record forwarded snapshot");
        }));

        tokio::task::yield_now().await;
        assert!(emitted_rx.try_recv().is_err());

        snapshot_tx
            .send(first.clone())
            .expect("send first snapshot");
        assert_eq!(emitted_rx.recv().await, Some(first));

        snapshot_tx
            .send(second.clone())
            .expect("send second snapshot");
        assert_eq!(emitted_rx.recv().await, Some(second));

        drop(snapshot_tx);
        forwarding.await.expect("join snapshot forwarding task");
        assert_eq!(emitted_rx.recv().await, None);
    }

    #[test]
    fn tauri_config_has_dynamic_windows_and_windows_bundle_settings() {
        let config: Value =
            serde_json::from_str(&project_file("tauri.conf.json")).expect("parse Tauri config");

        assert_eq!(config["identifier"], "com.kvmflow.controller");
        assert_eq!(config["build"]["frontendDist"], "./ui/dist");
        assert_eq!(
            config["build"]["beforeBuildCommand"],
            json!({
                "script": "npm --prefix ui run build",
                "cwd": "."
            })
        );
        assert_eq!(config["app"]["windows"], json!([]));
        assert_eq!(
            config["app"]["security"]["capabilities"],
            json!([{
                "identifier": "main-event-listener",
                "windows": ["main"],
                "permissions": [
                    "core:event:allow-listen",
                    "core:event:allow-unlisten"
                ]
            }])
        );
        assert_eq!(config["bundle"]["active"], true);
        assert_eq!(config["bundle"]["targets"], json!(["nsis"]));
        assert_eq!(
            config["bundle"]["icon"],
            json!([
                "../../assets/icons/windows/32x32.png",
                "../../assets/icons/windows/128x128.png",
                "../../assets/icons/windows/128x128@2x.png",
                "../../assets/icons/windows/icon.ico"
            ])
        );
        assert_eq!(
            config["bundle"]["windows"]["nsis"]["installMode"],
            "currentUser"
        );
        assert_eq!(
            config["bundle"]["windows"]["nsis"]["languages"],
            json!(["SimpChinese"])
        );
    }

    #[test]
    fn storage_warning_notice_uses_the_prepared_fallback_root() {
        let prepared_root = Path::new("prepared-fallback").to_path_buf();
        let prepared = PreparedStorage {
            layout: StorageLayout {
                root: prepared_root.clone(),
                data_dir: prepared_root.join("data"),
                logs_dir: prepared_root.join("logs"),
                diagnostics_dir: prepared_root.join("diagnostics"),
                fallback_used: true,
            },
            warning: Some(StorageWarning::PrimaryRootUnavailable {
                requested: Path::new("requested").to_path_buf(),
                fallback: Path::new("warning-field-must-not-drive-the-notice").to_path_buf(),
            }),
        };

        let notice = storage_notice(&prepared).expect("fallback notice");

        assert_eq!(notice.code, DiagnosticNoticeCode::StorageFallback);
        assert_eq!(notice.message, "安装目录不可写，日志已切换到备用目录");
        assert_eq!(notice.path, Some(prepared_root.display().to_string()));
    }

    #[test]
    fn logging_guard_owner_satisfies_tauri_managed_state_bounds() {
        fn assert_managed_state<T: Send + Sync + 'static>() {}

        assert_managed_state::<LoggingGuardOwner>();
    }
}
