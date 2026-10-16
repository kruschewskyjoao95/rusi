mod common;

use common::TestEnv;
use rusi::tools::fs::{file_edit, file_read, file_write, FsError};
use std::sync::Arc;
use std::thread;

// ============================================================================
// CHALLENGE SUITE: Milestone 1 - Filesystem Tools (M1-2)
// ============================================================================

// ----------------------------------------------------------------------------
// 1. Boundary Invariants on file_read
// ----------------------------------------------------------------------------

#[test]
fn test_challenge_read_single_line_with_newline() {
    let env = TestEnv::new();
    let file = env.create_file("single_nl.txt", "Single line with newline\n");

    // Default bounds (None, None)
    let res = file_read(&file, None, None).expect("read should succeed");
    assert_eq!(res, "1: Single line with newline\n");

    // Exact slice [1, 1]
    let res = file_read(&file, Some(1), Some(1)).expect("exact [1,1] should succeed");
    assert_eq!(res, "1: Single line with newline\n");

    // Slice extending past total lines [1, 50]
    let res = file_read(&file, Some(1), Some(50)).expect("slice past end should clamp");
    assert_eq!(res, "1: Single line with newline\n");

    // start_line = 0 normalized to 1
    let res = file_read(&file, Some(0), Some(1)).expect("start_line 0 should normalize");
    assert_eq!(res, "1: Single line with newline\n");

    // start_line > total_lines
    let err = file_read(&file, Some(2), Some(2)).unwrap_err();
    match err {
        FsError::LineOutOfRange { requested, total } => {
            assert_eq!(requested, 2);
            assert_eq!(total, 1);
        }
        other => panic!("Expected LineOutOfRange, got {:?}", other),
    }
}

#[test]
fn test_challenge_read_single_line_without_newline() {
    let env = TestEnv::new();
    let file = env.create_file("single_no_nl.txt", "Single line without newline");

    let res = file_read(&file, None, None).expect("read should succeed");
    assert_eq!(res, "1: Single line without newline\n");

    let res = file_read(&file, Some(1), Some(1)).expect("exact slice should succeed");
    assert_eq!(res, "1: Single line without newline\n");

    let res = file_read(&file, Some(1), Some(10)).expect("slice past end should clamp");
    assert_eq!(res, "1: Single line without newline\n");
}

#[test]
fn test_challenge_read_single_line_whitespace_and_tabs() {
    let env = TestEnv::new();
    let file = env.create_file("whitespace.txt", "  \t \t  \n");

    let res = file_read(&file, None, None).expect("read should succeed");
    assert_eq!(res, "1:   \t \t  \n");
}

#[test]
fn test_challenge_read_exact_bound_slices_on_multiline_file() {
    let env = TestEnv::new();
    let content = "line 1\nline 2\nline 3\nline 4\nline 5\n";
    let file = env.create_file("multiline.txt", content);

    // Exact single line 1: [1, 1]
    let res = file_read(&file, Some(1), Some(1)).expect("slice [1,1] failed");
    assert_eq!(res, "1: line 1\n");

    // Exact single line in middle: [3, 3]
    let res = file_read(&file, Some(3), Some(3)).expect("slice [3,3] failed");
    assert_eq!(res, "3: line 3\n");

    // Exact single line at end: [5, 5]
    let res = file_read(&file, Some(5), Some(5)).expect("slice [5,5] failed");
    assert_eq!(res, "5: line 5\n");

    // Exact full slice: [1, 5]
    let res = file_read(&file, Some(1), Some(5)).expect("slice [1,5] failed");
    assert_eq!(res, "1: line 1\n2: line 2\n3: line 3\n4: line 4\n5: line 5\n");

    // Exact sub-slice: [2, 4]
    let res = file_read(&file, Some(2), Some(4)).expect("slice [2,4] failed");
    assert_eq!(res, "2: line 2\n3: line 3\n4: line 4\n");
}

#[test]
fn test_challenge_read_slice_extending_past_total_lines() {
    let env = TestEnv::new();
    let content = "alpha\nbeta\ngamma\n";
    let file = env.create_file("three_lines.txt", content);

    // [1, 100] -> clamps to [1, 3]
    let res = file_read(&file, Some(1), Some(100)).expect("slice [1,100] failed");
    assert_eq!(res, "1: alpha\n2: beta\n3: gamma\n");

    // [2, 100] -> clamps to [2, 3]
    let res = file_read(&file, Some(2), Some(100)).expect("slice [2,100] failed");
    assert_eq!(res, "2: beta\n3: gamma\n");

    // [3, 100] -> clamps to [3, 3]
    let res = file_read(&file, Some(3), Some(100)).expect("slice [3,100] failed");
    assert_eq!(res, "3: gamma\n");

    // [1, usize::MAX] -> extreme clamp without overflow
    let res = file_read(&file, Some(1), Some(usize::MAX)).expect("slice with usize::MAX failed");
    assert_eq!(res, "1: alpha\n2: beta\n3: gamma\n");

    // Out of range start with extending end: [4, 100]
    let err = file_read(&file, Some(4), Some(100)).unwrap_err();
    match err {
        FsError::LineOutOfRange { requested, total } => {
            assert_eq!(requested, 4);
            assert_eq!(total, 3);
        }
        other => panic!("Expected LineOutOfRange, got {:?}", other),
    }
}

#[test]
fn test_challenge_read_crlf_line_endings() {
    let env = TestEnv::new();
    let content = "windows line 1\r\nwindows line 2\r\nwindows line 3\r\n";
    let file = env.create_file("crlf.txt", content);

    let res = file_read(&file, None, None).expect("reading CRLF file failed");
    // lines() strips \r\n, formatting adds \n
    assert_eq!(res, "1: windows line 1\n2: windows line 2\n3: windows line 3\n");
}

// ----------------------------------------------------------------------------
// 2. Overwrite Protection Integrity
// ----------------------------------------------------------------------------

#[test]
fn test_challenge_overwrite_protection_repeated_attempts() {
    let env = TestEnv::new();
    let file = env.create_file("protected.txt", "ORIGINAL_SAFE_CONTENT");

    // Attempt 1: overwrite=false must fail
    let err1 = file_write(&file, "MALICIOUS_OVERWRITE_1", false).unwrap_err();
    assert!(matches!(err1, FsError::FileAlreadyExists(_)));
    assert_eq!(env.read_file("protected.txt"), "ORIGINAL_SAFE_CONTENT");

    // Attempt 2: overwrite=false must fail again
    let err2 = file_write(&file, "MALICIOUS_OVERWRITE_2", false).unwrap_err();
    assert!(matches!(err2, FsError::FileAlreadyExists(_)));
    assert_eq!(env.read_file("protected.txt"), "ORIGINAL_SAFE_CONTENT");

    // Attempt 3: overwrite=false must fail again
    let err3 = file_write(&file, "MALICIOUS_OVERWRITE_3", false).unwrap_err();
    assert!(matches!(err3, FsError::FileAlreadyExists(_)));
    assert_eq!(env.read_file("protected.txt"), "ORIGINAL_SAFE_CONTENT");

    // Legitimate overwrite=true succeeds
    let ok = file_write(&file, "LEGITIMATE_UPDATE", true);
    assert!(ok.is_ok());
    assert_eq!(env.read_file("protected.txt"), "LEGITIMATE_UPDATE");

    // Post-overwrite attempt with overwrite=false must fail
    let err4 = file_write(&file, "MALICIOUS_OVERWRITE_4", false).unwrap_err();
    assert!(matches!(err4, FsError::FileAlreadyExists(_)));
    assert_eq!(env.read_file("protected.txt"), "LEGITIMATE_UPDATE");
}

// ----------------------------------------------------------------------------
// 3. Repeated Edit Sequences
// ----------------------------------------------------------------------------

#[test]
fn test_challenge_repeated_edit_pipeline() {
    let env = TestEnv::new();
    let initial = "fn stage_1() { todo!() }\nfn stage_2() { todo!() }\nfn stage_3() { todo!() }\n";
    let file = env.create_file("pipeline.rs", initial);

    // Edit 1
    file_edit(&file, "fn stage_1() { todo!() }", "fn stage_1() -> i32 { 10 }")
        .expect("edit 1 failed");

    // Edit 2
    file_edit(&file, "fn stage_2() { todo!() }", "fn stage_2() -> i32 { 20 }")
        .expect("edit 2 failed");

    // Edit 3
    file_edit(&file, "fn stage_3() { todo!() }", "fn stage_3() -> i32 { 30 }")
        .expect("edit 3 failed");

    let final_content = env.read_file("pipeline.rs");
    let expected = "fn stage_1() -> i32 { 10 }\nfn stage_2() -> i32 { 20 }\nfn stage_3() -> i32 { 30 }\n";
    assert_eq!(final_content, expected);

    // Reading back with file_read to check line numbers
    let read_back = file_read(&file, None, None).expect("file_read after edits failed");
    assert_eq!(
        read_back,
        "1: fn stage_1() -> i32 { 10 }\n2: fn stage_2() -> i32 { 20 }\n3: fn stage_3() -> i32 { 30 }\n"
    );
}

#[test]
fn test_challenge_edit_expand_and_shrink_lines() {
    let env = TestEnv::new();
    let initial = "START\nPLACEHOLDER\nEND\n";
    let file = env.create_file("expand_shrink.txt", initial);

    // Expand 1 line into 5 lines
    let expansion = "BLOCK_1\nBLOCK_2\nBLOCK_3\nBLOCK_4\nBLOCK_5";
    file_edit(&file, "PLACEHOLDER", expansion).expect("expansion failed");

    let read_expanded = file_read(&file, None, None).expect("read expanded failed");
    assert_eq!(
        read_expanded,
        "1: START\n2: BLOCK_1\n3: BLOCK_2\n4: BLOCK_3\n5: BLOCK_4\n6: BLOCK_5\n7: END\n"
    );

    // Shrink middle 3 blocks into 1 line
    let shrink_target = "BLOCK_2\nBLOCK_3\nBLOCK_4";
    file_edit(&file, shrink_target, "SHRUNK_BLOCK").expect("shrink failed");

    let read_shrunk = file_read(&file, None, None).expect("read shrunk failed");
    assert_eq!(
        read_shrunk,
        "1: START\n2: BLOCK_1\n3: SHRUNK_BLOCK\n4: BLOCK_5\n5: END\n"
    );
}

#[test]
fn test_challenge_edit_delete_block_with_empty_replacement() {
    let env = TestEnv::new();
    let initial = "header\nREMOVE_ME\nfooter\n";
    let file = env.create_file("delete_test.txt", initial);

    file_edit(&file, "REMOVE_ME\n", "").expect("deletion of block failed");
    let updated = env.read_file("delete_test.txt");
    assert_eq!(updated, "header\nfooter\n");
}

#[test]
fn test_challenge_edit_idempotent_replacement() {
    let env = TestEnv::new();
    let initial = "stable content\n";
    let file = env.create_file("idempotent.txt", initial);

    file_edit(&file, "stable content", "stable content").expect("idempotent edit failed");
    let updated = env.read_file("idempotent.txt");
    assert_eq!(updated, initial);
}

// ----------------------------------------------------------------------------
// 4. Concurrency & Stress Testing
// ----------------------------------------------------------------------------

#[test]
fn test_challenge_concurrent_reads_on_shared_file() {
    let env = Arc::new(TestEnv::new());
    let content = (1..=50)
        .map(|i| format!("Line #{i} in the shared file"))
        .collect::<Vec<_>>()
        .join("\n");
    let file = env.create_file("shared_read.txt", &content);

    let mut handles = Vec::new();
    for thread_idx in 0..10 {
        let file_clone = file.clone();
        let handle = thread::spawn(move || {
            for i in 1..=10 {
                let start = (thread_idx * 2 + i) % 40 + 1;
                let end = start + 5;
                let res = file_read(&file_clone, Some(start), Some(end));
                assert!(res.is_ok(), "Thread read at [{start}, {end}] must succeed");
                let text = res.unwrap();
                assert!(text.contains(&format!("{start}: Line #{start}")));
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().expect("Concurrent read thread panicked");
    }
}

#[test]
fn test_challenge_concurrent_independent_writes_and_edits() {
    let env = Arc::new(TestEnv::new());

    let mut handles = Vec::new();
    for thread_id in 0..10 {
        let env_clone = Arc::clone(&env);
        let handle = thread::spawn(move || {
            let fname = format!("thread_file_{thread_id}.txt");
            let target_path = env_clone.file_path(&fname);

            // 1. Initial write
            let initial = format!("Header {thread_id}\nTarget {thread_id}\nFooter {thread_id}\n");
            file_write(&target_path, &initial, false).expect("initial write failed");

            // 2. Overwrite protection check
            let fail_res = file_write(&target_path, "collision", false);
            assert!(fail_res.is_err(), "must guard against accidental overwrite");

            // 3. Edit unique block
            file_edit(
                &target_path,
                &format!("Target {thread_id}"),
                &format!("Replaced {thread_id}"),
            )
            .expect("edit failed");

            // 4. Read back and verify
            let read_back = file_read(&target_path, None, None).expect("read failed");
            assert!(read_back.contains(&format!("Replaced {thread_id}")));
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().expect("Concurrent writer thread panicked");
    }
}

// ----------------------------------------------------------------------------
// 5. Error Messages Accuracy and Descriptiveness
// ----------------------------------------------------------------------------

#[test]
fn test_challenge_error_messages_formatting() {
    let env = TestEnv::new();
    let missing_path = env.file_path("does_not_exist.txt");

    // 1. FileNotFound error format
    let err = file_read(&missing_path, None, None).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.starts_with("File not found: "), "Msg was: {msg}");
    assert!(msg.contains("does_not_exist.txt"));

    // 2. FileAlreadyExists error format
    let existing = env.create_file("exists.txt", "data");
    let err = file_write(&existing, "new", false).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("File already exists: '") && msg.contains("Pass overwrite=true to overwrite."),
        "Msg was: {msg}"
    );

    // 3. InvalidLineRange error format
    let multi_file = env.create_file("multi.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
    let err = file_read(&multi_file, Some(5), Some(2)).unwrap_err();
    let msg = format!("{err}");
    assert_eq!(msg, "Invalid line range: start_line (5) > end_line (2)");

    // 4. LineOutOfRange error format
    let err = file_read(&existing, Some(10), None).unwrap_err();
    let msg = format!("{err}");
    assert_eq!(
        msg,
        "Line out of range: requested line 10, but file only has 1 lines"
    );

    // 5. TargetBlockNotFound error format
    let err = file_edit(&existing, "NON_EXISTENT_PATTERN", "REPLACEMENT").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.starts_with("Target block not found in file: '") && msg.contains("exists.txt"),
        "Msg was: {msg}"
    );

    // 6. AmbiguousTargetBlock error format
    let ambig_file = env.create_file("ambig.txt", "TAG TAG TAG");
    let err = file_edit(&ambig_file, "TAG", "REPLACE").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("Ambiguous target block in file '") && msg.contains("found 3 occurrences"),
        "Msg was: {msg}"
    );

    // 7. EmptyTargetBlock error format
    let err = file_edit(&ambig_file, "", "REPLACE").unwrap_err();
    let msg = format!("{err}");
    assert_eq!(msg, "Target block cannot be empty");
}

// ----------------------------------------------------------------------------
// 6. Advanced Edge Cases: Regex Metacharacters, Large Files, Empty Writes
// ----------------------------------------------------------------------------

#[test]
fn test_challenge_edit_with_regex_metacharacters() {
    let env = TestEnv::new();
    let content = "Pattern: ^[a-z]+.*$ (matched)\nOther: [0-9]+\n";
    let file = env.create_file("regex_meta.txt", content);

    // Ensure regex special chars like ^, $, +, *, [, ] are treated as exact literal strings
    let target = "^[a-z]+.*$ (matched)";
    let replacement = "MODIFIED_LITERAL";
    file_edit(&file, target, replacement).expect("edit with regex special chars must succeed");

    let updated = env.read_file("regex_meta.txt");
    assert_eq!(updated, "Pattern: MODIFIED_LITERAL\nOther: [0-9]+\n");
}

#[test]
fn test_challenge_edit_overlapping_target_matches() {
    let env = TestEnv::new();
    // "aaaa" contains "aa" twice (non-overlapping in Rust str::matches)
    let file = env.create_file("overlap.txt", "aaaa");

    let err = file_edit(&file, "aa", "b").unwrap_err();
    match err {
        FsError::AmbiguousTargetBlock { count, .. } => {
            assert_eq!(count, 2);
        }
        other => panic!("Expected AmbiguousTargetBlock with count 2, got {:?}", other),
    }
}

#[test]
fn test_challenge_read_and_write_empty_content_lifecycle() {
    let env = TestEnv::new();
    let file = env.file_path("zero_byte.txt");

    // 1. Write empty file
    file_write(&file, "", false).expect("writing empty file should succeed");
    assert!(file.exists());

    // 2. Read empty file with default bounds
    let read_res = file_read(&file, None, None).expect("reading empty file should succeed");
    assert_eq!(read_res, "");

    // 3. Read empty file with explicit bounds [1, 10]
    let read_bounds = file_read(&file, Some(1), Some(10)).expect("reading empty file with bounds should succeed");
    assert_eq!(read_bounds, "");

    // 4. Overwrite guard prevents accidental write to empty file
    let err = file_write(&file, "new data", false).unwrap_err();
    assert!(matches!(err, FsError::FileAlreadyExists(_)));

    // 5. Overwrite=true succeeds
    file_write(&file, "new data", true).expect("overwrite=true on empty file should succeed");
    assert_eq!(env.read_file("zero_byte.txt"), "new data");
}

#[test]
fn test_challenge_read_file_with_multiple_consecutive_blank_lines() {
    let env = TestEnv::new();
    // 3 blank lines
    let file = env.create_file("blank_lines.txt", "\n\n\n");

    let res = file_read(&file, None, None).expect("reading blank lines should succeed");
    assert_eq!(res, "1: \n2: \n3: \n");

    let slice_res = file_read(&file, Some(2), Some(3)).expect("reading blank slice should succeed");
    assert_eq!(slice_res, "2: \n3: \n");
}

#[test]
fn test_challenge_large_file_scale_and_slice() {
    let env = TestEnv::new();
    let line_count = 50_000;
    let mut large_content = String::with_capacity(line_count * 20);
    for i in 1..=line_count {
        use std::fmt::Write;
        let _ = writeln!(large_content, "Record {i:06} data payload");
    }
    let file = env.create_file("large.txt", &large_content);

    // Read small slice near the end: lines 49,990 to 49,995
    let res = file_read(&file, Some(49990), Some(49995)).expect("large file slice should succeed");
    let expected = "\
49990: Record 049990 data payload
49991: Record 049991 data payload
49992: Record 049992 data payload
49993: Record 049993 data payload
49994: Record 049994 data payload
49995: Record 049995 data payload
";
    assert_eq!(res, expected);

    // Out of range check on large file
    let err = file_read(&file, Some(50001), None).unwrap_err();
    assert!(matches!(err, FsError::LineOutOfRange { requested: 50001, total: 50000 }));
}

