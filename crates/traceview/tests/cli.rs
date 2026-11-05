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
    assert_eq!(
        result["problem_patterns"][0]["example"],
        "Download failed - error 0x80004005"
    );
    assert_eq!(json["codes"][0]["code"], "0x80004005");
    let reading = &json["codes"][0]["readings"][0];
    assert_eq!(reading["kind"], "HRESULT");
    assert!(!reading["message"].as_str().unwrap().is_empty());
}

/// A second log, in its own folder, written later than the sample.
const SECOND_LOG: &str = concat!(
    "<![LOG[Content download failed - error 0x80070002]LOG]!><time=\"10:23:02.100+000\" date=\"04-06-2026\" component=\"ContentTransferManager\" context=\"\" type=\"3\" thread=\"77\" file=\"c.cpp\">
",
    "<![LOG[All done]LOG]!><time=\"10:23:03.000+000\" date=\"04-06-2026\" component=\"ContentTransferManager\" context=\"\" type=\"1\" thread=\"77\" file=\"c.cpp\">
",
);

fn logs_folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sample.log"), SCCM_LOG).unwrap();
    std::fs::write(dir.path().join("CTM.log"), SECOND_LOG).unwrap();
    std::fs::write(
        dir.path().join("Clean.log"),
        "2026-04-06 10:00:00 host a[1]: all good
",
    )
    .unwrap();
    std::fs::write(dir.path().join("notes.md"), "not a log").unwrap();
    dir
}

#[test]
fn a_folder_query_merges_logs_by_time() {
    let dir = logs_folder();
    let json = stdout_json(&traceview(&[
        "query",
        dir.path().to_str().unwrap(),
        "--min-level",
        "warning",
    ]));
    assert_eq!(json["log_text_trust"], "untrusted");
    assert_eq!(json["logs_read"], 3);
    let result = &json["result"];
    assert_eq!(result["total_rows"], 6);
    assert_eq!(result["total_matches"], 3);
    let found: Vec<(&str, u64)> = result["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["file"].as_str().unwrap(), row["row"].as_u64().unwrap()))
        .collect();
    // 10:23:02.100 falls between the sample's rows at 02.334 and 02.500.
    assert_eq!(
        found,
        [("CTM.log", 1), ("sample.log", 2), ("sample.log", 3)]
    );
}

#[test]
fn several_files_query_as_one_timeline() {
    let dir = logs_folder();
    let first = dir.path().join("sample.log");
    let second = dir.path().join("CTM.log");
    let json = stdout_json(&traceview(&[
        "query",
        first.to_str().unwrap(),
        second.to_str().unwrap(),
        "--limit",
        "2",
    ]));
    assert_eq!(json["logs_read"], 2);
    assert_eq!(json["result"]["total_matches"], 5);
    assert_eq!(json["result"]["next_offset"], 2);
    assert_eq!(json["result"]["rows"][0]["message"], "Connected to mp01");
}

#[test]
fn a_missing_file_among_several_is_an_error() {
    let (_dir, path) = sample_log();
    let output = traceview(&["query", path.to_str().unwrap(), "missing.log"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr_code(&output), "file_not_found");
}

#[test]
fn a_folder_with_no_logs_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.md"), "not a log").unwrap();
    let output = traceview(&["summarize", dir.path().to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr_code(&output), "no_logs");
}

#[test]
fn a_folder_summary_lists_logs_with_problems_first() {
    let dir = logs_folder();
    let json = stdout_json(&traceview(&["summarize", dir.path().to_str().unwrap()]));
    assert_eq!(json["logs_read"], 3);
    let result = &json["result"];
    assert_eq!(result["total_rows"], 6);
    assert_eq!(result["levels"]["error"], 2);
    let logs = result["logs_with_problems"].as_array().unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0]["file"], "sample.log");
    assert_eq!(logs[0]["errors"], 1);
    assert_eq!(logs[0]["warnings"], 1);
    assert_eq!(result["other_logs"], serde_json::json!(["Clean.log"]));
    let patterns = result["problem_patterns"].as_array().unwrap();
    assert_eq!(patterns.len(), 3);
    assert!(patterns[..2]
        .iter()
        .all(|pattern| pattern["level"] == "error"));
    assert_eq!(patterns[2]["file"], "sample.log");
    let codes: Vec<&str> = result["codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|code| code["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"0x80070002") && codes.contains(&"0x80004005"));
}

#[test]
fn lines_output_puts_one_row_on_each_line() {
    let (_dir, path) = sample_log();
    let output = traceview(&["query", path.to_str().unwrap(), "--format", "lines"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].starts_with("# "));
    assert!(lines[0].contains("3 of 3 rows match"));
    assert!(lines[0].contains("untrusted"));
    assert_eq!(
        lines[2],
        "2 2026-04-06T10:23:02.334 ERR PolicyAgent 5240 | Download failed - error 0x80004005"
    );
    assert_eq!(
        lines[3],
        "3 2026-04-06T10:23:02.500 WARN PolicyAgent 5240 | Retrying in 15 minutes\\nsecond line"
    );
}

#[test]
fn lines_output_names_each_log_when_merged() {
    let dir = logs_folder();
    let output = traceview(&[
        "query",
        dir.path().to_str().unwrap(),
        "--min-level",
        "error",
        "--format",
        "lines",
        "--limit",
        "1",
    ]);
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains("3 logs merged by time: 2 of 6 rows match"));
    assert!(lines[0].contains("Next page: --offset 1."));
    assert!(lines[1].starts_with("CTM.log:1 2026-04-06T10:23:02.100 ERR "));
}

#[test]
fn lookup_explains_a_code_in_any_form() {
    for code in ["0x80070005", "-2147024891", "80070005"] {
        let json = stdout_json(&traceview(&["lookup", code]));
        assert_eq!(json["input"], code);
        let reading = &json["readings"][0];
        assert_eq!(reading["hex"], "0x80070005", "{code}");
        assert_eq!(reading["facility"], "WIN32 (7)");
        assert!(!reading["message"].as_str().unwrap().is_empty());
    }
    let json = stdout_json(&traceview(&["lookup", "1603"]));
    let readings = json["readings"].as_array().unwrap();
    assert!(readings.iter().all(|reading| reading["message"] != ""));
}

#[test]
fn lookup_without_a_code_is_an_error() {
    let output = traceview(&["lookup", "hello"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stderr_code(&output), "no_code");
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
        &["query", path, "--limit", "many"],
        &["query", path, "--min-level", "loud"],
        &["query", path, "--text", "a", "--regex", "b"],
        &["query", path, "--text"],
        &["query", path, "--top", "5"],
        &["summarize", path, "--text", "a"],
        &["query", path, "--format", "xml"],
        &["summarize", path, "--format", "lines"],
        &["lookup"],
        &["lookup", "--nope"],
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
    let licenses = traceview(&["--licenses"]);
    assert!(licenses.status.success());
    let text = String::from_utf8_lossy(&licenses.stdout);
    assert!(text.contains("Copyright (c) 2026 Nathaniel Graham"));
    assert!(text.contains("Third-party notices for TraceView"));
    assert!(text.contains("accesskit_windows"));
}
