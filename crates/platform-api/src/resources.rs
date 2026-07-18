use kvmflow_domain::{ResourceSample, WindowState};

/// Samples resource usage for the application process tree.
pub trait ResourceSampler: Send {
    /// Captures the current application resource usage for `window_state`.
    fn sample(&mut self, window_state: WindowState) -> Result<ResourceSample, ResourceSampleError>;
}

/// Errors that can occur while sampling application resources.
#[derive(Debug, thiserror::Error)]
pub enum ResourceSampleError {
    /// The configured application root PID was not present in the system snapshot.
    #[error("current process is missing from the system snapshot")]
    CurrentProcessMissing,
    /// Refreshing or reading the system snapshot failed.
    #[error("system process refresh failed: {0}")]
    Refresh(String),
}
