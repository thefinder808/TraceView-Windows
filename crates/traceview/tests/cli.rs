//! Runs the real executable, because scripts and AI agents depend on its exact
//! JSON shape, exit codes and error reporting.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

const SCCM_LOG: &str = concat!(
    "<![LOG[Connected to mp01]LOG]!><time=\"10:23:01.442+000\" date=\"04-06-2026\" component=\"CcmMessaging\" context=\"\" type=\"1\" thread=\"4128\" file=\"a.cpp\">\r\n",
    "<![LOG[Download failed - error 0x80004005]LOG]!><time=\"10:23:02.334+000\" date=\"04-06-2026\" component=\"PolicyAgent\" context=\"\" type=\"3\" thread=\"5240\" file=\"b.cpp\">\r\n",
    "<![LOG[Retrying in 15 minutes\r\nsecond line]LOG]!><time=\"10:23:02.500+000\" date=\"04-06-2026\" component=\"PolicyAgent\" context=\"\" type=\"2\" thread=\"5240\" file=\"b.cpp\">\r\n",
);

fn traceview(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_traceview"))
        .args(args)
        .output()
        .unwrap()
}

fn sample_log() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.log");
    std::fs::write(&path, SCCM_LOG).unwrap();
    (dir, path)
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn stderr_code(output: &Output) -> String {
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    error["error"]["code"].as_str().unwrap().to_string()
}

#[test]
fn query_returns_rows_as_json() {
    let (_dir, path) = sample_log();
    let json = stdout_json(&traceview(&["query", path.to_str().unwrap()]));
    assert_eq!(json["log_text_trust"], "untrusted");
    assert_eq!(json["file"]["format"], "sccm");
    assert_eq!(json["file"]["encoding"], "utf8");
    assert_eq!(json["file"]["rows"], 3);
    assert_eq!(json["result"]["total_matches"], 3);

    let row = &json["result"]["rows"][1];
    assert_eq!(row["row"], 2);
    assert_eq!(row["level"], "error");
    assert_eq!(row["component"], "PolicyAgent");
    assert_eq!(row["thread"], "5240");
    assert_eq!(row["source"], "b.cpp");
    assert_eq!(row["timestamp"], "2026-04-06T10:23:02.334");
    assert_eq!(row["message"], "Download failed - error 0x80004005");
    assert_eq!(
        json["result"]["rows"][2]["message"],
        "Retrying in 15 minutes\r\nsecond line"
    );
}

#[test]
fn query_applies_filters() {
    let (_dir, path) = sample_log();
    let path = path.to_str().unwrap();
    let matches = |args: &[&str]| {
        let mut all = vec!["query", path];
        all.extend_from_slice(args);
        stdout_json(&traceview(&all))["result"]["total_matches"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(matches(&["--text", "DOWNLOAD"]), 1);
    assert_eq!(matches(&["--text", "DOWNLOAD", "--case-sensitive"]), 0);
    assert_eq!(matches(&["--regex", r"error 0x[0-9a-f]{8}"]), 1);
    assert_eq!(matches(&["--min-level", "warning"]), 2);
    assert_eq!(matches(&["--min-level", "error"]), 1);
    assert_eq!(matches(&["--level", "warning"]), 1);
    assert_eq!(matches(&["--component", "policyagent"]), 2);
    assert_eq!(matches(&["--thread", "5240"]), 2);
    assert_eq!(matches(&["--thread", "4128", "--min-level", "error"]), 0);
    assert_eq!(matches(&["--from", "2026-04-06T10:23:02"]), 2);
    assert_eq!(matches(&["--to", "2026-04-05"]), 0);
}

#[test]
fn query_pages_results() {
    let (_dir, path) = sample_log();
    let path = path.to_str().unwrap();
    let first = stdout_json(&traceview(&["query", path, "--limit", "2"]));
    assert_eq!(first["result"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(first["result"]["next_offset"], 2);
    let second = stdout_json(&traceview(&[
        "query", path, "--limit", "2", "--offset", "2",
    ]));
    assert_eq!(second["result"]["rows"].as_array().unwrap().len(), 1);
    assert_eq!(second["result"]["next_offset"], Value::Null);
}

#[test]
fn summarize_reports_levels_and_patterns() {
    let (_dir, path) = sample_log();
    let json = stdout_json(&traceview(&[
        "summarize",
        path.to_str().unwrap(),
        "--top",
        "5",
    ]));
    assert_eq!(json["log_text_trust"], "untrusted");
    let result = &json["result"];
    assert_eq!(result["total_rows"], 3);
    assert_eq!(result["levels"]["error"], 1);
    assert_eq!(result["levels"]["warning"], 1);
    assert_eq!(result["levels"]["info"], 1);
    assert_eq!(result["first_timestamp"], "2026-04-06T10:23:01.000");
    assert_eq!(
        result["problem_patterns"][0]["template"],
        "Download failed - error <hex>"
    );
    assert_eq!(result["problem_patterns"][0]["level"], "error");
}

#[test]
fn pretty_output_is_the_same_json() {
    let (_dir, path) = sample_log();
    let path = path.to_str().unwrap();
    let compact = traceview(&["query", path, "--no-cache"]);
    let pretty = traceview(&["query", path, "--no-cache", "--pretty"]);
    assert!(String::from_utf8_lossy(&pretty.stdout).contains("\n  "));
    let strip_timing = |mut value: Value| {
        value["file"]["open_seconds"] = Value::Null;
        value
    };
    assert_eq!(
        strip_timing(stdout_json(&compact)),
        strip_timing(stdout_json(&pretty))
    );
}

#[test]
fn a_missing_file_is_a_json_error_with_exit_code_one() {
    let output = traceview(&["query", "does-not-exist.log"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr_code(&output), "file_not_found");
    assert!(output.stdout.is_empty());
}

#[test]
fn a_bad_pattern_is_a_json_error() {
    let (_dir, path) = sample_log();
    let output = traceview(&["query", path.to_str().unwrap(), "--regex", "("]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr_code(&output), "invalid_pattern");
}

#[test]
fn a_bad_time_is_a_json_error() {
    let (_dir, path) = sample_log();
    let output = traceview(&["query", path.to_str().unwrap(), "--from", "yesterday"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr_code(&output), "invalid_time");
}

#[test]
fn incorrect_usage_exits_with_code_two() {
    let (_dir, path) = sample_log();
    let path = path.to_str().unwrap();
    for args in [
        &["frobnicate"][..],
        &["query"],
        &["query", path, "--nope"],
        &["query", path, "extra"],
        &["query", path, "--limit", "many"],
        &["query", path, "--min-level", "loud"],
        &["query", path, "--text", "a", "--regex", "b"],
        &["query", path, "--text"],
        &["query", path, "--top", "5"],
        &["summarize", path, "--text", "a"],
    ] {
        let output = traceview(args);
        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
        assert_eq!(stderr_code(&output), "usage", "args: {args:?}");
    }
}

#[test]
fn help_and_version_succeed() {
    let help = traceview(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("traceview query"));
    let version = traceview(&["--version"]);
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("traceview "));
}
