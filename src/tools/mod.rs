pub mod fs;
pub mod process;

pub use fs::*;
pub use process::*;

/// Unified error type for native tools.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Filesystem error: {0}")]
    Fs(#[from] FsError),
    #[error("Process execution error: {0}")]
    Process(#[from] ProcessError),
}
