use std::{collections::VecDeque, fmt::Write as _, fs, io::Write, path::PathBuf, time::Duration};

use kvmflow_domain::{
    BuildInfo, GateStatus, MetricProgress, ResourceSample, ValidationProgress, WindowState,
};
use serde::{Deserialize, Serialize};
use tempfile::{Builder, NamedTempFile};

pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(10);
pub const PERSIST_INTERVAL: Duration = Duration::from_secs(60);
pub const CPU_WINDOW_SAMPLES: usize = 60;
pub const RECENT_SAMPLE_CAPACITY: usize = 360;
pub const BACKGROUND_MEMORY_TARGET: u64 = 50 * 1024 * 1024;
pub const BACKGROUND_MEMORY_BLOCKER: u64 = 80 * 1024 * 1024;
pub const VISIBLE_MEMORY_TARGET: u64 = 180 * 1024 * 1024;
pub const BACKGROUND_CPU_TARGET: f32 = 0.5;
pub const BACKGROUND_CPU_BLOCKER: f32 = 1.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub schema_version: u16,
    pub build: BuildInfo,
    pub generated_at_unix_ms: u64,
    pub latest_sample: Option<ResourceSample>,
    pub progress: ValidationProgress,
    pub foundation_blocked: bool,
    pub total_sample_count: u64,
    pub valid_background_cpu_sample_count: u64,
}

pub struct ValidationMonitor {
    build: BuildInfo,
    recent_samples: VecDeque<ResourceSample>,
    first_observation_unix_ms: Option<u64>,
    latest_sample: Option<ResourceSample>,
    continuous_uptime_seconds: u64,
    background_max_memory: Option<u64>,
    visible_max_memory: Option<u64>,
    background_cpu_window: VecDeque<f32>,
    background_cpu_sum: f64,
    cpu_foundation_blocked: bool,
    total_sample_count: u64,
    valid_background_cpu_sample_count: u64,
    valid_cpu_sample_count: u64,
    cpu_sum: f64,
    peak_cpu_percent: Option<f32>,
    peak_memory_bytes: Option<u64>,
}

impl ValidationMonitor {
    pub fn new(build: BuildInfo) -> Self {
        Self {
            build,
            recent_samples: VecDeque::with_capacity(RECENT_SAMPLE_CAPACITY),
            first_observation_unix_ms: None,
            latest_sample: None,
            continuous_uptime_seconds: 0,
            background_max_memory: None,
            visible_max_memory: None,
            background_cpu_window: VecDeque::with_capacity(CPU_WINDOW_SAMPLES),
            background_cpu_sum: 0.0,
            cpu_foundation_blocked: false,
            total_sample_count: 0,
            valid_background_cpu_sample_count: 0,
            valid_cpu_sample_count: 0,
            cpu_sum: 0.0,
            peak_cpu_percent: None,
            peak_memory_bytes: None,
        }
    }

    pub fn record(&mut self, sample: ResourceSample) -> ValidationProgress {
        let first_observation = self
            .first_observation_unix_ms
            .get_or_insert(sample.observed_at_unix_ms);
        self.continuous_uptime_seconds = sample
            .observed_at_unix_ms
            .saturating_sub(*first_observation)
            / 1_000;
        self.latest_sample = Some(sample);
        self.total_sample_count = self.total_sample_count.saturating_add(1);
        update_max(&mut self.peak_memory_bytes, sample.resident_memory_bytes);
        if sample.cpu_valid {
            self.valid_cpu_sample_count = self.valid_cpu_sample_count.saturating_add(1);
            self.cpu_sum += f64::from(sample.cpu_percent);
            self.peak_cpu_percent = Some(
                self.peak_cpu_percent
                    .map_or(sample.cpu_percent, |peak| peak.max(sample.cpu_percent)),
            );
        }

        self.recent_samples.push_back(sample);
        if self.recent_samples.len() > RECENT_SAMPLE_CAPACITY {
            self.recent_samples.pop_front();
        }

        match sample.window_state {
            WindowState::Closed => {
                update_max(
                    &mut self.background_max_memory,
                    sample.resident_memory_bytes,
                );
                if sample.cpu_valid {
                    self.record_background_cpu(sample.cpu_percent);
                }
            }
            WindowState::Open => {
                update_max(&mut self.visible_max_memory, sample.resident_memory_bytes)
            }
        }

        self.progress()
    }

    pub fn progress(&self) -> ValidationProgress {
        ValidationProgress {
            background_memory: memory_progress(
                self.background_max_memory,
                BACKGROUND_MEMORY_TARGET,
            ),
            visible_memory: memory_progress(self.visible_max_memory, VISIBLE_MEMORY_TARGET),
            background_cpu: self.background_cpu_progress(),
            continuous_uptime_seconds: self.continuous_uptime_seconds,
            sample_count: self.total_sample_count,
            valid_cpu_sample_count: self.valid_cpu_sample_count,
            peak_memory_bytes: self.peak_memory_bytes,
            average_cpu_percent: (self.valid_cpu_sample_count > 0)
                .then(|| self.cpu_sum / self.valid_cpu_sample_count as f64),
            peak_cpu_percent: self.peak_cpu_percent.map(f64::from),
        }
    }

    pub fn report(&self, generated_at_unix_ms: u64) -> ValidationReport {
        ValidationReport {
            schema_version: 1,
            build: self.build.clone(),
            generated_at_unix_ms,
            latest_sample: self.latest_sample,
            progress: self.progress(),
            foundation_blocked: self.foundation_blocked(),
            total_sample_count: self.total_sample_count,
            valid_background_cpu_sample_count: self.valid_background_cpu_sample_count,
        }
    }

    fn record_background_cpu(&mut self, cpu_percent: f32) {
        self.background_cpu_window.push_back(cpu_percent);
        self.background_cpu_sum += f64::from(cpu_percent);
        self.valid_background_cpu_sample_count =
            self.valid_background_cpu_sample_count.saturating_add(1);

        if self.background_cpu_window.len() > CPU_WINDOW_SAMPLES {
            let oldest = self
                .background_cpu_window
                .pop_front()
                .expect("CPU window has an oldest sample when it exceeds its capacity");
            self.background_cpu_sum -= f64::from(oldest);
        }

        self.cpu_foundation_blocked |= self.background_cpu_window.len() == CPU_WINDOW_SAMPLES
            && self
                .background_cpu_average()
                .is_some_and(|cpu| cpu > f64::from(BACKGROUND_CPU_BLOCKER));
    }

    fn background_cpu_progress(&self) -> MetricProgress {
        let Some(average) = self.background_cpu_average() else {
            return MetricProgress {
                status: GateStatus::NotObserved,
                current: None,
                target: f64::from(BACKGROUND_CPU_TARGET),
                unit: "percent".into(),
            };
        };

        let status = if self.background_cpu_window.len() < CPU_WINDOW_SAMPLES {
            GateStatus::Collecting
        } else if average <= f64::from(BACKGROUND_CPU_TARGET) {
            GateStatus::Passed
        } else {
            GateStatus::Failed
        };

        MetricProgress {
            status,
            current: Some(average),
            target: f64::from(BACKGROUND_CPU_TARGET),
            unit: "percent".into(),
        }
    }

    fn background_cpu_average(&self) -> Option<f64> {
        (!self.background_cpu_window.is_empty())
            .then(|| self.background_cpu_sum / self.background_cpu_window.len() as f64)
    }

    fn foundation_blocked(&self) -> bool {
        self.background_max_memory
            .is_some_and(|memory| memory > BACKGROUND_MEMORY_BLOCKER)
            || self.cpu_foundation_blocked
    }
}

fn update_max(current: &mut Option<u64>, value: u64) {
    *current = Some(current.map_or(value, |maximum| maximum.max(value)));
}

fn memory_progress(maximum: Option<u64>, target: u64) -> MetricProgress {
    match maximum {
        Some(current) => MetricProgress {
            status: if current <= target {
                GateStatus::Passed
            } else {
                GateStatus::Failed
            },
            current: Some(current as f64),
            target: target as f64,
            unit: "bytes".into(),
        },
        None => MetricProgress {
            status: GateStatus::NotObserved,
            current: None,
            target: target as f64,
            unit: "bytes".into(),
        },
    }
}

#[async_trait::async_trait]
pub trait ValidationReportStore: Send + Sync {
    async fn persist(&self, report: &ValidationReport) -> Result<PathBuf, ValidationStoreError>;
}

pub struct ValidationStore {
    diagnostics_dir: PathBuf,
}

impl ValidationStore {
    pub fn new(diagnostics_dir: PathBuf) -> Self {
        Self { diagnostics_dir }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationStoreError {
    #[error("validation report I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("validation report serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("validation report task failed: {0}")]
    Join(String),
}

#[async_trait::async_trait]
impl ValidationReportStore for ValidationStore {
    async fn persist(&self, report: &ValidationReport) -> Result<PathBuf, ValidationStoreError> {
        let diagnostics_dir = self.diagnostics_dir.clone();
        let report = report.clone();

        tokio::task::spawn_blocking(move || persist_report(diagnostics_dir, &report))
            .await
            .map_err(|error| ValidationStoreError::Join(error.to_string()))?
    }
}

fn persist_report(
    diagnostics_dir: PathBuf,
    report: &ValidationReport,
) -> Result<PathBuf, ValidationStoreError> {
    fs::create_dir_all(&diagnostics_dir)?;

    let destination = diagnostics_dir.join(report_filename(&report.build));
    let serialized = serde_json::to_vec_pretty(report)?;
    let mut temporary: NamedTempFile = Builder::new()
        .prefix("foundation-")
        .suffix(".json.tmp")
        .tempfile_in(&diagnostics_dir)?;
    temporary.write_all(&serialized)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&destination)
        .map_err(|error| ValidationStoreError::Io(error.error))?;

    Ok(destination)
}

fn report_filename(build: &BuildInfo) -> String {
    format!(
        "foundation-{}-{}.json",
        sanitize_filename_component(&build.version, true),
        sanitize_filename_component(&build.build_id, false)
    )
}

fn sanitize_filename_component(value: &str, reserve_hyphens: bool) -> String {
    let mut sanitized = String::new();
    let mut invalid_run = false;
    let mut changed = false;

    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            if invalid_run {
                sanitized.push('_');
                invalid_run = false;
            }
            sanitized.push(byte as char);
        } else {
            invalid_run = true;
            changed = true;
        }
    }

    if invalid_run {
        sanitized.push('_');
    }
    if sanitized.is_empty() {
        return "unknown".into();
    }
    if changed
        || value.contains("--")
        || value == "unknown"
        || (reserve_hyphens && value.contains('-'))
    {
        sanitized.push_str("--");
        for byte in value.bytes() {
            write!(sanitized, "{byte:02x}").expect("writing to a String cannot fail");
        }
    }

    sanitized
}
