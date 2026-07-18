use std::{
    fs,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use kvmflow_diagnostics::{RetentionPolicy, SizeRollingWriter, TimeSource};
use tempfile::tempdir;

const TEST_POLICY: RetentionPolicy = RetentionPolicy {
    max_file_bytes: 32,
    max_age: Duration::from_secs(7 * 24 * 60 * 60),
    max_total_bytes: 96,
};
const OVERSIZED_MARKER: &[u8] = b"{\"error\":\"oversized\"}\n";

fn total_log_bytes(directory: &std::path::Path) -> u64 {
    fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name == "kvmflow.log" || (name.starts_with("kvmflow.") && name.ends_with(".log"))
        })
        .map(|entry| entry.metadata().unwrap().len())
        .sum()
}

#[derive(Clone)]
struct FixedTime {
    milliseconds: Arc<Mutex<u128>>,
}

impl FixedTime {
    fn new(milliseconds: u128) -> Self {
        Self {
            milliseconds: Arc::new(Mutex::new(milliseconds)),
        }
    }
}

impl TimeSource for FixedTime {
    fn unix_time_millis(&self) -> u128 {
        *self.milliseconds.lock().unwrap()
    }
}

#[test]
fn exceeding_file_limit_rotates_active_log() {
    let temp = tempdir().unwrap();
    let time = FixedTime::new(1_000);
    let mut writer = SizeRollingWriter::with_time_source(temp.path(), TEST_POLICY, time).unwrap();

    writer.write_all(&[b'a'; 32]).unwrap();
    writer.write_all(b"b").unwrap();
    writer.flush().unwrap();

    assert_eq!(
        fs::read(temp.path().join("kvmflow.1000.0.log")).unwrap(),
        [b'a'; 32]
    );
    assert_eq!(fs::read(temp.path().join("kvmflow.log")).unwrap(), b"b");
}

#[test]
fn oversized_write_is_marked_without_rotation_and_writer_remains_usable() {
    let temp = tempdir().unwrap();
    let time = FixedTime::new(1_000);
    let mut writer = SizeRollingWriter::with_time_source(temp.path(), TEST_POLICY, time).unwrap();

    writer.write_all(b"seed").unwrap();
    writer.flush().unwrap();
    let active_path = temp.path().join("kvmflow.log");
    let original_active = fs::read(&active_path).unwrap();

    let oversized_body = [b'x'; 33];
    assert_eq!(writer.write(&oversized_body).unwrap(), oversized_body.len());
    assert!(
        fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| entry.file_name() == "kvmflow.log")
    );
    assert_eq!(
        fs::read(&active_path).unwrap(),
        [original_active, OVERSIZED_MARKER.to_vec()].concat()
    );
    assert!(
        !fs::read(&active_path)
            .unwrap()
            .windows(oversized_body.len())
            .any(|body| body == oversized_body)
    );

    writer.write_all(b"ok").unwrap();
    writer.flush().unwrap();
    assert_eq!(
        fs::read(active_path).unwrap(),
        b"seed{\"error\":\"oversized\"}\nok"
    );
}

#[test]
fn invalid_retention_policies_are_rejected_before_filesystem_access() {
    let temp = tempdir().unwrap();

    for (name, policy) in [
        (
            "zero file bound",
            RetentionPolicy {
                max_file_bytes: 0,
                ..TEST_POLICY
            },
        ),
        (
            "zero total bound",
            RetentionPolicy {
                max_total_bytes: 0,
                ..TEST_POLICY
            },
        ),
        (
            "file bound exceeds total bound",
            RetentionPolicy {
                max_file_bytes: 128,
                max_total_bytes: 96,
                ..TEST_POLICY
            },
        ),
        (
            "file bound cannot contain marker",
            RetentionPolicy {
                max_file_bytes: (OVERSIZED_MARKER.len() - 1) as u64,
                max_total_bytes: OVERSIZED_MARKER.len() as u64,
                ..TEST_POLICY
            },
        ),
    ] {
        let logs_dir = temp.path().join(name);
        let error = match SizeRollingWriter::new(&logs_dir, policy) {
            Ok(_) => panic!("{name} policy unexpectedly succeeded"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{name}");
        assert!(!logs_dir.exists(), "{name}");
    }
}

#[test]
fn cleanup_removes_rotated_files_older_than_max_age() {
    let temp = tempdir().unwrap();
    let time = FixedTime::new(8 * 24 * 60 * 60 * 1_000);
    fs::write(temp.path().join("kvmflow.0.0.log"), b"old").unwrap();
    fs::write(temp.path().join("kvmflow.604800001.1.log"), b"fresh").unwrap();

    let mut writer = SizeRollingWriter::with_time_source(temp.path(), TEST_POLICY, time).unwrap();
    writer.write_all(&[b'a'; 32]).unwrap();
    writer.write_all(b"b").unwrap();

    assert!(!temp.path().join("kvmflow.0.0.log").exists());
    assert!(temp.path().join("kvmflow.604800001.1.log").exists());
}

#[test]
fn cleanup_removes_oldest_rotated_files_until_total_size_is_bounded() {
    let temp = tempdir().unwrap();
    let time = FixedTime::new(10_000);
    fs::write(temp.path().join("kvmflow.1000.0.log"), [b'a'; 32]).unwrap();
    fs::write(temp.path().join("kvmflow.2000.1.log"), [b'b'; 32]).unwrap();
    fs::write(temp.path().join("kvmflow.3000.2.log"), [b'c'; 32]).unwrap();

    let mut writer = SizeRollingWriter::with_time_source(temp.path(), TEST_POLICY, time).unwrap();
    writer.write_all(&[b'd'; 32]).unwrap();
    writer.write_all(b"e").unwrap();

    assert!(!temp.path().join("kvmflow.1000.0.log").exists());
    assert!(!temp.path().join("kvmflow.2000.1.log").exists());
    assert!(temp.path().join("kvmflow.3000.2.log").exists());
    assert!(temp.path().join("kvmflow.10000.0.log").exists());
    assert!(total_log_bytes(temp.path()) <= TEST_POLICY.max_total_bytes);
}

#[test]
fn non_rotating_active_growth_evicts_old_rotated_log_before_total_cap_is_exceeded() {
    let temp = tempdir().unwrap();
    let time = FixedTime::new(10_000);
    let policy = RetentionPolicy {
        max_file_bytes: 128,
        max_total_bytes: 128,
        ..TEST_POLICY
    };
    let rotated_path = temp.path().join("kvmflow.1000.0.log");
    fs::write(&rotated_path, [b'a'; 32]).unwrap();
    fs::write(temp.path().join("kvmflow.log"), [b'b'; 96]).unwrap();

    let mut writer = SizeRollingWriter::with_time_source(temp.path(), policy, time).unwrap();
    writer.write_all(b"c").unwrap();

    assert!(!rotated_path.exists());
    assert_eq!(
        fs::metadata(temp.path().join("kvmflow.log")).unwrap().len(),
        97
    );
    assert!(total_log_bytes(temp.path()) <= policy.max_total_bytes);
}

#[test]
fn cleanup_never_removes_the_active_log() {
    let temp = tempdir().unwrap();
    let time = FixedTime::new(10_000);
    fs::write(temp.path().join("kvmflow.0.0.log"), [b'a'; 32]).unwrap();

    let mut writer = SizeRollingWriter::with_time_source(temp.path(), TEST_POLICY, time).unwrap();
    writer.write_all(&[b'b'; 32]).unwrap();
    writer.write_all(b"c").unwrap();

    assert!(temp.path().join("kvmflow.log").is_file());
}

#[test]
fn rotations_in_the_same_millisecond_have_distinct_sequences() {
    let temp = tempdir().unwrap();
    let time = FixedTime::new(1_000);
    let mut writer = SizeRollingWriter::with_time_source(temp.path(), TEST_POLICY, time).unwrap();

    writer.write_all(&[b'a'; 32]).unwrap();
    writer.write_all(b"b").unwrap();
    writer.write_all(&[b'c'; 31]).unwrap();
    writer.write_all(b"d").unwrap();

    assert!(temp.path().join("kvmflow.1000.0.log").is_file());
    assert!(temp.path().join("kvmflow.1000.1.log").is_file());
}

#[test]
fn writer_produces_valid_structured_json() {
    let temp = tempdir().unwrap();
    let writer_policy = RetentionPolicy {
        max_file_bytes: 1_024,
        max_total_bytes: 2_048,
        ..TEST_POLICY
    };
    let writer = SizeRollingWriter::new(temp.path(), writer_policy).unwrap();
    let (non_blocking, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(1_024)
        .lossy(false)
        .finish(writer);
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_writer(non_blocking)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(component = "engine", build_id = "test", "started");
    });
    drop(guard);

    let event: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.path().join("kvmflow.log")).unwrap()).unwrap();
    assert_eq!(event["fields"]["component"], "engine");
    assert_eq!(event["fields"]["build_id"], "test");
    assert_eq!(event["fields"]["message"], "started");
}

#[test]
fn non_blocking_tracing_marks_oversized_events_and_keeps_worker_alive() {
    let temp = tempdir().unwrap();
    let policy = RetentionPolicy {
        max_file_bytes: 256,
        max_total_bytes: 512,
        ..TEST_POLICY
    };
    let oversized_body = "sensitive-oversized-body-".repeat(32);
    let writer = SizeRollingWriter::new(temp.path(), policy).unwrap();
    let (non_blocking, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(1_024)
        .lossy(false)
        .finish(writer);
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_writer(non_blocking)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(component = "engine", body = %oversized_body, "oversized event");
        tracing::info!(
            component = "engine",
            build_id = "test",
            "after oversized event"
        );
    });
    drop(guard);

    let contents = fs::read(temp.path().join("kvmflow.log")).unwrap();
    assert!(
        !contents
            .windows(oversized_body.len())
            .any(|body| body == oversized_body.as_bytes())
    );

    let events: Vec<serde_json::Value> = contents
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["error"], "oversized");
    assert_eq!(events[1]["fields"]["component"], "engine");
    assert_eq!(events[1]["fields"]["build_id"], "test");
    assert_eq!(events[1]["fields"]["message"], "after oversized event");
    assert!(total_log_bytes(temp.path()) <= policy.max_total_bytes);
}
