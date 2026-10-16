#![allow(unexpected_cfgs)]
#![cfg(milestone_3)]

mod common;

use common::TestEnv;
use rusi::tools::search::{code_search, format_search_results, SearchError, SearchMatch};

// ============================================================================
// Tier 1: Feature Coverage (Representative Happy-Path Inputs)
// ============================================================================

#[test]
fn test_tier1_code_search_literal_pattern() {
    let env = TestEnv::new();
    env.create_file(
        "src/lib.rs",
        "// Copyright\npub fn calculate() -> u32 {\n    let magic_token = 42;\n    magic_token\n}\n",
    );
    env.create_file("README.md", "# Documentation\nSome docs here.\n");

    let matches = code_search("magic_token", env.path()).expect("code_search should succeed");
    assert_eq!(matches.len(), 2, "Expected 2 occurrences of magic_token in src/lib.rs");

    assert!(matches[0].file.contains("src/lib.rs"));
    assert_eq!(matches[0].line_number, 3);
    assert!(matches[0].line_content.contains("let magic_token = 42;"));

    assert_eq!(matches[1].line_number, 4);
    assert!(matches[1].line_content.contains("magic_token"));
}

#[test]
fn test_tier1_code_search_regex_identifier() {
    let env = TestEnv::new();
    env.create_file(
        "module_a.rs",
        "fn helper_one() {}\npub fn exported_alpha() {}\n",
    );
    env.create_file(
        "module_b.rs",
        "pub fn exported_beta() {}\nfn private_two() {}\n",
    );

    let pattern = r"pub fn exported_[a-z]+\(\)";
    let matches = code_search(pattern, env.path()).expect("Regex code_search should succeed");

    assert_eq!(matches.len(), 2);
    let contents: Vec<&str> = matches.iter().map(|m| m.line_content.trim()).collect();
    assert!(contents.contains(&"pub fn exported_alpha() {}"));
    assert!(contents.contains(&"pub fn exported_beta() {}"));
}

#[test]
fn test_tier1_code_search_multiple_matches_across_multiple_files() {
    let env = TestEnv::new();
    env.create_file("service1.rs", "const TIMEOUT_MS: u64 = 5000;\n");
    env.create_file("service2.rs", "const TIMEOUT_MS: u64 = 10000;\n");
    env.create_file("service3.rs", "const TIMEOUT_MS: u64 = 3000;\n");
    env.create_file("other.rs", "const OTHER_VAL: u64 = 1;\n");

    let matches = code_search("TIMEOUT_MS", env.path()).expect("code_search should succeed");
    assert_eq!(matches.len(), 3);

    let files: Vec<&str> = matches.iter().map(|m| m.file.as_str()).collect();
    assert!(files.iter().any(|f| f.ends_with("service1.rs")));
    assert!(files.iter().any(|f| f.ends_with("service2.rs")));
    assert!(files.iter().any(|f| f.ends_with("service3.rs")));
}

#[test]
fn test_tier1_code_search_scoped_subpath() {
    let env = TestEnv::new();
    env.create_file("frontend/src/app.js", "const API_KEY = 'secret';\n");
    env.create_file("backend/src/server.js", "const API_KEY = 'secret';\n");

    let subpath = env.file_path("frontend");
    let matches = code_search("API_KEY", &subpath).expect("Scoped code_search should succeed");

    assert_eq!(matches.len(), 1);
    assert!(matches[0].file.contains("frontend/src/app.js"));
    assert!(!matches[0].file.contains("backend"));
}

#[test]
fn test_tier1_format_search_results() {
    let matches = vec![
        SearchMatch {
            file: "src/main.rs".to_string(),
            line_number: 10,
            line_content: "println!(\"hello\");".to_string(),
        },
        SearchMatch {
            file: "src/tools.rs".to_string(),
            line_number: 42,
            line_content: "pub fn run() {}".to_string(),
        },
    ];

    let formatted = format_search_results(&matches);
    assert!(formatted.contains("src/main.rs"));
    assert!(formatted.contains("10"));
    assert!(formatted.contains("println!(\"hello\");"));
    assert!(formatted.contains("src/tools.rs"));
    assert!(formatted.contains("42"));
    assert!(formatted.contains("pub fn run() {}"));
}

#[test]
fn test_tier1_code_search_case_sensitive() {
    let env = TestEnv::new();
    env.create_file("case.txt", "lowercase_token\nUPPERCASE_TOKEN\n");

    let matches_lower = code_search("lowercase_token", env.path()).unwrap();
    assert_eq!(matches_lower.len(), 1);
    assert_eq!(matches_lower[0].line_number, 1);

    let matches_upper = code_search("UPPERCASE_TOKEN", env.path()).unwrap();
    assert_eq!(matches_upper.len(), 1);
    assert_eq!(matches_upper[0].line_number, 2);
}

// ============================================================================
// Tier 2: Boundary & Corner Cases (Edge Conditions & Failure Modes)
// ============================================================================

#[test]
fn test_tier2_code_search_respects_gitignore() {
    let env = TestEnv::new();
    env.create_file(".gitignore", "*.log\nignored_dir/\nsecret.env\n");
    env.create_file("app.rs", "let token = \"SEARCHABLE_MARKER\";\n");
    env.create_file("test.log", "debug log with SEARCHABLE_MARKER\n");
    env.create_file("ignored_dir/data.txt", "data with SEARCHABLE_MARKER\n");
    env.create_file("secret.env", "KEY=SEARCHABLE_MARKER\n");

    let matches =
        code_search("SEARCHABLE_MARKER", env.path()).expect("code_search should respect gitignore");

    // Only app.rs must be returned! All .gitignore rules must be respected.
    assert_eq!(
        matches.len(),
        1,
        "code_search must exclude files matching .gitignore patterns"
    );
    assert!(matches[0].file.ends_with("app.rs"));
}

#[test]
fn test_tier2_code_search_ignores_hidden_files_and_directories() {
    let env = TestEnv::new();
    env.create_file(".git/config", "HIDDEN_SEARCH_TOKEN = true\n");
    env.create_file(".hidden_folder/file.rs", "let x = \"HIDDEN_SEARCH_TOKEN\";\n");
    env.create_file(".secret_file", "HIDDEN_SEARCH_TOKEN\n");
    env.create_file("visible.rs", "let y = \"HIDDEN_SEARCH_TOKEN\";\n");

    let matches = code_search("HIDDEN_SEARCH_TOKEN", env.path())
        .expect("code_search should ignore hidden files");

    assert_eq!(
        matches.len(),
        1,
        "Hidden files and hidden directories must be ignored by default"
    );
    assert!(matches[0].file.ends_with("visible.rs"));
}

#[test]
fn test_tier2_code_search_invalid_regex_syntax() {
    let env = TestEnv::new();
    env.create_file("test.txt", "hello world\n");

    let invalid_pattern = "([unclosed_parenthesis";
    let res = code_search(invalid_pattern, env.path());

    assert!(res.is_err(), "Invalid regex syntax must return SearchError::InvalidRegex");
    match res.unwrap_err() {
        SearchError::InvalidRegex { pattern, .. } => {
            assert_eq!(pattern, invalid_pattern);
        }
        other => panic!("Expected SearchError::InvalidRegex, got: {other:?}"),
    }
}

#[test]
fn test_tier2_code_search_non_existent_path() {
    let non_existent = std::path::Path::new("/non/existent/path/for/rusi/search");

    let res = code_search("pattern", non_existent);
    assert!(res.is_err(), "Non-existent path must return an error");
    match res.unwrap_err() {
        SearchError::PathNotFound(p) => {
            assert_eq!(p, non_existent);
        }
        SearchError::Io(_) => {}
        other => panic!("Expected PathNotFound or Io error, got: {other:?}"),
    }
}

#[test]
fn test_tier2_code_search_no_matches_returns_empty_vec() {
    let env = TestEnv::new();
    env.create_file("test.txt", "some boring plain text\n");

    let matches = code_search("ABSENT_PATTERN_XYZ", env.path())
        .expect("code_search with no matches should return Ok(vec![])");
    assert!(matches.is_empty(), "Empty matches vector expected");
}

#[test]
fn test_tier2_code_search_gracefully_skips_binary_data() {
    let env = TestEnv::new();
    // Create a binary file with non-utf8 bytes
    let bin_path = env.file_path("binary.dat");
    std::fs::write(&bin_path, b"\x00\xFF\xFE\xFD\xFCSEARCH_TOKEN\x00\x01\x02").unwrap();

    // Create a normal utf8 file
    env.create_file("text.txt", "SEARCH_TOKEN found here\n");

    let matches = code_search("SEARCH_TOKEN", env.path())
        .expect("code_search should not crash on binary files");

    // Text file should match
    assert!(matches.iter().any(|m| m.file.ends_with("text.txt")));
}
