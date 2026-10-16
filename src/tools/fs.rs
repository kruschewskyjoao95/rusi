use std::path::{Path, PathBuf};
use thiserror::Error;

/// Error types for filesystem operations.
#[derive(Debug, Error)]
pub enum FsError {
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    #[error("File already exists: '{0}'. Pass overwrite=true to overwrite.")]
    FileAlreadyExists(PathBuf),

    #[error("Invalid line range: start_line ({start}) > end_line ({end})")]
    InvalidLineRange { start: usize, end: usize },

    #[error("Line out of range: requested line {requested}, but file only has {total} lines")]
    LineOutOfRange { requested: usize, total: usize },

    #[error("Target block not found in file: '{0}'")]
    TargetBlockNotFound(PathBuf),

    #[error("Ambiguous target block in file '{path}': found {count} occurrences")]
    AmbiguousTargetBlock { path: PathBuf, count: usize },

    #[error("Target block cannot be empty")]
    EmptyTargetBlock,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Reads lines from a file with slice range support and 1-indexed line numbering.
///
/// Output format: each line is formatted as `"{line_no}: {line}\n"`.
///
/// Behavior:
/// - If the file does not exist, returns `FsError::FileNotFound`.
/// - If the file is empty, returns `Ok(String::new())`.
/// - `start_line` defaults to 1 (clamped to 1 if `Some(0)` is provided).
/// - `end_line` defaults to the total number of lines in the file.
/// - If `start_line > total_lines`, returns `FsError::LineOutOfRange`.
/// - If `start_line > end_line`, returns `FsError::InvalidLineRange`.
/// - `end_line` is clamped to `total_lines` if `end_line > total_lines`.
pub fn file_read(
    path: impl AsRef<Path>,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, FsError> {
    let path = path.as_ref();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(FsError::FileNotFound(path.to_path_buf()));
        }
        Err(e) => return Err(FsError::Io(e)),
    };

    if content.is_empty() {
        return Ok(String::new());
    }

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    if total_lines == 0 {
        return Ok(String::new());
    }

    let start = start_line.unwrap_or(1).max(1);
    let end = end_line.unwrap_or(total_lines);

    if start > total_lines {
        return Err(FsError::LineOutOfRange {
            requested: start,
            total: total_lines,
        });
    }

    if start > end {
        return Err(FsError::InvalidLineRange { start, end });
    }

    let clamped_end = end.min(total_lines);

    let mut output = String::new();
    for (idx, line) in lines[start - 1..clamped_end].iter().enumerate() {
        let line_no = start + idx;
        output.push_str(&format!("{line_no}: {line}\n"));
    }

    Ok(output)
}

/// Writes content to a file, creating parent directories if they do not exist.
///
/// If `overwrite` is false and the file already exists, returns `FsError::FileAlreadyExists`.
pub fn file_write(
    path: impl AsRef<Path>,
    content: &str,
    overwrite: bool,
) -> Result<(), FsError> {
    let path = path.as_ref();

    if path.exists() && !overwrite {
        return Err(FsError::FileAlreadyExists(path.to_path_buf()));
    }

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, content)?;
    Ok(())
}

/// Modifies a contiguous block in a file by locating the exact target block.
///
/// - If `target_block` is empty, returns `FsError::EmptyTargetBlock`.
/// - If the file does not exist, returns `FsError::FileNotFound`.
/// - If `target_block` is not found, returns `FsError::TargetBlockNotFound`.
/// - If `target_block` appears more than once, returns `FsError::AmbiguousTargetBlock`.
/// - Replaces the single occurrence with `replacement_block` and writes back to file.
pub fn file_edit(
    path: impl AsRef<Path>,
    target_block: &str,
    replacement_block: &str,
) -> Result<(), FsError> {
    let path = path.as_ref();

    if target_block.is_empty() {
        return Err(FsError::EmptyTargetBlock);
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(FsError::FileNotFound(path.to_path_buf()));
        }
        Err(e) => return Err(FsError::Io(e)),
    };

    let count = content.matches(target_block).count();
    match count {
        0 => Err(FsError::TargetBlockNotFound(path.to_path_buf())),
        1 => {
            let updated = content.replacen(target_block, replacement_block, 1);
            std::fs::write(path, updated)?;
            Ok(())
        }
        _ => Err(FsError::AmbiguousTargetBlock {
            path: path.to_path_buf(),
            count,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(prefix: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "rusi_fs_test_{prefix}_{}_{}",
                std::process::id(),
                id
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("Failed to create temp test directory");
            Self { path }
        }

        fn file_path(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_file_read_full_and_line_numbering() {
        let temp = TempDirGuard::new("read_full");
        let file = temp.file_path("test.txt");
        std::fs::write(&file, "first line\nsecond line\nthird line").unwrap();

        let output = file_read(&file, None, None).expect("file_read failed");
        assert_eq!(
            output,
            "1: first line\n2: second line\n3: third line\n"
        );
    }

    #[test]
    fn test_file_read_slice_middle() {
        let temp = TempDirGuard::new("read_middle");
        let file = temp.file_path("test.txt");
        std::fs::write(&file, "one\ntwo\nthree\nfour\nfive").unwrap();

        let output = file_read(&file, Some(2), Some(4)).expect("file_read failed");
        assert_eq!(
            output,
            "2: two\n3: three\n4: four\n"
        );
    }

    #[test]
    fn test_file_read_clamping_end_line() {
        let temp = TempDirGuard::new("read_clamp");
        let file = temp.file_path("test.txt");
        std::fs::write(&file, "alpha\nbeta\ngamma").unwrap();

        let output = file_read(&file, Some(2), Some(100)).expect("file_read failed");
        assert_eq!(output, "2: beta\n3: gamma\n");
    }

    #[test]
    fn test_file_read_start_zero_clamped_to_one() {
        let temp = TempDirGuard::new("read_zero");
        let file = temp.file_path("test.txt");
        std::fs::write(&file, "alpha\nbeta\ngamma").unwrap();

        let output = file_read(&file, Some(0), Some(2)).expect("file_read failed");
        assert_eq!(output, "1: alpha\n2: beta\n");
    }

    #[test]
    fn test_file_read_line_out_of_range() {
        let temp = TempDirGuard::new("read_oor");
        let file = temp.file_path("test.txt");
        std::fs::write(&file, "line1\nline2").unwrap();

        let err = file_read(&file, Some(5), None).unwrap_err();
        match err {
            FsError::LineOutOfRange { requested, total } => {
                assert_eq!(requested, 5);
                assert_eq!(total, 2);
            }
            other => panic!("Unexpected error: {:?}", other),
        }
    }

    #[test]
    fn test_file_read_invalid_line_range() {
        let temp = TempDirGuard::new("read_inv");
        let file = temp.file_path("test.txt");
        std::fs::write(&file, "line1\nline2\nline3\nline4").unwrap();

        let err = file_read(&file, Some(3), Some(1)).unwrap_err();
        match err {
            FsError::InvalidLineRange { start, end } => {
                assert_eq!(start, 3);
                assert_eq!(end, 1);
            }
            other => panic!("Unexpected error: {:?}", other),
        }
    }

    #[test]
    fn test_file_read_file_not_found() {
        let temp = TempDirGuard::new("read_not_found");
        let missing = temp.file_path("non_existent.txt");

        let err = file_read(&missing, None, None).unwrap_err();
        assert!(matches!(err, FsError::FileNotFound(_)));
    }

    #[test]
    fn test_file_read_empty_file() {
        let temp = TempDirGuard::new("read_empty");
        let file = temp.file_path("empty.txt");
        std::fs::write(&file, "").unwrap();

        let output = file_read(&file, None, None).expect("file_read on empty file failed");
        assert_eq!(output, "");
    }

    #[test]
    fn test_file_write_new_file() {
        let temp = TempDirGuard::new("write_new");
        let file = temp.file_path("hello.txt");

        file_write(&file, "hello world", false).expect("file_write failed");
        let read_back = std::fs::read_to_string(&file).unwrap();
        assert_eq!(read_back, "hello world");
    }

    #[test]
    fn test_file_write_creates_parent_dirs() {
        let temp = TempDirGuard::new("write_parents");
        let nested_file = temp.path.join("a").join("b").join("c").join("deep.txt");

        assert!(!nested_file.parent().unwrap().exists());
        file_write(&nested_file, "nested content", false).expect("nested write failed");

        assert!(nested_file.exists());
        assert_eq!(std::fs::read_to_string(&nested_file).unwrap(), "nested content");
    }

    #[test]
    fn test_file_write_overwrite_guard() {
        let temp = TempDirGuard::new("write_guard");
        let file = temp.file_path("guard.txt");
        std::fs::write(&file, "initial").unwrap();

        let err = file_write(&file, "overwrite attempt", false).unwrap_err();
        assert!(matches!(err, FsError::FileAlreadyExists(_)));

        // Verify original content was not touched
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "initial");

        // Now test overwrite=true succeeds
        file_write(&file, "overwritten", true).expect("overwrite=true should succeed");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "overwritten");
    }

    #[test]
    fn test_file_edit_single_unique_match() {
        let temp = TempDirGuard::new("edit_single");
        let file = temp.file_path("edit.txt");
        std::fs::write(&file, "fn foo() {\n    let x = 10;\n    x + 1\n}").unwrap();

        file_edit(&file, "let x = 10;", "let x = 42;").expect("file_edit failed");

        let updated = std::fs::read_to_string(&file).unwrap();
        assert_eq!(updated, "fn foo() {\n    let x = 42;\n    x + 1\n}");
    }

    #[test]
    fn test_file_edit_multiline_block() {
        let temp = TempDirGuard::new("edit_multiline");
        let file = temp.file_path("multi.txt");
        std::fs::write(&file, "header\nstart\nmiddle\nend\nfooter").unwrap();

        let target = "start\nmiddle\nend";
        let replacement = "replaced_block";
        file_edit(&file, target, replacement).expect("multiline edit failed");

        let updated = std::fs::read_to_string(&file).unwrap();
        assert_eq!(updated, "header\nreplaced_block\nfooter");
    }

    #[test]
    fn test_file_edit_target_not_found() {
        let temp = TempDirGuard::new("edit_not_found");
        let file = temp.file_path("file.txt");
        std::fs::write(&file, "hello world").unwrap();

        let err = file_edit(&file, "missing phrase", "replacement").unwrap_err();
        assert!(matches!(err, FsError::TargetBlockNotFound(_)));
    }

    #[test]
    fn test_file_edit_ambiguous_matches() {
        let temp = TempDirGuard::new("edit_ambiguous");
        let file = temp.file_path("ambig.txt");
        std::fs::write(&file, "repeat\nsomething\nrepeat\nsomething\nrepeat").unwrap();

        let err = file_edit(&file, "repeat", "single").unwrap_err();
        match err {
            FsError::AmbiguousTargetBlock { count, .. } => {
                assert_eq!(count, 3);
            }
            other => panic!("Expected AmbiguousTargetBlock, got {:?}", other),
        }
    }

    #[test]
    fn test_file_edit_empty_target_block() {
        let temp = TempDirGuard::new("edit_empty");
        let file = temp.file_path("file.txt");
        std::fs::write(&file, "some content").unwrap();

        let err = file_edit(&file, "", "replacement").unwrap_err();
        assert!(matches!(err, FsError::EmptyTargetBlock));
    }

    #[test]
    fn test_file_edit_file_not_found() {
        let temp = TempDirGuard::new("edit_missing");
        let missing = temp.file_path("non_existent.txt");

        let err = file_edit(&missing, "target", "replacement").unwrap_err();
        assert!(matches!(err, FsError::FileNotFound(_)));
    }
}
