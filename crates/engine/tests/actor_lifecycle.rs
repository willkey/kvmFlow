use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use kvmflow_diagnostics::{
    ValidationMonitor, ValidationReport, ValidationReportStore, ValidationStoreError,
};
use kvmflow_domain::{BuildInfo, EndpointRole, ResourceSample, RuntimePhase, WindowState};
use kvmflow_engine::{EngineCommand, EngineConfig, EngineError, EngineHandle, spawn_engine};
use kvmflow_platform_api::{ResourceSampleError, ResourceSampler};
use tempfile::TempDir;
use tokio::{sync::Notify, time::advance};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(10);
const PERSIST_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct SampleObserver {
    calls: Arc<Mutex<Vec<WindowState>>>,
    notified: Arc<Notify>,
}

impl SampleObserver {
    fn calls(&self) -> Vec<WindowState> {
        self.calls.lock().unwrap().clone()
    }

    async fn wait_for_calls(&self, expected: usize) {
        loop {
            if self.calls.lock().unwrap().len() >= expected {
                return;
            }
            self.notified.notified().await;
        }
    }
}

struct SequenceSampler {
    samples: VecDeque<Result<ResourceSample, ResourceSampleError>>,
    observer: SampleObserver,
}

impl SequenceSampler {
    fn new(samples: impl IntoIterator<Item = Result<ResourceSample, ResourceSampleError>>) -> Self {
        Self {
            samples: samples.into_iter().collect(),
            observer: SampleObserver {
                calls: Arc::new(Mutex::new(Vec::new())),
                notified: Arc::new(Notify::new()),
            },
        }
    }

    fn observer(&self) -> SampleObserver {
        self.observer.clone()
    }
}

impl ResourceSampler for SequenceSampler {
    fn sample(&mut self, window_state: WindowState) -> Result<ResourceSample, ResourceSampleError> {
        self.observer.calls.lock().unwrap().push(window_state);
        self.observer.notified.notify_one();
        self.samples
            .pop_front()
            .unwrap_or_else(|| Err(ResourceSampleError::Refresh("no queued sample".into())))
    }
}

#[derive(Clone)]
enum StoreOutcome {
    Succeeds,
    Fails(&'static str),
}

#[derive(Clone)]
struct FakeValidationStore {
    state: Arc<FakeValidationStoreState>,
}

struct FakeValidationStoreState {
    reports: Mutex<Vec<ValidationReport>>,
    outcomes: Mutex<VecDeque<StoreOutcome>>,
    directory: PathBuf,
    notified: Notify,
}

impl FakeValidationStore {
    fn new(directory: &Path, outcomes: impl IntoIterator<Item = StoreOutcome>) -> Self {
        Self {
            state: Arc::new(FakeValidationStoreState {
                reports: Mutex::new(Vec::new()),
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                directory: directory.to_path_buf(),
                notified: Notify::new(),
            }),
        }
    }

    fn reports(&self) -> Vec<ValidationReport> {
        self.state.reports.lock().unwrap().clone()
    }

    async fn wait_for_reports(&self, expected: usize) {
        loop {
            if self.state.reports.lock().unwrap().len() >= expected {
                return;
            }
            self.state.notified.notified().await;
        }
    }
}

#[async_trait]
impl ValidationReportStore for FakeValidationStore {
    async fn persist(&self, report: &ValidationReport) -> Result<PathBuf, ValidationStoreError> {
        let index = {
            let mut reports = self.state.reports.lock().unwrap();
            reports.push(report.clone());
            reports.len()
        };
        let outcome = self
            .state
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(StoreOutcome::Succeeds);
        self.state.notified.notify_one();

        match outcome {
            StoreOutcome::Succeeds => Ok(self.state.directory.join(format!("report-{index}.json"))),
            StoreOutcome::Fails(message) => Err(ValidationStoreError::Join(message.into())),
        }
    }
}

fn build() -> BuildInfo {
    BuildInfo {
        version: "0.1.0".into(),
        build_id: "actor-test".into(),
    }
}

fn sample(observed_at_unix_ms: u64, window_state: WindowState) -> ResourceSample {
    ResourceSample {
        observed_at_unix_ms,
        uptime_seconds: observed_at_unix_ms / 1_000,
        window_state,
        process_count: 3,
        resident_memory_bytes: 42 * 1024 * 1024,
        cpu_percent: 0.25,
        cpu_valid: true,
    }
}

fn spawn_test_engine(sampler: SequenceSampler, store: FakeValidationStore) -> EngineHandle {
    let build = build();
    spawn_engine(
        EngineConfig {
            role: EndpointRole::WindowsController,
            build: build.clone(),
            diagnostic_notice: None,
            sample_interval: SAMPLE_INTERVAL,
            persist_interval: PERSIST_INTERVAL,
        },
        Box::new(sampler),
        ValidationMonitor::new(build),
        Box::new(store),
    )
}

async fn wait_for_running(
    engine: &EngineHandle,
) -> tokio::sync::watch::Receiver<kvmflow_domain::AppSnapshot> {
    let mut snapshots = engine.subscribe();
    snapshots.changed().await.unwrap();
    let snapshot = snapshots.borrow().clone();
    assert_eq!(snapshot.phase, RuntimePhase::Running);
    assert_eq!(snapshot.window_state, WindowState::Closed);
    snapshots
}

fn temp_store(outcomes: impl IntoIterator<Item = StoreOutcome>) -> (TempDir, FakeValidationStore) {
    let directory = tempfile::tempdir().unwrap();
    let store = FakeValidationStore::new(directory.path(), outcomes);
    (directory, store)
}

#[tokio::test(start_paused = true)]
async fn engine_seeds_starting_and_closed_before_actor_startup() {
    let (_directory, store) = temp_store([]);
    let engine = spawn_test_engine(SequenceSampler::new([]), store);

    let snapshot = engine.current_snapshot();
    assert_eq!(snapshot.phase, RuntimePhase::Starting);
    assert_eq!(snapshot.window_state, WindowState::Closed);
}

#[tokio::test(start_paused = true)]
async fn actor_startup_asynchronously_publishes_running_and_closed() {
    let (_directory, store) = temp_store([]);
    let engine = spawn_test_engine(SequenceSampler::new([]), store);

    let snapshots = wait_for_running(&engine).await;

    assert!(!snapshots.has_changed().unwrap());
}

#[tokio::test(start_paused = true)]
async fn window_commands_only_change_window_state_and_publish_once() {
    let (_directory, store) = temp_store([]);
    let engine = spawn_test_engine(SequenceSampler::new([]), store);
    let mut snapshots = wait_for_running(&engine).await;

    engine.send(EngineCommand::WindowOpened).await.unwrap();
    snapshots.changed().await.unwrap();
    let opened = snapshots.borrow().clone();
    assert_eq!(opened.phase, RuntimePhase::Running);
    assert_eq!(opened.window_state, WindowState::Open);
    assert!(!snapshots.has_changed().unwrap());

    engine.send(EngineCommand::WindowClosed).await.unwrap();
    snapshots.changed().await.unwrap();
    let closed = snapshots.borrow().clone();
    assert_eq!(closed.phase, RuntimePhase::Running);
    assert_eq!(closed.window_state, WindowState::Closed);
    assert!(!snapshots.has_changed().unwrap());
}

#[tokio::test(start_paused = true)]
async fn sampling_waits_ten_seconds_then_consumes_one_sample_and_publishes_it() {
    let (_directory, store) = temp_store([]);
    let sampler = SequenceSampler::new([Ok(sample(10_000, WindowState::Closed))]);
    let observer = sampler.observer();
    let engine = spawn_test_engine(sampler, store);
    let mut snapshots = wait_for_running(&engine).await;

    advance(Duration::from_secs(9)).await;
    assert!(observer.calls().is_empty());
    assert!(!snapshots.has_changed().unwrap());

    advance(Duration::from_secs(1)).await;
    observer.wait_for_calls(1).await;
    snapshots.changed().await.unwrap();

    assert_eq!(observer.calls(), vec![WindowState::Closed]);
    assert_eq!(
        snapshots.borrow().latest_resource,
        Some(sample(10_000, WindowState::Closed))
    );
}

#[tokio::test(start_paused = true)]
async fn advancing_paused_time_skips_missed_sample_ticks_and_persists_once_at_sixty_seconds() {
    let (_directory, store) = temp_store([]);
    let sampler = SequenceSampler::new([Ok(sample(60_000, WindowState::Closed))]);
    let observer = sampler.observer();
    let engine = spawn_test_engine(sampler, store.clone());
    let mut snapshots = wait_for_running(&engine).await;

    advance(PERSIST_INTERVAL).await;
    store.wait_for_reports(1).await;
    snapshots.changed().await.unwrap();

    assert_eq!(observer.calls(), vec![WindowState::Closed]);
    let reports = store.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(
        reports[0].latest_sample,
        Some(sample(60_000, WindowState::Closed))
    );
    assert_eq!(reports[0].total_sample_count, 1);
    assert_eq!(
        snapshots.borrow().latest_resource,
        Some(sample(60_000, WindowState::Closed))
    );
}

#[tokio::test(start_paused = true)]
async fn periodic_persistence_errors_do_not_stop_the_actor() {
    let (_directory, store) = temp_store([StoreOutcome::Fails("periodic failure")]);
    let engine = spawn_test_engine(
        SequenceSampler::new([Ok(sample(60_000, WindowState::Closed))]),
        store.clone(),
    );
    let mut snapshots = wait_for_running(&engine).await;

    advance(PERSIST_INTERVAL).await;
    store.wait_for_reports(1).await;
    snapshots.changed().await.unwrap();

    engine.send(EngineCommand::WindowOpened).await.unwrap();
    snapshots.changed().await.unwrap();
    assert_eq!(snapshots.borrow().phase, RuntimePhase::Running);
    assert_eq!(snapshots.borrow().window_state, WindowState::Open);
}

#[tokio::test(start_paused = true)]
async fn sampler_errors_do_not_stop_the_actor() {
    let (_directory, store) = temp_store([]);
    let sampler =
        SequenceSampler::new([Err(ResourceSampleError::Refresh("sample failure".into()))]);
    let observer = sampler.observer();
    let engine = spawn_test_engine(sampler, store);
    let mut snapshots = wait_for_running(&engine).await;

    advance(SAMPLE_INTERVAL).await;
    observer.wait_for_calls(1).await;

    engine.send(EngineCommand::WindowOpened).await.unwrap();
    snapshots.changed().await.unwrap();
    assert_eq!(snapshots.borrow().phase, RuntimePhase::Running);
    assert_eq!(snapshots.borrow().window_state, WindowState::Open);
}

#[tokio::test(start_paused = true)]
async fn export_report_returns_the_store_path_without_terminating_the_actor() {
    let (directory, store) = temp_store([]);
    let engine = spawn_test_engine(SequenceSampler::new([]), store.clone());
    let mut snapshots = wait_for_running(&engine).await;

    let path = engine.export_report().await.unwrap();

    assert!(path.starts_with(directory.path()));
    assert_eq!(store.reports().len(), 1);
    engine.send(EngineCommand::WindowOpened).await.unwrap();
    snapshots.changed().await.unwrap();
    assert_eq!(snapshots.borrow().phase, RuntimePhase::Running);
}

#[tokio::test(start_paused = true)]
async fn export_report_returns_store_errors_without_terminating_the_actor() {
    let (_directory, store) = temp_store([StoreOutcome::Fails("export failure")]);
    let engine = spawn_test_engine(SequenceSampler::new([]), store.clone());
    let mut snapshots = wait_for_running(&engine).await;

    let error = engine.export_report().await.unwrap_err();

    assert!(matches!(
        error,
        EngineError::Report(ValidationStoreError::Join(message)) if message == "export failure"
    ));
    assert_eq!(store.reports().len(), 1);
    engine.send(EngineCommand::WindowOpened).await.unwrap();
    snapshots.changed().await.unwrap();
    assert_eq!(snapshots.borrow().phase, RuntimePhase::Running);
}

#[tokio::test(start_paused = true)]
async fn shutdown_publishes_stopping_persists_once_and_terminates_the_actor() {
    let (_directory, store) = temp_store([]);
    let engine = spawn_test_engine(SequenceSampler::new([]), store.clone());
    let mut snapshots = wait_for_running(&engine).await;

    engine.shutdown().await.unwrap();
    snapshots.changed().await.unwrap();

    assert_eq!(snapshots.borrow().phase, RuntimePhase::Stopping);
    assert_eq!(store.reports().len(), 1);
    assert!(snapshots.changed().await.is_err());
    assert!(matches!(
        engine.send(EngineCommand::WindowOpened).await,
        Err(EngineError::Stopped)
    ));
}

#[tokio::test(start_paused = true)]
async fn shutdown_returns_store_errors_after_publishing_stopping_and_terminating() {
    let (_directory, store) = temp_store([StoreOutcome::Fails("shutdown failure")]);
    let engine = spawn_test_engine(SequenceSampler::new([]), store.clone());
    let mut snapshots = wait_for_running(&engine).await;

    let error = engine.shutdown().await.unwrap_err();
    snapshots.changed().await.unwrap();

    assert!(matches!(
        error,
        EngineError::Report(ValidationStoreError::Join(message)) if message == "shutdown failure"
    ));
    assert_eq!(snapshots.borrow().phase, RuntimePhase::Stopping);
    assert_eq!(store.reports().len(), 1);
    assert!(snapshots.changed().await.is_err());
    assert!(matches!(
        engine.send(EngineCommand::WindowOpened).await,
        Err(EngineError::Stopped)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_acknowledgement_rejects_every_immediate_follow_up_command() {
    const ATTEMPTS: usize = 128;

    for _ in 0..ATTEMPTS {
        let (_directory, store) = temp_store([]);
        let engine = spawn_test_engine(SequenceSampler::new([]), store);
        wait_for_running(&engine).await;

        engine.shutdown().await.unwrap();

        assert!(matches!(
            engine.send(EngineCommand::WindowOpened).await,
            Err(EngineError::Stopped)
        ));
    }
}
