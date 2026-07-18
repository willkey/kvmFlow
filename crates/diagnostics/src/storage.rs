use std::{
    fs, io,
    path::{Path, PathBuf},
    process,
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableKind {
    WindowsExe,
    MacAppBundle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayout {
    pub root: PathBuf,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub diagnostics_dir: PathBuf,
    pub fallback_used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageWarning {
    PrimaryRootUnavailable {
        requested: PathBuf,
        fallback: PathBuf,
    },
}

#[derive(Debug)]
pub struct PreparedStorage {
    pub layout: StorageLayout,
    pub warning: Option<StorageWarning>,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("executable path has no parent: {executable}")]
    ExecutableParentMissing { executable: PathBuf },
    #[error("macOS app bundle ancestor was not found: {executable}")]
    AppBundleNotFound { executable: PathBuf },
    #[error(
        "unable to prepare emergency storage at {fallback} after primary root {requested} failed: primary: {primary_error}; fallback: {fallback_error}"
    )]
    FallbackRootUnavailable {
        requested: PathBuf,
        fallback: PathBuf,
        primary_error: io::Error,
        fallback_error: io::Error,
    },
}

pub fn resolve_install_root(
    executable: &Path,
    kind: ExecutableKind,
) -> Result<PathBuf, StorageError> {
    match kind {
        ExecutableKind::WindowsExe => executable.parent().map(Path::to_path_buf).ok_or_else(|| {
            StorageError::ExecutableParentMissing {
                executable: executable.to_path_buf(),
            }
        }),
        ExecutableKind::MacAppBundle => executable
            .ancestors()
            .find(|ancestor| {
                ancestor
                    .extension()
                    .is_some_and(|extension| extension == "app")
            })
            .ok_or_else(|| StorageError::AppBundleNotFound {
                executable: executable.to_path_buf(),
            })?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| StorageError::ExecutableParentMissing {
                executable: executable.to_path_buf(),
            }),
    }
}

pub fn prepare_storage(
    executable: &Path,
    kind: ExecutableKind,
    fallback_base: &Path,
) -> Result<PreparedStorage, StorageError> {
    let requested = resolve_install_root(executable, kind)?;

    match create_layout(requested.clone(), false) {
        Ok(layout) => Ok(PreparedStorage {
            layout,
            warning: None,
        }),
        Err(primary_error) => {
            let fallback = fallback_base.join(format!("kvmflow-emergency-{}", process::id()));
            match create_layout(fallback.clone(), true) {
                Ok(layout) => Ok(PreparedStorage {
                    layout,
                    warning: Some(StorageWarning::PrimaryRootUnavailable {
                        requested,
                        fallback,
                    }),
                }),
                Err(fallback_error) => Err(StorageError::FallbackRootUnavailable {
                    requested,
                    fallback,
                    primary_error,
                    fallback_error,
                }),
            }
        }
    }
}

fn create_layout(root: PathBuf, fallback_used: bool) -> io::Result<StorageLayout> {
    let data_dir = root.join("data");
    let logs_dir = root.join("logs");
    let diagnostics_dir = root.join("diagnostics");

    fs::create_dir_all(&data_dir)?;
    fs::create_dir_all(&logs_dir)?;
    fs::create_dir_all(&diagnostics_dir)?;

    Ok(StorageLayout {
        root,
        data_dir,
        logs_dir,
        diagnostics_dir,
        fallback_used,
    })
}
