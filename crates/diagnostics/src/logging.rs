use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use thiserror::Error;
use tracing_appender::non_blocking::WorkerGuard;

use crate::StorageLayout;

const ACTIVE_LOG_NAME: &str = "kvmflow.log";
const ROTATED_LOG_PREFIX: &str = "kvmflow.";
const ROTATED_LOG_SUFFIX: &str = ".log";
const OVERSIZED_MARKER: &[u8] = b"{\"error\":\"oversized\"}\n";

/// Bounds the on-disk log files retained by a [`SizeRollingWriter`].
#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    pub max_file_bytes: u64,
    pub max_age: Duration,
    pub max_total_bytes: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_file_bytes: 10 * 1024 * 1024,
            max_age: Duration::from_secs(7 * 24 * 60 * 60),
            max_total_bytes: 100 * 1024 * 1024,
        }
    }
}

/// Supplies wall-clock timestamps for rotated log names and retention cleanup.
///
/// Production callers normally use [`SizeRollingWriter::new`].
/// [`SizeRollingWriter::with_time_source`] accepts this trait so tests can make
/// rotation and age-based retention deterministic.
pub trait TimeSource: Send + Sync {
    fn unix_time_millis(&self) -> u128;
}

#[derive(Debug)]
struct SystemTimeSource;

impl TimeSource for SystemTimeSource {
    fn unix_time_millis(&self) -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }
}

/// A synchronous writer that rotates the active kvmFlow log before it exceeds
/// the configured size limit.
///
/// This is public to support deterministic integration tests and local
/// subscribers. Applications should use [`init_file_logging`] for the normal
/// non-blocking logging setup.
pub struct SizeRollingWriter {
    logs_dir: PathBuf,
    active_path: PathBuf,
    active_file: File,
    active_bytes: u64,
    rotated_bytes: u64,
    policy: RetentionPolicy,
    time_source: Box<dyn TimeSource>,
    sequence: u64,
}

impl SizeRollingWriter {
    /// Creates a writer that timestamps rotations using the system clock.
    pub fn new(logs_dir: impl AsRef<Path>, policy: RetentionPolicy) -> io::Result<Self> {
        Self::with_time_source(logs_dir, policy, SystemTimeSource)
    }

    /// Creates a writer with an injectable clock for deterministic testing.
    pub fn with_time_source(
        logs_dir: impl AsRef<Path>,
        policy: RetentionPolicy,
        time_source: impl TimeSource + 'static,
    ) -> io::Result<Self> {
        validate_retention_policy(policy)?;

        let logs_dir = logs_dir.as_ref().to_path_buf();
        fs::create_dir_all(&logs_dir)?;
        let active_path = logs_dir.join(ACTIVE_LOG_NAME);
        let active_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)?;
        let active_bytes = active_file.metadata()?.len();
        let rotated_bytes = rotated_log_bytes(&logs_dir)?;

        Ok(Self {
            logs_dir,
            active_path,
            active_file,
            active_bytes,
            rotated_bytes,
            policy,
            time_source: Box::new(time_source),
            sequence: 0,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.active_file.flush()?;

        let rotated_bytes = self.active_bytes;
        let timestamp = self.time_source.unix_time_millis();
        let rotated_path = self
            .logs_dir
            .join(format!("kvmflow.{timestamp}.{}.log", self.sequence));
        self.sequence = self.sequence.saturating_add(1);
        fs::rename(&self.active_path, rotated_path)?;

        self.active_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.active_path)?;
        self.active_bytes = 0;
        self.rotated_bytes = self.rotated_bytes.saturating_add(rotated_bytes);
        Ok(())
    }

    fn cleanup(&mut self) -> io::Result<()> {
        let now = self.time_source.unix_time_millis();
        let max_age_ms = self.policy.max_age.as_millis();
        let mut rotated_logs = Vec::new();

        for entry in fs::read_dir(&self.logs_dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some((timestamp, sequence)) = rotated_log_parts(&path) else {
                continue;
            };

            if now.saturating_sub(timestamp) > max_age_ms {
                fs::remove_file(path)?;
                continue;
            }

            rotated_logs.push(RotatedLog {
                path,
                timestamp,
                sequence,
                bytes: entry.metadata()?.len(),
            });
        }

        let mut retained_rotated_bytes = rotated_logs
            .iter()
            .map(|log| log.bytes)
            .fold(0, u64::saturating_add);
        let mut total_bytes = self.active_bytes.saturating_add(retained_rotated_bytes);

        rotated_logs.sort_by_key(|log| (log.timestamp, log.sequence));
        for log in rotated_logs {
            if total_bytes <= self.policy.max_total_bytes {
                break;
            }
            fs::remove_file(log.path)?;
            total_bytes = total_bytes.saturating_sub(log.bytes);
            retained_rotated_bytes = retained_rotated_bytes.saturating_sub(log.bytes);
        }
        self.rotated_bytes = retained_rotated_bytes;

        Ok(())
    }

    fn write_record(&mut self, buffer: &[u8], require_complete_write: bool) -> io::Result<usize> {
        let attempted_bytes = buffer.len() as u64;
        let rotated =
            if self.active_bytes.saturating_add(attempted_bytes) > self.policy.max_file_bytes {
                self.rotate()?;
                true
            } else {
                false
            };

        let written = if require_complete_write {
            self.active_file.write_all(buffer)?;
            buffer.len()
        } else {
            self.active_file.write(buffer)?
        };
        self.active_bytes = self.active_bytes.saturating_add(written as u64);
        if rotated
            || self.active_bytes.saturating_add(self.rotated_bytes) > self.policy.max_total_bytes
        {
            self.cleanup()?;
        }
        Ok(written)
    }
}

impl Write for SizeRollingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if (buffer.len() as u64) > self.policy.max_file_bytes {
            self.write_record(OVERSIZED_MARKER, true)?;
            return Ok(buffer.len());
        }

        self.write_record(buffer, false)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.active_file.flush()
    }
}

fn validate_retention_policy(policy: RetentionPolicy) -> io::Result<()> {
    if policy.max_file_bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "max_file_bytes must be greater than zero",
        ));
    }
    if policy.max_total_bytes == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "max_total_bytes must be greater than zero",
        ));
    }
    if policy.max_file_bytes > policy.max_total_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "max_file_bytes must not exceed max_total_bytes",
        ));
    }
    if policy.max_file_bytes < OVERSIZED_MARKER.len() as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "max_file_bytes must contain the oversized event marker",
        ));
    }

    Ok(())
}

struct RotatedLog {
    path: PathBuf,
    timestamp: u128,
    sequence: u64,
    bytes: u64,
}

fn rotated_log_bytes(directory: &Path) -> io::Result<u64> {
    fs::read_dir(directory)?.try_fold(0u64, |total, entry| {
        let entry = entry?;
        if rotated_log_parts(&entry.path()).is_some() {
            Ok(total.saturating_add(entry.metadata()?.len()))
        } else {
            Ok(total)
        }
    })
}

fn rotated_log_parts(path: &Path) -> Option<(u128, u64)> {
    let name = path.file_name()?.to_str()?;
    let body = name
        .strip_prefix(ROTATED_LOG_PREFIX)?
        .strip_suffix(ROTATED_LOG_SUFFIX)?;
    let (timestamp, sequence) = body.split_once('.')?;
    Some((timestamp.parse().ok()?, sequence.parse().ok()?))
}

/// Keeps the non-blocking logging worker alive until application shutdown.
pub struct LoggingGuard {
    _worker: WorkerGuard,
}

/// Errors returned while preparing or installing file logging.
#[derive(Debug, Error)]
pub enum LoggingError {
    #[error("unable to prepare file logging: {0}")]
    Io(#[from] io::Error),
    #[error("unable to install file logging subscriber: {0}")]
    Subscriber(String),
}

/// Installs the process file logger at `INFO` level and returns its worker guard.
///
/// Calling this after another global tracing subscriber is installed returns an
/// error instead of silently retaining the existing subscriber.
pub fn init_file_logging(
    layout: &StorageLayout,
    policy: RetentionPolicy,
) -> Result<LoggingGuard, LoggingError> {
    let writer = SizeRollingWriter::new(&layout.logs_dir, policy)?;
    let (non_blocking, worker) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(1_024)
        .lossy(false)
        .finish(writer);

    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_writer(non_blocking)
        .try_init()
        .map_err(|error| LoggingError::Subscriber(error.to_string()))?;

    Ok(LoggingGuard { _worker: worker })
}
