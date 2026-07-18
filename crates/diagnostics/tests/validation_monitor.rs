use std::{fs, sync::Arc};

use kvmflow_diagnostics::{
    BACKGROUND_CPU_BLOCKER, BACKGROUND_CPU_TARGET, BACKGROUND_MEMORY_BLOCKER,
    BACKGROUND_MEMORY_TARGET, CPU_WINDOW_SAMPLES, RECENT_SAMPLE_CAPACITY, SAMPLE_INTERVAL,
    VISIBLE_MEMORY_TARGET, ValidationMonitor, ValidationReport, ValidationReportStore,
    ValidationStore,
};
use kvmflow_domain::{BuildInfo, GateStatus, ResourceSample, WindowState};
use tempfile::tempdir;

const MIB: u64 = 1024 * 1024;

fn build(build_id: &str) -> BuildInfo {
    BuildInfo {
        version: "1.2.3".into(),
        build_id: build_id.into(),
    }
}

fn sample(
    index: u64,
    window_state: WindowState,
    resident_memory_bytes: u64,
    cpu_percent: f32,
    cpu_valid: bool,
) -> ResourceSample {
    let interval_ms = SAMPLE_INTERVAL.as_millis() as u64;
    ResourceSample {
        observed_at_unix_ms: index * interval_ms,
        uptime_seconds: index * SAMPLE_INTERVAL.as_secs(),
        window_state,
        process_count: 3,
        resident_memory_bytes,
        cpu_percent,
        cpu_valid,
    }
}

fn record_background_cpu(monitor: &mut ValidationMonitor, cpu_percent: f32) {
    for index in 0..CPU_WINDOW_SAMPLES as u64 {
        monitor.record(sample(
            index,
            WindowState::Closed,
            40 * MIB,
            cpu_percent,
            true,
        ));
    }
}

fn report_for(build_id: &str) -> ValidationReport {
    report_for_info("1.2.3", build_id)
}

fn report_for_info(version: &str, build_id: &str) -> ValidationReport {
    let mut monitor = ValidationMonitor::new(BuildInfo {
        version: version.into(),
        build_id: build_id.into(),
    });
    monitor.record(sample(0, WindowState::Closed, 40 * MIB, 0.4, true));
    monitor.report(1_700_000_000_000)
}

#[test]
fn no_sample_leaves_all_metrics_not_observed() {
    let monitor = ValidationMonitor::new(build("no-sample"));

    let progress = monitor.progress();
    for metric in [
        progress.background_memory,
        progress.visible_memory,
        progress.background_cpu,
    ] {
        assert_eq!(metric.status, GateStatus::NotObserved);
        assert_eq!(metric.current, None);
    }
    assert_eq!(progress.continuous_uptime_seconds, 0);
}

#[test]
fn fewer_than_sixty_valid_background_cpu_samples_are_collecting() {
    let mut monitor = ValidationMonitor::new(build("collecting"));

    for index in 0..(CPU_WINDOW_SAMPLES - 1) as u64 {
        monitor.record(sample(index, WindowState::Closed, 40 * MIB, 0.4, true));
    }
    monitor.record(sample(
        CPU_WINDOW_SAMPLES as u64,
        WindowState::Closed,
        40 * MIB,
        99.0,
        false,
    ));

    assert_eq!(
        monitor.progress().background_cpu.status,
        GateStatus::Collecting
    );
    assert_eq!(
        monitor
            .report(1_700_000_000_000)
            .valid_background_cpu_sample_count,
        (CPU_WINDOW_SAMPLES - 1) as u64
    );
}

#[test]
fn exactly_sixty_valid_background_cpu_samples_average_point_four_and_pass() {
    let mut monitor = ValidationMonitor::new(build("cpu-pass"));
    record_background_cpu(&mut monitor, 0.4);

    let progress = monitor.progress();
    assert_eq!(progress.background_cpu.status, GateStatus::Passed);
    assert!((progress.background_cpu.current.unwrap() - 0.4).abs() < 0.000_001);
    assert_eq!(progress.background_cpu.target, BACKGROUND_CPU_TARGET as f64);
    assert_eq!(
        monitor
            .report(1_700_000_000_000)
            .valid_background_cpu_sample_count,
        CPU_WINDOW_SAMPLES as u64
    );
}

#[test]
fn cpu_target_and_blocker_boundaries_are_distinct() {
    let mut at_target = ValidationMonitor::new(build("cpu-target"));
    record_background_cpu(&mut at_target, BACKGROUND_CPU_TARGET);
    assert_eq!(
        at_target.progress().background_cpu.status,
        GateStatus::Passed
    );
    assert!(!at_target.report(1_700_000_000_000).foundation_blocked);

    let mut at_blocker = ValidationMonitor::new(build("cpu-blocker"));
    record_background_cpu(&mut at_blocker, BACKGROUND_CPU_BLOCKER);
    assert_eq!(
        at_blocker.progress().background_cpu.status,
        GateStatus::Failed
    );
    assert!(!at_blocker.report(1_700_000_000_000).foundation_blocked);

    let mut above_blocker = ValidationMonitor::new(build("cpu-above-blocker"));
    record_background_cpu(&mut above_blocker, 1.1);
    assert_eq!(
        above_blocker.progress().background_cpu.status,
        GateStatus::Failed
    );
    assert!(above_blocker.report(1_700_000_000_000).foundation_blocked);
}

#[test]
fn cpu_blocker_latches_after_the_rolling_window_recovers() {
    let mut monitor = ValidationMonitor::new(build("cpu-latch"));

    record_background_cpu(&mut monitor, 1.1);
    assert!(monitor.report(1_700_000_000_000).foundation_blocked);

    for index in CPU_WINDOW_SAMPLES as u64..(CPU_WINDOW_SAMPLES * 2) as u64 {
        monitor.record(sample(index, WindowState::Closed, 40 * MIB, 0.4, true));
    }

    let progress = monitor.progress();
    assert_eq!(progress.background_cpu.status, GateStatus::Passed);
    assert!((progress.background_cpu.current.unwrap() - 0.4).abs() < 0.000_001);
    assert!(monitor.report(1_700_000_000_000).foundation_blocked);
}

#[test]
fn invalid_background_cpu_sample_does_not_observe_cpu_metric() {
    let mut monitor = ValidationMonitor::new(build("cpu-not-observed"));

    monitor.record(sample(0, WindowState::Closed, 40 * MIB, 99.0, false));

    let progress = monitor.progress();
    assert_eq!(progress.background_cpu.status, GateStatus::NotObserved);
    assert_eq!(progress.background_cpu.current, None);
}

#[test]
fn session_resource_summary_tracks_samples_average_and_peaks() {
    let mut monitor = ValidationMonitor::new(build("resource-summary"));

    monitor.record(sample(0, WindowState::Open, 40 * MIB, 0.2, true));
    monitor.record(sample(1, WindowState::Open, 55 * MIB, 0.8, true));
    monitor.record(sample(2, WindowState::Closed, 45 * MIB, 99.0, false));

    let progress = monitor.progress();
    assert_eq!(progress.sample_count, 3);
    assert_eq!(progress.valid_cpu_sample_count, 2);
    assert_eq!(progress.peak_memory_bytes, Some(55 * MIB));
    assert!((progress.peak_cpu_percent.expect("peak CPU") - 0.8).abs() < 0.000_001);
    assert!((progress.average_cpu_percent.expect("average CPU") - 0.5).abs() < 0.000_001);
}

#[test]
fn background_memory_distinguishes_final_target_from_blocker() {
    let mut at_target = ValidationMonitor::new(build("memory-target"));
    at_target.record(sample(
        0,
        WindowState::Closed,
        BACKGROUND_MEMORY_TARGET,
        0.0,
        false,
    ));
    assert_eq!(
        at_target.progress().background_memory.status,
        GateStatus::Passed
    );
    assert!(!at_target.report(1_700_000_000_000).foundation_blocked);

    let mut above_target = ValidationMonitor::new(build("memory-above-target"));
    above_target.record(sample(0, WindowState::Closed, 55 * MIB, 0.0, false));
    assert_eq!(
        above_target.progress().background_memory.status,
        GateStatus::Failed
    );
    assert!(!above_target.report(1_700_000_000_000).foundation_blocked);

    let mut at_blocker = ValidationMonitor::new(build("memory-blocker"));
    at_blocker.record(sample(
        0,
        WindowState::Closed,
        BACKGROUND_MEMORY_BLOCKER,
        0.0,
        false,
    ));
    assert_eq!(
        at_blocker.progress().background_memory.status,
        GateStatus::Failed
    );
    assert!(!at_blocker.report(1_700_000_000_000).foundation_blocked);

    let mut above_blocker = ValidationMonitor::new(build("memory-above-blocker"));
    above_blocker.record(sample(
        0,
        WindowState::Closed,
        BACKGROUND_MEMORY_BLOCKER + 1,
        0.0,
        false,
    ));
    assert_eq!(
        above_blocker.progress().background_memory.status,
        GateStatus::Failed
    );
    assert!(above_blocker.report(1_700_000_000_000).foundation_blocked);
}

#[test]
fn visible_memory_passes_at_target_and_fails_above_it() {
    let mut below_target = ValidationMonitor::new(build("visible-pass"));
    below_target.record(sample(0, WindowState::Open, 170 * MIB, 0.0, false));
    assert_eq!(
        below_target.progress().visible_memory.status,
        GateStatus::Passed
    );

    let mut at_target = ValidationMonitor::new(build("visible-target"));
    at_target.record(sample(
        0,
        WindowState::Open,
        VISIBLE_MEMORY_TARGET,
        0.0,
        false,
    ));
    assert_eq!(
        at_target.progress().visible_memory.status,
        GateStatus::Passed
    );

    let mut above_target = ValidationMonitor::new(build("visible-fail"));
    above_target.record(sample(
        0,
        WindowState::Open,
        VISIBLE_MEMORY_TARGET + 1,
        0.0,
        false,
    ));
    assert_eq!(
        above_target.progress().visible_memory.status,
        GateStatus::Failed
    );
}

#[tokio::test]
async fn different_build_ids_and_lossy_sanitization_get_distinct_report_filenames() {
    let temp = tempdir().unwrap();
    let store = ValidationStore::new(temp.path().to_path_buf());

    let first = store.persist(&report_for("release-a")).await.unwrap();
    let second = store.persist(&report_for("release-b")).await.unwrap();
    let slash = store.persist(&report_for("release/a")).await.unwrap();
    let colon = store.persist(&report_for("release:a")).await.unwrap();
    let empty = store.persist(&report_for("")).await.unwrap();
    let invalid_only = store.persist(&report_for("///")).await.unwrap();
    let suffix_mimic = store
        .persist(&report_for("release_a--72656c656173652f61"))
        .await
        .unwrap();
    let literal_unknown = store.persist(&report_for("unknown")).await.unwrap();
    let first_tuple = store
        .persist(&report_for_info("1", "build-c"))
        .await
        .unwrap();
    let second_tuple = store
        .persist(&report_for_info("1-build", "c"))
        .await
        .unwrap();

    assert_ne!(first, second);
    assert_eq!(
        first.file_name().unwrap().to_string_lossy(),
        "foundation-1.2.3-release-a.json"
    );
    assert_ne!(slash, colon);
    assert_eq!(
        slash.file_name().unwrap().to_string_lossy(),
        "foundation-1.2.3-release_a--72656c656173652f61.json"
    );
    assert_ne!(slash, suffix_mimic);
    assert_ne!(empty, literal_unknown);
    assert_ne!(empty, invalid_only);
    assert_ne!(first_tuple, second_tuple);
    assert_eq!(
        empty.file_name().unwrap().to_string_lossy(),
        "foundation-1.2.3-unknown.json"
    );
    assert_eq!(
        invalid_only.file_name().unwrap().to_string_lossy(),
        "foundation-1.2.3-_--2f2f2f.json"
    );
}

#[tokio::test]
async fn atomic_persistence_writes_camel_case_json_without_a_temp_file() {
    let temp = tempdir().unwrap();
    let store = ValidationStore::new(temp.path().to_path_buf());
    let report = report_for("atomic");

    let path = store.persist(&report).await.unwrap();
    let contents = fs::read(&path).unwrap();
    let persisted: ValidationReport = serde_json::from_slice(&contents).unwrap();

    assert_eq!(persisted, report);
    let json: serde_json::Value = serde_json::from_slice(&contents).unwrap();
    assert!(json.get("schemaVersion").is_some());
    assert!(json.get("generatedAtUnixMs").is_some());
    assert!(json.get("latestSample").is_some());
    assert!(
        fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
    );
}

#[tokio::test]
async fn concurrent_persistence_for_the_same_destination_succeeds_and_cleans_temps() {
    let temp = tempdir().unwrap();
    let store = Arc::new(ValidationStore::new(temp.path().to_path_buf()));
    let first_report = report_for("concurrent");
    let mut second_report = first_report.clone();
    second_report.generated_at_unix_ms += 1;

    let first_store = Arc::clone(&store);
    let second_store = Arc::clone(&store);
    let (first, second) = tokio::join!(
        first_store.persist(&first_report),
        second_store.persist(&second_report),
    );
    let first_path = first.unwrap();
    let second_path = second.unwrap();

    assert_eq!(first_path, second_path);
    let contents = fs::read(&first_path).unwrap();
    let persisted: ValidationReport = serde_json::from_slice(&contents).unwrap();
    assert!(persisted == first_report || persisted == second_report);
    assert!(
        fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".json.tmp"))
    );
}

#[test]
fn bounded_recent_history_preserves_long_lived_aggregate_state() {
    let mut monitor = ValidationMonitor::new(build("bounded-history"));
    let total = RECENT_SAMPLE_CAPACITY as u64 + 1;

    for index in 0..total {
        let memory = if index == 0 {
            BACKGROUND_MEMORY_BLOCKER + 1
        } else {
            40 * MIB
        };
        monitor.record(sample(index, WindowState::Closed, memory, 0.4, true));
    }

    let report = monitor.report(1_700_000_000_000);
    assert_eq!(report.total_sample_count, total);
    assert_eq!(
        report.latest_sample.unwrap().resident_memory_bytes,
        40 * MIB
    );
    assert_eq!(
        report.progress.background_memory.current,
        Some((BACKGROUND_MEMORY_BLOCKER + 1) as f64)
    );
    assert!(report.foundation_blocked);
    assert_eq!(
        report.progress.continuous_uptime_seconds,
        total.saturating_sub(1) * SAMPLE_INTERVAL.as_secs()
    );
}
