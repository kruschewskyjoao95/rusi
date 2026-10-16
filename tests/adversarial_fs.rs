mod common;

use common::TestEnv;
use rusi::tools::fs::{file_edit, file_read, file_write, FsError};

// ============================================================================
// ADVERSARIAL CATEGORY 1: Multiline & Block Replacement Stress
// ============================================================================

#[test]
fn test_stress_multiline_with_mixed_indentation_and_blank_lines() {
    let env = TestEnv::new();
    let original = "header\n\n\t// Section 1\n    let x = 1;\n\n\t// Section 2\n    let y = 2;\n\nfooter";
    let file = env.create_file("indent.txt", original);

    let target = "\t// Section 1\n    let x = 1;\n\n\t// Section 2\n    let y = 2;";
    let replacement = "    // Modified\n    let z = x + y;";

    file_edit(&file, target, replacement).expect("multiline replacement with mixed whitespace failed");

    let updated = env.read_file("indent.txt");
    let expected = "header\n\n    // Modified\n    let z = x + y;\n\nfooter";
    assert_eq!(updated, expected);
}

#[test]
fn test_stress_edit_delete_block_with_empty_replacement() {
    let env = TestEnv::new();
    let original = "line1\nREMOVE_ME_START\nto be deleted\nREMOVE_ME_END\nline2";
    let file = env.create_file("delete.txt", original);

    let target = "REMOVE_ME_START\nto be deleted\nREMOVE_ME_END\n";
    file_edit(&file, target, "").expect("replacement with empty string (deletion) should succeed");

    let updated = env.read_file("delete.txt");
    assert_eq!(updated, "line1\nline2");
}

#[test]
fn test_stress_edit_entire_file_content() {
    let env = TestEnv::new();
    let original = "entire file content to be fully replaced";
    let file = env.create_file("full.txt", original);

    file_edit(&file, original, "completely new file content").expect("replacing entire file content should succeed");
    assert_eq!(env.read_file("full.txt"), "completely new file content");
}

#[test]
fn test_stress_edit_at_start_of_file() {
    let env = TestEnv::new();
    let original = "PREFIX_TO_REPLACErest of file";
    let file = env.create_file("prefix.txt", original);

    file_edit(&file, "PREFIX_TO_REPLACE", "NEW_PREFIX_").expect("prefix replacement failed");
    assert_eq!(env.read_file("prefix.txt"), "NEW_PREFIX_rest of file");
}

#[test]
fn test_stress_edit_at_end_of_file() {
    let env = TestEnv::new();
    let original = "beginning of fileSUFFIX_TO_REPLACE";
    let file = env.create_file("suffix.txt", original);

    file_edit(&file, "SUFFIX_TO_REPLACE", "_NEW_SUFFIX").expect("suffix replacement failed");
    assert_eq!(env.read_file("suffix.txt"), "beginning of file_NEW_SUFFIX");
}

#[test]
fn test_stress_edit_single_line_to_huge_multiline_block() {
    let env = TestEnv::new();
    let original = "start\nPLACEHOLDER\nend";
    let file = env.create_file("expand.txt", original);

    // Expand into 500 lines
    let huge_block: String = (0..500)
        .map(|i| format!("generated line {i}"))
        .collect::<Vec<_>>()
        .join("\n");

    file_edit(&file, "PLACEHOLDER", &huge_block).expect("expanding single line to huge block failed");

    let updated = env.read_file("expand.txt");
    let lines: Vec<&str> = updated.lines().collect();
    assert_eq!(lines.len(), 502);
    assert_eq!(lines[0], "start");
    assert_eq!(lines[1], "generated line 0");
    assert_eq!(lines[500], "generated line 499");
    assert_eq!(lines[501], "end");
}

// ============================================================================
// ADVERSARIAL CATEGORY 2: Ambiguous Matches & Overlapping Substrings
// ============================================================================

#[test]
fn test_stress_ambiguous_matches_large_count() {
    let env = TestEnv::new();
    // 50 occurrences of TOKEN
    let content = "prefix ".to_string() + &"TOKEN in between ".repeat(50) + "suffix";
    let file = env.create_file("repeat.txt", &content);

    let err = file_edit(&file, "TOKEN", "REPLACED").unwrap_err();
    match err {
        FsError::AmbiguousTargetBlock { count, path } => {
            assert_eq!(count, 50);
            assert_eq!(path, file);
        }
        other => panic!("Expected AmbiguousTargetBlock with count 50, got: {other:?}"),
    }

    // Verify content was untouched
    assert_eq!(env.read_file("repeat.txt"), content);
}

#[test]
fn test_stress_ambiguous_overlapping_subpatterns() {
    let env = TestEnv::new();
    // "aaaa" contains "aa" at index 0 and index 2 (non-overlapping) -> count 2
    let file = env.create_file("overlap.txt", "aaaa");

    let err = file_edit(&file, "aa", "b").unwrap_err();
    match err {
        FsError::AmbiguousTargetBlock { count, .. } => {
            assert_eq!(count, 2);
        }
        other => panic!("Expected AmbiguousTargetBlock with count 2, got: {other:?}"),
    }
}

#[test]
fn test_stress_target_block_as_substring_of_surrounding_tokens() {
    let env = TestEnv::new();
    // "cat" is inside "caterpillar", "concatenate", and standalone "cat"
    // Total occurrences of "cat" should be 3 -> Ambiguous!
    let file = env.create_file("substr.txt", "the caterpillar will concatenate with the cat");

    let err = file_edit(&file, "cat", "feline").unwrap_err();
    match err {
        FsError::AmbiguousTargetBlock { count, .. } => {
            assert_eq!(count, 3);
        }
        other => panic!("Expected AmbiguousTargetBlock with count 3, got: {other:?}"),
    }

    // But if we qualify the target block with surrounding context, it is unique and succeeds
    file_edit(&file, "with the cat", "with the feline").expect("disambiguated edit should succeed");
    assert_eq!(
        env.read_file("substr.txt"),
        "the caterpillar will concatenate with the feline"
    );
}

#[test]
fn test_stress_regex_metacharacters_treated_as_literals() {
    let env = TestEnv::new();
    // File containing regex special characters: .*+?^${}()|[]\
    let content = r#"let re = regex::Regex::new(r"^[a-z0-9_.+-]+@[a-z0-9-]+\.[a-z0-9-.]+$");"#;
    let file = env.create_file("regex.txt", content);

    // Edit using regex metacharacters in target block
    let target = r#"r"^[a-z0-9_.+-]+@[a-z0-9-]+\.[a-z0-9-.]+$""#;
    let replacement = r#"r"^[a-zA-Z0-9]+$""#;

    file_edit(&file, target, replacement).expect("target block with regex metacharacters should match literally");
    let updated = env.read_file("regex.txt");
    assert_eq!(
        updated,
        r#"let re = regex::Regex::new(r"^[a-zA-Z0-9]+$");"#
    );
}

// ============================================================================
// ADVERSARIAL CATEGORY 3: Special Characters & Unicode
// ============================================================================

#[test]
fn test_stress_unicode_multibyte_cjk_emoji_and_zwj() {
    let env = TestEnv::new();
    let content = "1. 日本語のテキスト\n2. 🦀 Rust 🚀 Rocket\n3. Family: 👨‍👩‍👧‍👦 emoji zwj\n4. Cyrillic: Привет мир\n5. Math: ∀x ∈ ℝ, x² ≥ 0\n";
    let file = env.create_file("unicode.txt", content);

    // Test file_read preserving UTF-8 multi-byte characters and line numbers
    let read_slice = file_read(&file, Some(2), Some(4)).expect("file_read on unicode should succeed");
    let expected_read = "2: 2. 🦀 Rust 🚀 Rocket\n3: 3. Family: 👨‍👩‍👧‍👦 emoji zwj\n4: 4. Cyrillic: Привет мир\n";
    assert_eq!(read_slice, expected_read);

    // Test file_edit on multi-byte emoji line
    file_edit(&file, "🦀 Rust 🚀 Rocket", "🦀 Rusi 🛡️ Shield").expect("file_edit on unicode should succeed");
    let updated = env.read_file("unicode.txt");
    assert!(updated.contains("2. 🦀 Rusi 🛡️ Shield\n"));
    assert!(updated.contains("3. Family: 👨‍👩‍👧‍👦 emoji zwj\n"));

    // Test file_edit on math symbols
    file_edit(&file, "∀x ∈ ℝ, x² ≥ 0", "∃x ∈ ℂ, x² = -1").expect("math unicode edit failed");
    let updated2 = env.read_file("unicode.txt");
    assert!(updated2.contains("5. Math: ∃x ∈ ℂ, x² = -1\n"));
}

#[test]
fn test_stress_escapes_and_control_characters() {
    let env = TestEnv::new();
    // Null byte and tab characters in content
    let content = "line with \t tabs \t\nline with quote \" and backslash \\\n";
    let file = env.create_file("escapes.txt", content);

    file_edit(&file, "\" and backslash \\", "' and forward slash /").expect("editing quotes and slashes failed");
    let updated = env.read_file("escapes.txt");
    assert_eq!(updated, "line with \t tabs \t\nline with quote ' and forward slash /\n");
}

// ============================================================================
// ADVERSARIAL CATEGORY 4: Deeply Nested Parent Directories
// ============================================================================

#[test]
fn test_stress_deeply_nested_directories_50_levels() {
    let env = TestEnv::new();
    let mut deep_path = env.path().to_path_buf();
    for i in 0..50 {
        deep_path = deep_path.join(format!("level_{i:03}"));
    }
    let target_file = deep_path.join("deep_test.txt");

    // Write file in 50-level deep non-existent path
    file_write(&target_file, "deep content payload", false).expect("writing to 50 levels of directories should succeed");

    assert!(target_file.exists());
    let read_back = std::fs::read_to_string(&target_file).unwrap();
    assert_eq!(read_back, "deep content payload");

    // Attempting overwrite with overwrite=false should fail
    let err = file_write(&target_file, "new payload", false).unwrap_err();
    assert!(matches!(err, FsError::FileAlreadyExists(_)));

    // Overwrite with overwrite=true should succeed
    file_write(&target_file, "overwritten payload", true).expect("overwrite=true on deep path should succeed");
    assert_eq!(std::fs::read_to_string(&target_file).unwrap(), "overwritten payload");
}

#[test]
fn test_stress_nested_directory_with_spaces_and_unicode() {
    let env = TestEnv::new();
    let path = env.file_path("sub dir with spaces/プロジェクト/📁 assets/config.yaml");

    file_write(&path, "key: value\n", false).expect("writing to unicode & spaced directory should succeed");
    assert!(path.exists());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "key: value\n");
}

// ============================================================================
// ADVERSARIAL CATEGORY 5: Zero-Byte Files & Empty Boundaries
// ============================================================================

#[test]
fn test_stress_zero_byte_file_lifecycle() {
    let env = TestEnv::new();
    let file = env.file_path("empty.txt");

    // 1. Create 0-byte file using file_write
    file_write(&file, "", false).expect("file_write with empty string should create 0-byte file");
    assert!(file.exists());
    assert_eq!(std::fs::metadata(&file).unwrap().len(), 0);

    // 2. Overwrite guard prevents accidental overwrite of 0-byte file
    let err = file_write(&file, "new data", false).unwrap_err();
    assert!(matches!(err, FsError::FileAlreadyExists(_)));

    // 3. file_read on 0-byte file returns empty string
    let read_res = file_read(&file, None, None).expect("file_read on 0-byte file should return Ok");
    assert_eq!(read_res, "");

    let read_bounded = file_read(&file, Some(1), Some(10)).expect("file_read with bounds on 0-byte file should return Ok");
    assert_eq!(read_bounded, "");

    // 4. file_edit with empty target fails with EmptyTargetBlock
    let err_empty = file_edit(&file, "", "replacement").unwrap_err();
    assert!(matches!(err_empty, FsError::EmptyTargetBlock));

    // 5. file_edit on 0-byte file with non-empty target fails with TargetBlockNotFound
    let err_not_found = file_edit(&file, "non-existent", "replacement").unwrap_err();
    assert!(matches!(err_not_found, FsError::TargetBlockNotFound(_)));
}

// ============================================================================
// ADVERSARIAL CATEGORY 6: CRLF vs LF Line Endings & Formatting Invariants
// ============================================================================

#[test]
fn test_stress_crlf_line_endings_read() {
    let env = TestEnv::new();
    // Windows CRLF line endings
    let crlf_content = "line one\r\nline two\r\nline three\r\n";
    let file = env.create_file("windows.txt", crlf_content);

    // file_read should normalize lines and not have trailing \r in line content
    let output = file_read(&file, Some(1), Some(2)).expect("reading CRLF file should succeed");
    let expected = "1: line one\n2: line two\n";
    assert_eq!(output, expected);
}

#[test]
fn test_stress_crlf_exact_block_edit() {
    let env = TestEnv::new();
    let crlf_content = "start\r\nmiddle block\r\nend\r\n";
    let file = env.create_file("crlf_edit.txt", crlf_content);

    // Exact match with \r\n
    file_edit(&file, "middle block\r\n", "replaced block\r\n").expect("crlf exact edit should succeed");

    let updated = env.read_file("crlf_edit.txt");
    assert_eq!(updated, "start\r\nreplaced block\r\nend\r\n");
}

#[test]
fn test_stress_mixed_line_endings() {
    let env = TestEnv::new();
    let mixed = "lf line\ncrlf line\r\ncr only line\rlast line";
    let file = env.create_file("mixed.txt", mixed);

    // str::lines() in Rust handles \n and \r\n, though bare \r is preserved as part of line
    let read_all = file_read(&file, None, None).expect("reading mixed endings should succeed");
    assert!(read_all.starts_with("1: lf line\n2: crlf line\n"));
}

// ============================================================================
// ADVERSARIAL CATEGORY 7: Sequential Chained Edits Stress
// ============================================================================

#[test]
fn test_stress_sequential_chained_edits_50_iterations() {
    let env = TestEnv::new();
    let file = env.create_file("chain.txt", "v0");

    for i in 0..50 {
        let from = format!("v{i}");
        let to = format!("v{}", i + 1);
        file_edit(&file, &from, &to).unwrap_or_else(|e| panic!("Step {i} failed: {e:?}"));
    }

    assert_eq!(env.read_file("chain.txt"), "v50");
}

// ============================================================================
// ADVERSARIAL CATEGORY 8: Large File & Slice Boundary Stress
// ============================================================================

#[test]
fn test_stress_large_file_line_reading() {
    let env = TestEnv::new();
    // 50,000 lines
    let mut large_content = String::with_capacity(1_500_000);
    for i in 1..=50_000 {
        use std::fmt::Write;
        writeln!(&mut large_content, "record #{i} data").unwrap();
    }
    let file = env.create_file("large.txt", &large_content);

    // Read a 10-line slice from line 49,995 to 50,000
    let slice = file_read(&file, Some(49_995), Some(50_000)).expect("reading end slice of 50k lines failed");
    let expected = "49995: record #49995 data\n\
                    49996: record #49996 data\n\
                    49997: record #49997 data\n\
                    49998: record #49998 data\n\
                    49999: record #49999 data\n\
                    50000: record #50000 data\n";
    assert_eq!(slice, expected);

    // Out of range check
    let err = file_read(&file, Some(50_001), None).unwrap_err();
    match err {
        FsError::LineOutOfRange { requested, total } => {
            assert_eq!(requested, 50_001);
            assert_eq!(total, 50_000);
        }
        other => panic!("Expected LineOutOfRange, got: {other:?}"),
    }
}

// ============================================================================
// ADVERSARIAL CATEGORY 9: File Type, Perms, and Single Line Boundaries
// ============================================================================

#[test]
fn test_stress_single_line_without_trailing_newline() {
    let env = TestEnv::new();
    let file = env.create_file("no_newline.txt", "lone line without newline");

    let read_all = file_read(&file, None, None).expect("single line without newline read failed");
    assert_eq!(read_all, "1: lone line without newline\n");

    let read_bounded = file_read(&file, Some(1), Some(1)).expect("bounded slice read failed");
    assert_eq!(read_bounded, "1: lone line without newline\n");

    let read_clamp = file_read(&file, Some(1), Some(5)).expect("clamped slice read failed");
    assert_eq!(read_clamp, "1: lone line without newline\n");

    let err_oor = file_read(&file, Some(2), None).unwrap_err();
    assert!(matches!(err_oor, FsError::LineOutOfRange { requested: 2, total: 1 }));
}

#[test]
fn test_stress_newline_only_file() {
    let env = TestEnv::new();
    // 3 empty lines created by 3 newlines
    let file = env.create_file("newlines.txt", "\n\n\n");

    let read_all = file_read(&file, None, None).expect("read newlines file failed");
    assert_eq!(read_all, "1: \n2: \n3: \n");
}

#[test]
fn test_stress_target_is_directory_error_handling() {
    let env = TestEnv::new();
    let dir_path = env.file_path("some_dir");
    std::fs::create_dir_all(&dir_path).unwrap();

    // file_read on directory returns Io error
    let read_err = file_read(&dir_path, None, None).unwrap_err();
    assert!(matches!(read_err, FsError::Io(_)));

    // file_write on existing directory returns Io error (or FileAlreadyExists if overwrite=false)
    let write_err1 = file_write(&dir_path, "payload", false).unwrap_err();
    assert!(matches!(write_err1, FsError::FileAlreadyExists(_)));

    let write_err2 = file_write(&dir_path, "payload", true).unwrap_err();
    assert!(matches!(write_err2, FsError::Io(_)));

    // file_edit on directory returns Io error
    let edit_err = file_edit(&dir_path, "target", "rep").unwrap_err();
    assert!(matches!(edit_err, FsError::Io(_)));
}

#[test]
fn test_stress_identical_target_and_replacement() {
    let env = TestEnv::new();
    let file = env.create_file("ident.txt", "hello world");

    file_edit(&file, "world", "world").expect("identical target and replacement should succeed");
    assert_eq!(env.read_file("ident.txt"), "hello world");
}

#[test]
fn test_stress_overlapping_occurrences_banana_case() {
    let env = TestEnv::new();
    // In "banana", "ana" starts at index 1 and index 3
    // Under standard non-overlapping semantics, it matches 1 time
    let file = env.create_file("banana.txt", "banana");

    file_edit(&file, "ana", "XYZ").expect("non-overlapping match should succeed");
    assert_eq!(env.read_file("banana.txt"), "bXYZna");
}

