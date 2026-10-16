#![forbid(unsafe_code)]

use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use kvmflow_diagnostics::{
    ExecutableKind, LoggingGuard, PERSIST_INTERVAL, PreparedStorage, RetentionPolicy,
    SAMPLE_INTERVAL, SysinfoResourceSampler, ValidationMonitor, ValidationStore, init_file_logging,
    prepare_storage_root, resolve_install_root,
};
use kvmflow_domain::{
    AppSnapshot, BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole,
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

const AUTOSTART_ARGUMENT: &str = "--autostart";

fn launched_from_autostart<I, S>(arguments: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    arguments
        .into_iter()
        .any(|argument| argument.as_ref() == OsStr::new(AUTOSTART_ARGUMENT))
}

fn storage_notice(prepared: &PreparedStorage) -> Option<DiagnosticNotice> {
    prepared.warning.as_ref().map(|_| DiagnosticNotice {
        code: DiagnosticNoticeCode::StorageFallback,
        message: "应用数据目录不可写，日志已切换到备用目录".into(),
        path: Some(prepared.layout.root.display().to_string()),
    })
}

fn migrate_legacy_runtime(legacy_root: &Path, standard_root: &Path) -> io::Result<bool> {
    if legacy_root == standard_root
        || !legacy_root.join("data/device-identity.json").is_file()
        || standard_root.join("data/device-identity.json").is_file()
    {
        return Ok(false);
    }

    let legacy_directories = ["data", "logs", "diagnostics"]
        .into_iter()
        .map(|directory| legacy_root.join(directory))
        .filter(|source| source.is_dir())
        .collect::<Vec<_>>();
    for source in &legacy_directories {
        validate_migration_tree(source)?;
    }

    if standard_root.exists() {
        if fs::read_dir(standard_root)?.next().transpose()?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "standard application data directory is not empty",
            ));
        }
        fs::remove_dir(standard_root)?;
    }
    let parent = standard_root.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "standard application data directory has no parent",
        )
    })?;
    fs::create_dir_all(parent)?;
    let directory_name = standard_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("kvmflow");
    let staging_root = parent.join(format!(
        ".{directory_name}-migration-{}",
        std::process::id()
    ));
    if staging_root.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "application data migration staging directory already exists",
        ));
    }
    fs::create_dir(&staging_root)?;
    let migration_result = (|| {
        for source in &legacy_directories {
            let destination = staging_root.join(
                source
                    .file_name()
                    .expect("legacy runtime directory has a name"),
            );
            copy_migration_tree(source, &destination)?;
        }
        fs::rename(&staging_root, standard_root)
    })();
    if let Err(error) = migration_result {
        let _ = fs::remove_dir_all(&staging_root);
        return Err(error);
    }

    for source in legacy_directories {
        let _ = fs::remove_dir_all(source);
    }
    Ok(true)
}

fn validate_migration_tree(source: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy runtime data contains a symbolic link",
            ));
        }
        if file_type.is_dir() {
            validate_migration_tree(&entry.path())?;
        } else if !file_type.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy runtime data contains an unsupported file type",
            ));
        }
    }
    Ok(())
}

fn copy_migration_tree(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy runtime data contains a symbolic link",
            ));
        }
        if file_type.is_dir() {
            copy_migration_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(source_path, destination_path)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy runtime data contains an unsupported file type",
            ));
        }
    }
    Ok(())
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
    let snapshots = engine.subscribe();
    tauri::async_runtime::spawn(forward_snapshots(snapshots, move |snapshot| {
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
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let autostart_launch = launched_from_autostart(std::env::args_os());
    let build = BuildInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        build_id: option_env!("KVMFLOW_BUILD_ID").unwrap_or("dev").into(),
    };
    let executable = std::env::current_exe().map_err(|error| {
        eprintln!("failed to resolve executable path: {error}");
        error
    })?;
    let storage_root = app.path().app_data_dir().map_err(|error| {
        eprintln!("failed to resolve standard application data path: {error}");
        error
    })?;
    let legacy_root = resolve_install_root(&executable, ExecutableKind::MacAppBundle)?;
    let migrated = migrate_legacy_runtime(&legacy_root, &storage_root).map_err(|error| {
        eprintln!("failed to migrate legacy application data: {error}");
        error
    })?;
    let fallback_base = std::env::temp_dir();
    let prepared = prepare_storage_root(&storage_root, &fallback_base).map_err(|error| {
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
        legacy_storage_migrated = migrated,
        autostart_launch,
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
    menu_bar::create_menu_bar(app.handle(), &engine)?;
    if !autostart_launch {
        window::ensure_main_window(app.handle(), &engine)?;
    }

    Ok(())
}

fn configure_builder<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        commands::get_snapshot,
        commands::get_autostart_enabled,
        commands::set_autostart_enabled,
        commands::export_foundation_report,
        commands::configure_listen_port,
        commands::confirm_pairing,
        commands::reject_pairing,
        commands::set_local_control_allowed,
        commands::refresh_accessibility_permission,
        commands::open_accessibility_settings,
        commands::return_control,
        commands::reset_input
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
                    "failed to focus the existing Mac instance"
                );
            }
        }
        Err(error) => {
            tracing::error!(
                component = "single-instance",
                %error,
                "failed to open the existing Mac instance"
            );
        }
    }
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _working_directory| show_existing_instance(app),
        ))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARGUMENT]),
        ));
    let app = configure_builder(builder.setup(setup))
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
    use kvmflow_domain::{BuildInfo, DiagnosticNotice, DiagnosticNoticeCode, EndpointRole};
    use plist::Value as PlistValue;
    use serde_json::{Value, json};
    use tokio::sync::{mpsc, watch};

    use super::{
        LoggingGuardOwner, forward_snapshots, launched_from_autostart, migrate_legacy_runtime,
        spawn_startup_engine, storage_notice, test_support::snapshot,
    };

    #[test]
    fn autostart_argument_is_detected_without_hiding_manual_launches() {
        assert!(launched_from_autostart(["kvmFlow", "--autostart"]));
        assert!(!launched_from_autostart(["kvmFlow"]));
    }

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
    fn info_plist_declares_a_menu_bar_application() {
        let plist = PlistValue::from_file(project_path("Info.plist")).expect("parse Info.plist");
        let dictionary = plist.as_dictionary().expect("Info.plist dictionary");

        assert_eq!(
            dictionary
                .get("LSUIElement")
                .and_then(PlistValue::as_boolean),
            Some(true)
        );
        assert!(!dictionary.contains_key("LSBackgroundOnly"));
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
        assert_eq!(notice.message, "应用数据目录不可写，日志已切换到备用目录");
        assert_eq!(notice.path, Some(prepared_root.display().to_string()));
    }

    #[test]
    fn legacy_runtime_migrates_once_to_standard_application_data() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let legacy = temp.path().join("legacy");
        let standard = temp.path().join("standard");
        fs::create_dir_all(legacy.join("data")).expect("create legacy data");
        fs::create_dir_all(legacy.join("logs")).expect("create legacy logs");
        fs::write(legacy.join("data/device-identity.json"), "identity").expect("write identity");
        fs::write(legacy.join("data/config.json"), "config").expect("write config");
        fs::write(legacy.join("logs/kvmflow.log"), "log").expect("write log");

        assert!(migrate_legacy_runtime(&legacy, &standard).expect("migrate"));
        assert_eq!(
            fs::read_to_string(standard.join("data/device-identity.json")).expect("read identity"),
            "identity"
        );
        assert_eq!(
            fs::read_to_string(standard.join("data/config.json")).expect("read config"),
            "config"
        );
        assert_eq!(
            fs::read_to_string(standard.join("logs/kvmflow.log")).expect("read log"),
            "log"
        );
        assert!(!legacy.join("data").exists());
        assert!(!legacy.join("logs").exists());
        assert!(!migrate_legacy_runtime(&legacy, &standard).expect("second migration"));
    }

    #[cfg(unix)]
    #[test]
    fn legacy_runtime_rejects_symbolic_links_before_moving_data() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary directory");
        let legacy = temp.path().join("legacy");
        let standard = temp.path().join("standard");
        fs::create_dir_all(legacy.join("data")).expect("create legacy data");
        fs::write(legacy.join("data/device-identity.json"), "identity").expect("write identity");
        symlink(
            temp.path().join("outside"),
            legacy.join("data/linked-outside"),
        )
        .expect("create symbolic link");

        let error = migrate_legacy_runtime(&legacy, &standard).expect_err("reject symbolic link");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(legacy.join("data/device-identity.json").is_file());
        assert!(!standard.exists());
    }

    #[test]
    fn legacy_runtime_refuses_a_nonempty_destination_without_deleting_files() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let legacy = temp.path().join("legacy");
        let standard = temp.path().join("standard");
        fs::create_dir_all(legacy.join("data")).expect("create legacy data");
        fs::create_dir_all(standard.join("data")).expect("create standard data");
        fs::write(legacy.join("data/device-identity.json"), "legacy identity")
            .expect("write identity");
        fs::write(legacy.join("data/config.json"), "legacy config").expect("write legacy config");
        fs::write(standard.join("data/config.json"), "standard config")
            .expect("write standard config");

        let error = migrate_legacy_runtime(&legacy, &standard).expect_err("reject destination");

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(legacy.join("data/config.json")).expect("read legacy config"),
            "legacy config"
        );
        assert_eq!(
            fs::read_to_string(standard.join("data/config.json")).expect("read standard config"),
            "standard config"
        );
    }

    #[test]
    fn logging_guard_owner_satisfies_tauri_managed_state_bounds() {
        fn assert_managed_state<T: Send + Sync + 'static>() {}

        assert_managed_state::<LoggingGuardOwner>();
    }
}
