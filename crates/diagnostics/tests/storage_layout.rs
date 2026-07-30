use kvmflow_diagnostics::{
    ExecutableKind, StorageWarning, prepare_storage, prepare_storage_root, resolve_install_root,
};
use std::{fs, process};
use tempfile::tempdir;

#[test]
fn windows_root_is_executable_parent() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("kvmFlow");
    fs::create_dir_all(&root).unwrap();
    let executable = root.join("kvmFlow.exe");
    fs::write(&executable, []).unwrap();

    assert_eq!(
        resolve_install_root(&executable, ExecutableKind::WindowsExe).unwrap(),
        root,
    );
}

#[test]
fn mac_root_is_parent_of_app_bundle() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("kvmFlow");
    let executable = root.join("kvmFlow.app/Contents/MacOS/kvmFlow");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, []).unwrap();

    assert_eq!(
        resolve_install_root(&executable, ExecutableKind::MacAppBundle).unwrap(),
        root,
    );
}

#[test]
fn prepare_creates_data_logs_and_diagnostics() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("kvmFlow");
    fs::create_dir_all(&root).unwrap();
    let executable = root.join("kvmFlow.exe");
    let fallback = temp.path().join("fallback");

    let prepared = prepare_storage(&executable, ExecutableKind::WindowsExe, &fallback).unwrap();

    assert_eq!(prepared.layout.root, root);
    assert!(prepared.layout.data_dir.is_dir());
    assert!(prepared.layout.logs_dir.is_dir());
    assert!(prepared.layout.diagnostics_dir.is_dir());
    assert!(!prepared.layout.fallback_used);
    assert_eq!(prepared.warning, None);
}

#[test]
fn explicit_storage_root_supports_standard_application_data_directories() {
    let temp = tempdir().unwrap();
    let root = temp
        .path()
        .join("Library/Application Support/com.kvmflow.endpoint");
    let fallback = temp.path().join("fallback");

    let prepared = prepare_storage_root(&root, &fallback).unwrap();

    assert_eq!(prepared.layout.root, root);
    assert!(prepared.layout.data_dir.is_dir());
    assert!(prepared.layout.logs_dir.is_dir());
    assert!(prepared.layout.diagnostics_dir.is_dir());
    assert!(!prepared.layout.fallback_used);
}

#[test]
fn unwritable_primary_uses_explicit_emergency_root() {
    let temp = tempdir().unwrap();
    let blocked_root = temp.path().join("blocked");
    fs::write(&blocked_root, b"not a directory").unwrap();
    let executable = blocked_root.join("kvmFlow.exe");
    let fallback_base = temp.path().join("fallback");
    let fallback_root = fallback_base.join(format!("kvmflow-emergency-{}", process::id()));

    let prepared =
        prepare_storage(&executable, ExecutableKind::WindowsExe, &fallback_base).unwrap();

    assert_eq!(prepared.layout.root, fallback_root);
    assert!(prepared.layout.fallback_used);
    assert!(prepared.layout.data_dir.is_dir());
    assert!(prepared.layout.logs_dir.is_dir());
    assert!(prepared.layout.diagnostics_dir.is_dir());
    assert_eq!(
        prepared.warning,
        Some(StorageWarning::PrimaryRootUnavailable {
            requested: blocked_root,
            fallback: fallback_root,
        }),
    );
}
