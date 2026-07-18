use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use kvmflow_domain::EndpointRole;
use serde::{Deserialize, Serialize};
use tempfile::Builder;

use crate::DeviceIdentity;

const IDENTITY_SCHEMA_VERSION: u16 = 1;
const MAX_IDENTITY_FILE_BYTES: u64 = 4 * 1024;
pub const IDENTITY_FILE_NAME: &str = "device-identity.json";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredIdentity {
    schema_version: u16,
    role: EndpointRole,
    secret_key: [u8; 32],
}

pub fn load_or_create_identity(
    data_dir: impl AsRef<Path>,
    role: EndpointRole,
    device_name: impl Into<String>,
) -> Result<DeviceIdentity, IdentityStoreError> {
    let data_dir = data_dir.as_ref();
    let path = data_dir.join(IDENTITY_FILE_NAME);
    let device_name = device_name.into();

    match load_identity(&path, role, &device_name) {
        Ok(identity) => Ok(identity),
        Err(IdentityStoreError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            create_identity(data_dir, &path, role, device_name)
        }
        Err(error) => Err(error),
    }
}

fn load_identity(
    path: &Path,
    role: EndpointRole,
    device_name: &str,
) -> Result<DeviceIdentity, IdentityStoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(IdentityStoreError::Symlink(path.to_path_buf()));
    }
    if metadata.len() > MAX_IDENTITY_FILE_BYTES {
        return Err(IdentityStoreError::TooLarge(metadata.len()));
    }
    restrict_file_permissions(path)?;

    let contents = fs::read(path)?;
    let stored: StoredIdentity = serde_json::from_slice(&contents)?;
    if stored.schema_version != IDENTITY_SCHEMA_VERSION {
        return Err(IdentityStoreError::UnsupportedSchema(stored.schema_version));
    }
    if stored.role != role {
        return Err(IdentityStoreError::RoleMismatch {
            expected: role,
            actual: stored.role,
        });
    }
    DeviceIdentity::from_secret_bytes(role, device_name, &stored.secret_key).map_err(Into::into)
}

fn create_identity(
    data_dir: &Path,
    path: &Path,
    role: EndpointRole,
    device_name: String,
) -> Result<DeviceIdentity, IdentityStoreError> {
    fs::create_dir_all(data_dir)?;
    let identity = DeviceIdentity::generate(role, device_name.clone());
    let stored = StoredIdentity {
        schema_version: IDENTITY_SCHEMA_VERSION,
        role,
        secret_key: identity.secret_bytes(),
    };
    let mut temporary = Builder::new()
        .prefix("device-identity-")
        .suffix(".json.tmp")
        .tempfile_in(data_dir)?;
    restrict_file_permissions(temporary.path())?;
    serde_json::to_writer_pretty(&mut temporary, &stored)?;
    temporary.write_all(b"\n")?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;

    match temporary.persist_noclobber(path) {
        Ok(_) => {
            sync_parent_directory(data_dir)?;
            Ok(identity)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            load_identity(path, role, &device_name)
        }
        Err(error) => Err(IdentityStoreError::Io(error.error)),
    }
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityStoreError {
    #[error("device identity I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("device identity JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("stored identity is invalid: {0}")]
    Identity(#[from] crate::IdentityError),
    #[error("device identity file is a symbolic link: {0}")]
    Symlink(PathBuf),
    #[error("device identity file is too large: {0} bytes")]
    TooLarge(u64),
    #[error("unsupported device identity schema version: {0}")]
    UnsupportedSchema(u16),
    #[error("device identity belongs to {actual:?}, expected {expected:?}")]
    RoleMismatch {
        expected: EndpointRole,
        actual: EndpointRole,
    },
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn first_load_generates_a_file_and_second_load_preserves_identity() {
        let directory = tempfile::tempdir().expect("create temporary data directory");

        let first =
            load_or_create_identity(directory.path(), EndpointRole::MacEndpoint, "Mac endpoint")
                .expect("generate identity");
        let first_id = first.device_id();

        let second = load_or_create_identity(
            directory.path(),
            EndpointRole::MacEndpoint,
            "Renamed Mac endpoint",
        )
        .expect("reload identity");
        assert_eq!(second.device_id(), first_id);
        assert!(directory.path().join(IDENTITY_FILE_NAME).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn identity_file_permissions_are_restricted_to_the_current_user() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("create temporary data directory");
        load_or_create_identity(directory.path(), EndpointRole::MacEndpoint, "Mac endpoint")
            .expect("generate identity");

        let mode = fs::metadata(directory.path().join(IDENTITY_FILE_NAME))
            .expect("read identity metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn malformed_identity_is_rejected_without_replacement() {
        let directory = tempfile::tempdir().expect("create temporary data directory");
        let path = directory.path().join(IDENTITY_FILE_NAME);
        fs::write(&path, b"not-json").expect("write malformed identity");

        let error =
            load_or_create_identity(directory.path(), EndpointRole::MacEndpoint, "Mac endpoint")
                .expect_err("reject malformed identity");

        assert!(matches!(error, IdentityStoreError::Json(_)));
        assert_eq!(
            fs::read(path).expect("read malformed identity"),
            b"not-json"
        );
    }

    #[test]
    fn identity_cannot_be_reused_for_the_other_role() {
        let directory = tempfile::tempdir().expect("create temporary data directory");
        load_or_create_identity(directory.path(), EndpointRole::MacEndpoint, "Mac endpoint")
            .expect("generate Mac identity");

        let error = load_or_create_identity(
            directory.path(),
            EndpointRole::WindowsController,
            "Windows controller",
        )
        .expect_err("reject role mismatch");

        assert!(matches!(
            error,
            IdentityStoreError::RoleMismatch {
                expected: EndpointRole::WindowsController,
                actual: EndpointRole::MacEndpoint,
            }
        ));
    }

    #[test]
    fn concurrent_first_loads_converge_on_one_identity() {
        let directory = tempfile::tempdir().expect("create temporary data directory");
        let data_dir = directory.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(8));
        let threads = (0..8)
            .map(|_| {
                let data_dir = data_dir.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    load_or_create_identity(data_dir, EndpointRole::MacEndpoint, "Mac endpoint")
                        .expect("load identity concurrently")
                        .device_id()
                })
            })
            .collect::<Vec<_>>();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().expect("join identity loader"))
            .collect::<Vec<_>>();

        assert!(results.iter().all(|result| result == &results[0]));
        let files = fs::read_dir(directory.path())
            .expect("read identity directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect identity files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name(), IDENTITY_FILE_NAME);
    }
}
