use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use kvmflow_domain::{ResourceSample, WindowState};
use kvmflow_platform_api::{ResourceSampleError, ResourceSampler};
use sysinfo::{
    CpuRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind,
};

/// A process snapshot record used to deterministically summarize an application tree.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRecord {
    /// The process identifier.
    pub pid: u32,
    /// The identifier of the process parent, when one is reported.
    pub parent_pid: Option<u32>,
    /// The complete command line represented as a string.
    pub command: String,
    /// Resident memory use in bytes.
    pub resident_bytes: u64,
    /// CPU use reported by sysinfo before normalizing for logical CPUs.
    pub cpu_usage: f32,
}

/// Aggregated resource usage for the selected application processes.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessTreeSummary {
    /// PIDs selected as the root, its descendants, or configured helpers.
    pub selected_pids: BTreeSet<u32>,
    /// Total resident memory use in bytes.
    pub resident_bytes: u64,
    /// Number of selected processes.
    pub process_count: u32,
    /// CPU use normalized to an entire-machine percentage.
    pub cpu_percent: f32,
}

/// Selects the application process tree and aggregates its resource usage.
pub fn summarize_process_tree(
    records: &[ProcessRecord],
    root_pid: u32,
    bundle_identifier: &str,
    logical_cpu_count: usize,
) -> ProcessTreeSummary {
    let mut selected_pids = BTreeSet::new();
    if records.iter().any(|record| record.pid == root_pid) {
        selected_pids.insert(root_pid);
    }

    let mut added_descendant = true;
    while added_descendant {
        added_descendant = false;
        for record in records {
            if record
                .parent_pid
                .is_some_and(|parent_pid| selected_pids.contains(&parent_pid))
                && selected_pids.insert(record.pid)
            {
                added_descendant = true;
            }
        }
    }

    selected_pids.extend(
        records
            .iter()
            .filter(|record| record.command.contains(bundle_identifier))
            .map(|record| record.pid),
    );

    let (resident_bytes, cpu_usage) = records
        .iter()
        .filter(|record| selected_pids.contains(&record.pid))
        .fold((0_u64, 0_f32), |(memory, cpu), record| {
            (
                memory.saturating_add(record.resident_bytes),
                cpu + record.cpu_usage,
            )
        });

    ProcessTreeSummary {
        process_count: selected_pids.len().try_into().unwrap_or(u32::MAX),
        selected_pids,
        resident_bytes,
        cpu_percent: cpu_usage / logical_cpu_count.max(1) as f32,
    }
}

fn process_refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_memory()
        .with_cpu()
        .with_cmd(UpdateKind::OnlyIfNotSet)
        .with_tasks()
}

fn cpu_refresh_kind() -> CpuRefreshKind {
    CpuRefreshKind::nothing().with_cpu_usage()
}

/// Samples an application process tree using sysinfo system snapshots.
pub struct SysinfoResourceSampler {
    system: System,
    root_pid: u32,
    bundle_identifier: String,
    has_previous_process_refresh: bool,
}

impl SysinfoResourceSampler {
    /// Creates a sampler for `root_pid` and helpers associated with `bundle_identifier`.
    pub fn new(root_pid: u32, bundle_identifier: impl Into<String>) -> Self {
        let system =
            System::new_with_specifics(RefreshKind::nothing().with_cpu(cpu_refresh_kind()));

        Self {
            system,
            root_pid,
            bundle_identifier: bundle_identifier.into(),
            has_previous_process_refresh: false,
        }
    }
}

impl ResourceSampler for SysinfoResourceSampler {
    fn sample(&mut self, window_state: WindowState) -> Result<ResourceSample, ResourceSampleError> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            process_refresh_kind(),
        );

        if self.system.process(Pid::from_u32(self.root_pid)).is_none() {
            return Err(ResourceSampleError::CurrentProcessMissing);
        }

        let records = self
            .system
            .processes()
            .iter()
            .map(|(pid, process)| ProcessRecord {
                pid: pid.as_u32(),
                parent_pid: process.parent().map(Pid::as_u32),
                command: process
                    .cmd()
                    .iter()
                    .map(|argument| argument.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" "),
                resident_bytes: process.memory(),
                cpu_usage: process.cpu_usage(),
            })
            .collect::<Vec<_>>();
        let summary = summarize_process_tree(
            &records,
            self.root_pid,
            &self.bundle_identifier,
            self.system.cpus().len(),
        );
        let cpu_valid = self.has_previous_process_refresh;
        self.has_previous_process_refresh = true;
        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| ResourceSampleError::Refresh(error.to_string()))?
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);

        Ok(ResourceSample {
            observed_at_unix_ms,
            uptime_seconds: System::uptime(),
            window_state,
            process_count: summary.process_count,
            resident_memory_bytes: summary.resident_bytes,
            cpu_percent: summary.cpu_percent,
            cpu_valid,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{cpu_refresh_kind, process_refresh_kind};
    use sysinfo::UpdateKind;

    #[test]
    fn cpu_refresh_kind_requests_usage_without_frequency() {
        let refresh_kind = cpu_refresh_kind();

        assert!(refresh_kind.cpu_usage());
        assert!(!refresh_kind.frequency());
    }

    #[test]
    fn refresh_kind_requests_command_lines_and_resource_fields() {
        let refresh_kind = process_refresh_kind();

        assert_eq!(refresh_kind.cmd(), UpdateKind::OnlyIfNotSet);
        assert!(refresh_kind.memory());
        assert!(refresh_kind.cpu());
        assert!(refresh_kind.tasks());
        assert!(!refresh_kind.disk_usage());
        assert_eq!(refresh_kind.user(), UpdateKind::Never);
        assert_eq!(refresh_kind.cwd(), UpdateKind::Never);
        assert_eq!(refresh_kind.root(), UpdateKind::Never);
        assert_eq!(refresh_kind.environ(), UpdateKind::Never);
        assert_eq!(refresh_kind.exe(), UpdateKind::Never);
    }
}
