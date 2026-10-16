use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// An isolated temporary directory guard for E2E tests.
///
/// Automatically creates a unique directory under `std::env::temp_dir()`
/// and recursively deletes it on drop.
pub struct TestEnv {
    path: PathBuf,
}

impl TestEnv {
    /// Creates a new isolated test environment directory.
    pub fn new() -> Self {
        let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir_name = format!(
            "rusi_e2e_test_{}_{}_{}",
            std::process::id(),
            timestamp,
            counter
        );
        let path = std::env::temp_dir().join(dir_name);
        fs::create_dir_all(&path).expect("Failed to create temporary test directory");
        Self { path }
    }

    /// Returns the root path of the test environment.
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolves a relative path within the test environment.
    #[allow(dead_code)]
    pub fn file_path(&self, rel_path: impl AsRef<Path>) -> PathBuf {
        self.path.join(rel_path)
    }

    /// Creates a file with the given content at the relative path.
    /// Parent directories are created automatically if they do not exist.
    #[allow(dead_code)]
    pub fn create_file(&self, rel_path: impl AsRef<Path>, content: &str) -> PathBuf {
        let full_path = self.file_path(rel_path);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent directories");
        }
        fs::write(&full_path, content).expect("Failed to write test file");
        full_path
    }

    /// Reads the entire contents of a file at the relative path.
    #[allow(dead_code)]
    pub fn read_file(&self, rel_path: impl AsRef<Path>) -> String {
        let full_path = self.file_path(rel_path);
        fs::read_to_string(full_path).expect("Failed to read test file")
    }

    /// Checks if a file or directory exists at the relative path.
    #[allow(dead_code)]
    pub fn exists(&self, rel_path: impl AsRef<Path>) -> bool {
        self.file_path(rel_path).exists()
    }
}

impl Default for TestEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
