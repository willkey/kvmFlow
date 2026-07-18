#![forbid(unsafe_code)]

use std::{path::PathBuf, sync::Mutex};

use kvmflow_diagnostics::{
    ExecutableKind, LoggingGuard, PERSIST_INTERVAL, PreparedStorage, RetentionPolicy,
    SAMPLE_INTERVAL, SysinfoResourceSampler, ValidationMonitor, ValidationStore, init_file_logging,
    prepare_storage,
};
use kvmflow_domain::{
    AppSnapshot, BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole,
    KeyboardMonitorState,
};
#[cfg(test)]
use kvmflow_engine::spawn_engine;
use kvmflow_engine::{EngineConfig, EngineHandle, spawn_engine_with_kvm};
use kvmflow_platform_macos::spawn_macos_platform_adapter;
use kvmflow_transport::{ConfigStore, load_or_create_identity, spawn_macos_runtime};
use tauri::{Emitter, Manager};

mod commands;
mod menu_bar;
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

async fn forward_keyboard_monitors(
    mut monitors: tokio::sync::watch::Receiver<KeyboardMonitorState>,
    mut publish: impl FnMut(KeyboardMonitorState),
) {
    while monitors.changed().await.is_ok() {
        publish(monitors.borrow_and_update().clone());
    }
}

fn start_snapshot_emitter(app: &tauri::AppHandle, engine: &EngineHandle) {
    let app = app.clone();
    let snapshots = engine.subscribe();
    tauri::async_runtime::spawn(forward_snapshots(snapshots, move |snapshot| {
        if let Err(error) = app.emit("engine://snapshot", snapshot) {
            tracing::error!(component = "snapshot-emitter", %error, "failed to emit engine snapshot");
        }
    }));
}

fn start_keyboard_emitter(app: &tauri::AppHandle, engine: &EngineHandle) {
    let app = app.clone();
    let monitors = engine.subscribe_keyboard();
    tauri::async_runtime::spawn(forward_keyboard_monitors(monitors, move |monitor| {
        if let Err(error) = app.emit("engine://keyboard", monitor) {
            tracing::error!(component = "keyboard-emitter", %error, "failed to emit keyboard monitor");
        }
    }));
}

#[cfg(test)]
fn spawn_startup_engine(
    build: BuildInfo,
    diagnostic_notice: Option<DiagnosticNotice>,
    diagnostics_dir: PathBuf,
) -> EngineHandle {
    let sampler = SysinfoResourceSampler::new(std::process::id(), "com.kvmflow.endpoint");
    let monitor = ValidationMonitor::new(build.clone());
    let store = ValidationStore::new(diagnostics_dir);

    with_tauri_runtime(|| {
        spawn_engine(
            EngineConfig {
                role: EndpointRole::MacEndpoint,
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
    let identity = load_or_create_identity(&data_dir, EndpointRole::MacEndpoint, "Mac 被控端")?;
    let config_store = ConfigStore::new(data_dir);
    let mut config = config_store.load_macos()?;
    if config_store.reconcile_macos_identity(&mut config, &identity.device_id())? {
        tracing::warn!(
            component = "identity",
            "Local file identity changed; peer trust was reset"
        );
    }
    let sampler = SysinfoResourceSampler::new(std::process::id(), "com.kvmflow.endpoint");
    let monitor = ValidationMonitor::new(build.clone());
    let report_store = ValidationStore::new(diagnostics_dir);

    Ok(with_tauri_runtime(|| {
        let runtime = spawn_macos_runtime(identity, config_store, config);
        let platform = spawn_macos_platform_adapter();
        spawn_engine_with_kvm(
            EngineConfig {
                role: EndpointRole::MacEndpoint,
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
        )
    }))
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
    let prepared = prepare_storage(&executable, ExecutableKind::MacAppBundle, &fallback_base)
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
        platform = "macos",
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
    start_keyboard_emitter(app.handle(), &engine);
    menu_bar::create_menu_bar(app.handle(), &engine)?;
    window::ensure_main_window(app.handle(), &engine)?;

    Ok(())
}

fn configure_builder<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        commands::get_snapshot,
        commands::export_foundation_report,
        commands::configure_listen_port,
        commands::confirm_pairing,
        commands::reject_pairing,
        commands::set_local_control_allowed,
        commands::refresh_accessibility_permission,
        commands::open_accessibility_settings,
        commands::return_control
    ])
}

pub fn run() {
    let app = configure_builder(tauri::Builder::default().setup(setup))
        .build(tauri::generate_context!())
        .expect("failed to build kvmFlow macOS application");

    app.run(|_, event| {
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
        }
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

    fn with_test_runtime<T>(operation: impl FnOnce() -> T) -> T {
        let runtime = tauri::async_runtime::handle();
        let _runtime_guard = runtime.inner().enter();
        operation()
    }

    pub(crate) fn test_engine(build_id: &str) -> (TempDir, EngineHandle) {
        let diagnostics = tempfile::tempdir().expect("create diagnostics directory");
        let build = BuildInfo {
            version: "0.1.0".into(),
            build_id: build_id.into(),
        };
        let engine = with_test_runtime(|| {
            spawn_engine(
                EngineConfig {
                    role: EndpointRole::MacEndpoint,
                    build: build.clone(),
                    diagnostic_notice: None,
                    sample_interval: SAMPLE_INTERVAL,
                    persist_interval: PERSIST_INTERVAL,
                },
                Box::new(SysinfoResourceSampler::new(
                    std::process::id(),
                    "com.kvmflow.endpoint",
                )),
                ValidationMonitor::new(build),
                Box::new(ValidationStore::new(diagnostics.path().to_path_buf())),
            )
        });

        (diagnostics, engine)
    }

    pub(crate) fn snapshot(build_id: &str) -> AppSnapshot {
        AppSnapshot::from(&RuntimeState::new(
            EndpointRole::MacEndpoint,
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
    use kvmflow_domain::{
        BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole, KeyAction, KeyUsage,
        KeyboardMonitorState,
    };
    use plist::Value as PlistValue;
    use serde_json::{Value, json};
    use tokio::sync::{mpsc, watch};

    use super::{
        LoggingGuardOwner, forward_keyboard_monitors, forward_snapshots, spawn_startup_engine,
        storage_notice, test_support::snapshot,
    };

    fn project_path(path: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
    }

    fn project_file(path: &str) -> String {
        fs::read_to_string(project_path(path))
            .unwrap_or_else(|error| panic!("read {path}: {error}"))
    }

    #[test]
    fn startup_engine_composition_applies_mac_role_build_and_notice() {
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

        assert_eq!(snapshot.role, EndpointRole::MacEndpoint);
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

    #[tokio::test]
    async fn keyboard_forwarding_emits_each_monitor_change_and_stops_with_sender() {
        let initial = KeyboardMonitorState::default();
        let mut pressed = initial.clone();
        pressed.observe(KeyUsage::LEFT_GUI, KeyAction::Down);
        let (monitor_tx, monitor_rx) = watch::channel(initial);
        let (emitted_tx, mut emitted_rx) = mpsc::unbounded_channel();
        let forwarding = tokio::spawn(forward_keyboard_monitors(monitor_rx, move |monitor| {
            emitted_tx.send(monitor).expect("record keyboard monitor");
        }));

        monitor_tx.send(pressed.clone()).expect("send monitor");
        assert_eq!(emitted_rx.recv().await, Some(pressed));

        drop(monitor_tx);
        forwarding.await.expect("join keyboard forwarding task");
        assert_eq!(emitted_rx.recv().await, None);
    }

    #[test]
    fn tauri_config_has_dynamic_windows_and_macos_bundle_settings() {
        let config: Value =
            serde_json::from_str(&project_file("tauri.conf.json")).expect("parse Tauri config");

        assert_eq!(config["identifier"], "com.kvmflow.endpoint");
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
        assert_eq!(config["bundle"]["targets"], json!(["app", "dmg"]));
        assert_eq!(
            config["bundle"]["icon"],
            json!([
                "../../assets/icons/macos/32x32.png",
                "../../assets/icons/macos/128x128.png",
                "../../assets/icons/macos/128x128@2x.png",
                "../../assets/icons/macos/icon.png",
                "../../assets/icons/macos/icon.icns"
            ])
        );
        assert_eq!(config["bundle"]["macOS"]["minimumSystemVersion"], "12.0");
        assert_eq!(config["bundle"]["macOS"]["hardenedRuntime"], true);
    }

    #[test]
    fn info_plist_is_empty_and_declares_no_permissions() {
        let plist = PlistValue::from_file(project_path("Info.plist")).expect("parse Info.plist");
        let dictionary = plist.as_dictionary().expect("Info.plist dictionary");

        assert!(dictionary.is_empty());
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
