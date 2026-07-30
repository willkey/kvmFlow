#![forbid(unsafe_code)]

//! Provides V2 logging, diagnostics, and resource-monitoring boundaries.

mod logging;
mod resources;
mod storage;
mod validation;

pub use logging::{
    LoggingError, LoggingGuard, RetentionPolicy, SizeRollingWriter, TimeSource, init_file_logging,
};
pub use resources::{
    ProcessRecord, ProcessTreeSummary, SysinfoResourceSampler, summarize_process_tree,
};
pub use storage::{
    ExecutableKind, PreparedStorage, StorageError, StorageLayout, StorageWarning, prepare_storage,
    prepare_storage_root, resolve_install_root,
};
pub use validation::{
    BACKGROUND_CPU_BLOCKER, BACKGROUND_CPU_TARGET, BACKGROUND_MEMORY_BLOCKER,
    BACKGROUND_MEMORY_TARGET, CPU_WINDOW_SAMPLES, KvmValidationEvent, MEMORY_WINDOW_SAMPLES,
    PERSIST_INTERVAL, RECENT_SAMPLE_CAPACITY, SAMPLE_INTERVAL, VISIBLE_MEMORY_TARGET,
    ValidationMonitor, ValidationReport, ValidationReportStore, ValidationStore,
    ValidationStoreError,
};
