mod common;

use common::TestEnv;
use rusi::tools::fs::{file_edit, file_read, file_write, FsError};

// ============================================================================
// Tier 1: Feature Coverage (Representative Happy-Path Inputs)
// ============================================================================

#[test]
fn test_tier1_file_read_full_content_and_numbering() {
    let env = TestEnv::new();
    let file = env.create_file("sample.txt", "alpha\nbeta\ngamma\n");

    let result = file_read(&file, None, None).expect("file_read should succeed");
    let expected = "1: alpha\n2: beta\n3: gamma\n";
    assert_eq!(result, expected);
}

#[test]
fn test_tier1_file_read_middle_slice() {
    let env = TestEnv::new();
    let content = "line 1\nline 2\nline 3\nline 4\nline 5\n";
    let file = env.create_file("slice.txt", content);

    let result = file_read(&file, Some(2), Some(4)).expect("file_read middle slice should succeed");
    let expected = "2: line 2\n3: line 3\n4: line 4\n";
    assert_eq!(result, expected);
}

#[test]
fn test_tier1_file_read_prefix_slice() {
    let env = TestEnv::new();
    let content = "header\nrow 1\nrow 2\nrow 3\n";
    let file = env.create_file("prefix.txt", content);

    let result = file_read(&file, Some(1), Some(2)).expect("file_read prefix slice should succeed");
    let expected = "1: header\n2: row 1\n";
    assert_eq!(result, expected);
}

#[test]
fn test_tier1_file_read_suffix_slice() {
    let env = TestEnv::new();
    let content = "first\nsecond\nthird\nfourth\nfifth\n";
    let file = env.create_file("suffix.txt", content);

    let result = file_read(&file, Some(4), None).expect("file_read suffix slice should succeed");
    let expected = "4: fourth\n5: fifth\n";
    assert_eq!(result, expected);
}

#[test]
fn test_tier1_file_write_new_file() {
    let env = TestEnv::new();
    let target = env.file_path("new_document.txt");
    let payload = "Hello, Rusi native filesystem!\nSecond line.\n";

    let res = file_write(&target, payload, false);
    assert!(res.is_ok(), "file_write should succeed for new file");

    let on_disk = env.read_file("new_document.txt");
    assert_eq!(on_disk, payload);
}

#[test]
fn test_tier1_file_write_creates_deep_nested_directories() {
    let env = TestEnv::new();
    let target = env.file_path("deeply/nested/sub/directory/level/data.json");
    let payload = r#"{"status": "ok", "nested": true}"#;

    let res = file_write(&target, payload, false);
    assert!(
        res.is_ok(),
        "file_write must automatically create all non-existent parent directory levels"
    );

    assert!(env.exists("deeply/nested/sub/directory/level/data.json"));
    let on_disk = env.read_file("deeply/nested/sub/directory/level/data.json");
    assert_eq!(on_disk, payload);
}

#[test]
fn test_tier1_file_write_overwrite_true_replaces_content() {
    let env = TestEnv::new();
    let target = env.create_file("existing.txt", "initial content that will be overwritten");

    let new_content = "brand new pristine content";
    let res = file_write(&target, new_content, true);
    assert!(res.is_ok(), "file_write with overwrite=true must succeed");

    let on_disk = env.read_file("existing.txt");
    assert_eq!(on_disk, new_content);
}

#[test]
fn test_tier1_file_edit_unique_single_line() {
    let env = TestEnv::new();
    let original = "let a = 1;\nlet b = 2;\nlet c = 3;\n";
    let file = env.create_file("code.rs", original);

    let res = file_edit(&file, "let b = 2;", "let b = 200;");
    assert!(res.is_ok(), "file_edit should succeed for unique single-line target block");

    let updated = env.read_file("code.rs");
    let expected = "let a = 1;\nlet b = 200;\nlet c = 3;\n";
    assert_eq!(updated, expected);
}

#[test]
fn test_tier1_file_edit_multiline_block() {
    let env = TestEnv::new();
    let original = "fn calculate() -> i32 {\n    let temp = 10;\n    temp * 2\n}\n";
    let file = env.create_file("fn.rs", original);

    let target = "    let temp = 10;\n    temp * 2";
    let replacement = "    let factor = 5;\n    let temp = 10;\n    temp * factor";

    let res = file_edit(&file, target, replacement);
    assert!(res.is_ok(), "file_edit should replace contiguous multiline blocks");

    let updated = env.read_file("fn.rs");
    let expected = "fn calculate() -> i32 {\n    let factor = 5;\n    let temp = 10;\n    temp * factor\n}\n";
    assert_eq!(updated, expected);
}

#[test]
fn test_tier1_file_edit_preserves_special_characters_and_whitespace() {
    let env = TestEnv::new();
    let original = "\t\t// regex: ^[a-z]+$\n\t\tlet pattern = r\"\\d+\";\n";
    let file = env.create_file("special.rs", original);

    let target = "\t\tlet pattern = r\"\\d+\";";
    let replacement = "\t\tlet pattern = r\"[A-Z0-9_]+\";";

    let res = file_edit(&file, target, replacement);
    assert!(res.is_ok());

    let updated = env.read_file("special.rs");
    let expected = "\t\t// regex: ^[a-z]+$\n\t\tlet pattern = r\"[A-Z0-9_]+\";\n";
    assert_eq!(updated, expected);
}

// ============================================================================
// Tier 2: Boundary & Corner Cases (Edge Conditions & Failure Modes)
// ============================================================================

#[test]
fn test_tier2_file_read_empty_file() {
    let env = TestEnv::new();
    let file = env.create_file("empty.txt", "");

    let result = file_read(&file, None, None).expect("Reading an empty file must return Ok");
    assert_eq!(result, "", "Empty file must return empty string");
}

#[test]
fn test_tier2_file_read_non_existent_file_returns_not_found() {
    let env = TestEnv::new();
    let non_existent = env.file_path("does_not_exist.txt");

    let err = file_read(&non_existent, None, None).unwrap_err();
    match err {
        FsError::FileNotFound(path) => {
            assert_eq!(path, non_existent);
        }
        other => panic!("Expected FsError::FileNotFound, got: {other:?}"),
    }
}

#[test]
fn test_tier2_file_read_start_line_out_of_range() {
    let env = TestEnv::new();
    let file = env.create_file("short.txt", "line1\nline2\nline3\n");

    let err = file_read(&file, Some(10), None).unwrap_err();
    match err {
        FsError::LineOutOfRange { requested, total } => {
            assert_eq!(requested, 10);
            assert_eq!(total, 3);
        }
        other => panic!("Expected FsError::LineOutOfRange, got: {other:?}"),
    }
}

#[test]
fn test_tier2_file_read_start_greater_than_end_line() {
    let env = TestEnv::new();
    let file = env.create_file("range.txt", "one\ntwo\nthree\nfour\nfive\n");

    let err = file_read(&file, Some(4), Some(2)).unwrap_err();
    match err {
        FsError::InvalidLineRange { start, end } => {
            assert_eq!(start, 4);
            assert_eq!(end, 2);
        }
        other => panic!("Expected FsError::InvalidLineRange, got: {other:?}"),
    }
}

#[test]
fn test_tier2_file_read_end_line_clamping() {
    let env = TestEnv::new();
    let file = env.create_file("clamp.txt", "first\nsecond\nthird\n");

    // end_line 999 should be clamped to total_lines (3)
    let result = file_read(&file, Some(2), Some(999)).expect("Clamping end_line should succeed");
    let expected = "2: second\n3: third\n";
    assert_eq!(result, expected);
}

#[test]
fn test_tier2_file_read_start_zero_clamped_to_one() {
    let env = TestEnv::new();
    let file = env.create_file("zero.txt", "line1\nline2\n");

    // start_line = 0 should be normalized to 1
    let result = file_read(&file, Some(0), Some(1)).expect("start_line 0 should be normalized to 1");
    let expected = "1: line1\n";
    assert_eq!(result, expected);
}

#[test]
fn test_tier2_file_write_overwrite_guard_fails_when_file_exists() {
    let env = TestEnv::new();
    let file = env.create_file("immutable.txt", "original untouchable content");

    let res = file_write(&file, "malicious replacement", false);
    assert!(
        res.is_err(),
        "file_write with overwrite=false must fail if target file exists"
    );

    match res.unwrap_err() {
        FsError::FileAlreadyExists(path) => {
            assert_eq!(path, file);
        }
        other => panic!("Expected FsError::FileAlreadyExists, got: {other:?}"),
    }

    // Verify original content was not modified
    let on_disk = env.read_file("immutable.txt");
    assert_eq!(on_disk, "original untouchable content");
}

#[test]
fn test_tier2_file_edit_empty_target_block_fails() {
    let env = TestEnv::new();
    let file = env.create_file("target.txt", "some text in file");

    let res = file_edit(&file, "", "replacement");
    assert!(res.is_err(), "Empty target_block must return an error");
    match res.unwrap_err() {
        FsError::EmptyTargetBlock => {}
        other => panic!("Expected FsError::EmptyTargetBlock, got: {other:?}"),
    }
}

#[test]
fn test_tier2_file_edit_target_block_not_found_fails() {
    let env = TestEnv::new();
    let file = env.create_file("code.py", "def foo():\n    return 42\n");

    let res = file_edit(&file, "def non_existent_function():", "def bar():");
    assert!(res.is_err(), "Target block not found must return TargetBlockNotFound");
    match res.unwrap_err() {
        FsError::TargetBlockNotFound(path) => {
            assert_eq!(path, file);
        }
        other => panic!("Expected FsError::TargetBlockNotFound, got: {other:?}"),
    }
}

#[test]
fn test_tier2_file_edit_ambiguous_target_block_fails() {
    let env = TestEnv::new();
    let content = "match\nother text\nmatch\nyet more\nmatch\n";
    let file = env.create_file("ambiguous.txt", content);

    let res = file_edit(&file, "match", "replacement");
    assert!(
        res.is_err(),
        "Ambiguous target block (>1 occurrence) must return AmbiguousTargetBlock"
    );
    match res.unwrap_err() {
        FsError::AmbiguousTargetBlock { path, count } => {
            assert_eq!(path, file);
            assert_eq!(count, 3);
        }
        other => panic!("Expected FsError::AmbiguousTargetBlock, got: {other:?}"),
    }

    // Verify file was not partially or erroneously modified
    let on_disk = env.read_file("ambiguous.txt");
    assert_eq!(on_disk, content);
}

#[test]
fn test_tier2_file_edit_target_block_in_non_existent_file() {
    let env = TestEnv::new();
    let missing = env.file_path("missing.txt");

    let res = file_edit(&missing, "target", "replacement");
    assert!(res.is_err());
    match res.unwrap_err() {
        FsError::FileNotFound(path) => {
            assert_eq!(path, missing);
        }
        other => panic!("Expected FsError::FileNotFound, got: {other:?}"),
    }
}
