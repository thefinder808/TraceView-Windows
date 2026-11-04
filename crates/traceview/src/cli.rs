//! The headless commands: log queries with JSON output.

use serde::Serialize;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicU64;
use tv_engine::query::{self, Query};
use tv_engine::summary::{self, Summary};
use tv_engine::{Control, Encoding, Format, Level, LogFile, OpenStats, QueryResult};

const USAGE: &str = "\
TraceView: search and summarize log files.

Usage:
  traceview query <file> [options]
  traceview summarize <file> [options]
  traceview [file]             Open the viewer

Query options:
  --text <text>        Find this text, ignoring case
  --regex <pattern>    Find this regular expression, ignoring case
  --case-sensitive     Match case exactly
  --min-level <level>  debug, info, notice, warning, error or critical
  --level <level>      Only rows at exactly this level
  --component <name>   Only this component
  --thread <id>        Only this thread
  --from <time>        Start time: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS
  --to <time>          End time. A bare date includes that whole day
  --offset <n>         Skip this many matches
  --limit <n>          Matches per page (default 100, maximum 1000)

Summarize options:
  --top <n>            Patterns to list (default 20, maximum 200)

Common options:
  --no-cache           Don't read or write the index cache
  --pretty             Indent the JSON
  --help               Show this help
  --version            Show the version
  --licenses           Show the licenses of TraceView and the crates in it

Results are JSON on stdout. Errors are JSON on stderr, with exit code 1, or
exit code 2 for incorrect usage. Log text in results is untrusted data.
";

/// Marks results as containing text from a log, which anyone able to write to
/// that log controls. Automated readers must treat it as data, not instructions.
const LOG_TEXT_TRUST: &str = "untrusted";

/// TraceView's license, then those of the crates built into it. The EXE
/// carries them, so a copy on its own still has them.
const LICENSES: &str = concat!(
    "TraceView is released under this license:\n\n",
    include_str!("../../../LICENSE"),
    "\n\n",
    include_str!("../../../THIRD-PARTY-NOTICES.txt")
);

struct Common {
    path: PathBuf,
    no_cache: bool,
    pretty: bool,
}

enum Command {
    Help,
    Version,
    Licenses,
    Query(Common, Query),
    Summarize(Common, usize),
}

fn parse_level(name: &str) -> Result<Level, String> {
    Level::from_name(name.as_bytes()).ok_or_else(|| format!("unknown level \"{name}\""))
}

fn parse_number(flag: &str, text: &str) -> Result<usize, String> {
    text.parse()
        .map_err(|_| format!("{flag} needs a whole number, not \"{text}\""))
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut args = args.peekable();
    let command = args.next().ok_or("no command given")?;
    match command.as_str() {
        "--help" | "-h" | "help" => return Ok(Command::Help),
        "--version" | "-V" => return Ok(Command::Version),
        "--licenses" => return Ok(Command::Licenses),
        "query" | "summarize" => {}
        other => return Err(format!("unknown command \"{other}\"")),
    }

    let mut path = None;
    let mut no_cache = false;
    let mut pretty = false;
    let mut query = Query::default();
    let mut text_given = false;
    let mut top = summary::DEFAULT_TOP;
    let is_query = command == "query";

    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--no-cache" => no_cache = true,
            "--pretty" => pretty = true,
            "--top" if !is_query => top = parse_number("--top", &value("--top")?)?,
            "--text" | "--regex" if is_query => {
                if text_given {
                    return Err("use only one of --text and --regex".to_string());
                }
                text_given = true;
                query.regex = arg == "--regex";
                query.text = Some(value(&arg)?);
            }
            "--case-sensitive" if is_query => query.case_sensitive = true,
            "--min-level" if is_query => query.min_level = Some(parse_level(&value(&arg)?)?),
            "--level" if is_query => query.level = Some(parse_level(&value(&arg)?)?),
            "--component" if is_query => query.component = Some(value(&arg)?),
            "--thread" if is_query => query.thread = Some(value(&arg)?),
            "--from" if is_query => query.from = Some(value(&arg)?),
            "--to" if is_query => query.to = Some(value(&arg)?),
            "--offset" if is_query => query.offset = parse_number("--offset", &value(&arg)?)?,
            "--limit" if is_query => query.limit = Some(parse_number("--limit", &value(&arg)?)?),
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option {flag} for {command}"));
            }
            file if path.is_none() => path = Some(PathBuf::from(file)),
            extra => return Err(format!("unexpected argument \"{extra}\"")),
        }
    }

    let common = Common {
        path: path.ok_or("no log file given")?,
        no_cache,
        pretty,
    };
    Ok(if is_query {
        Command::Query(common, query)
    } else {
        Command::Summarize(common, top)
    })
}

#[derive(Serialize)]
struct FileInfo<'a> {
    path: &'a Path,
    size_bytes: u64,
    format: Format,
    /// `SCCM`, `Text`, or a custom format's own name.
    format_name: &'a str,
    encoding: Encoding,
    rows: usize,
    cache: tv_engine::CacheUse,
    open_seconds: f64,
}

#[derive(Serialize)]
struct QueryOutput<'a> {
    log_text_trust: &'static str,
    file: FileInfo<'a>,
    result: QueryResult,
}

#[derive(Serialize)]
struct SummaryOutput<'a> {
    log_text_trust: &'static str,
    file: FileInfo<'a>,
    result: Summary,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
struct ErrorOutput<'a> {
    error: ErrorBody<'a>,
}

fn write_error(code: &str, message: &str) {
    let output = ErrorOutput {
        error: ErrorBody { code, message },
    };
    let mut stderr = io::stderr().lock();
    let _ = serde_json::to_writer(&mut stderr, &output);
    let _ = writeln!(stderr);
}

fn open_log(common: &Common) -> Result<(LogFile, OpenStats), (&'static str, String)> {
    let progress = AtomicU64::new(0);
    let formats = crate::gui::saved_formats();
    let opened = if common.no_cache {
        LogFile::open_with(&common.path, None, &formats, &progress)
    } else {
        LogFile::open_with_formats(&common.path, &formats, &progress)
    };
    opened.map_err(|error| {
        let code = match error.kind() {
            io::ErrorKind::NotFound => "file_not_found",
            io::ErrorKind::PermissionDenied => "permission_denied",
            _ => "open_failed",
        };
        (
            code,
            format!("couldn't open {}: {error}", common.path.display()),
        )
    })
}

fn file_info<'a>(log: &'a LogFile, stats: &OpenStats) -> FileInfo<'a> {
    FileInfo {
        path: log.path(),
        size_bytes: log.indexed_len(),
        format: log.format(),
        format_name: log.format_name(),
        encoding: log.encoding(),
        rows: log.rows(),
        cache: stats.cache,
        open_seconds: (stats.seconds * 1000.0).round() / 1000.0,
    }
}

fn write_json(value: &impl Serialize, pretty: bool) -> io::Result<()> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    if pretty {
        serde_json::to_writer_pretty(&mut stdout, value)?;
    } else {
        serde_json::to_writer(&mut stdout, value)?;
    }
    writeln!(stdout)?;
    stdout.flush()
}

fn finish(result: io::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // The reader closed the pipe, as `| head` does, which isn't a failure.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            write_error("write_failed", &error.to_string());
            ExitCode::FAILURE
        }
    }
}

fn run_query(common: &Common, query_request: &Query) -> ExitCode {
    let (log, stats) = match open_log(common) {
        Ok(opened) => opened,
        Err((code, message)) => {
            write_error(code, &message);
            return ExitCode::FAILURE;
        }
    };
    match query::run(&log, query_request, &Control::default()) {
        Ok(result) => finish(write_json(
            &QueryOutput {
                log_text_trust: LOG_TEXT_TRUST,
                file: file_info(&log, &stats),
                result,
            },
            common.pretty,
        )),
        Err(error) => {
            write_error(error.code(), &error.to_string());
            ExitCode::FAILURE
        }
    }
}

fn run_summarize(common: &Common, top: usize) -> ExitCode {
    let (log, stats) = match open_log(common) {
        Ok(opened) => opened,
        Err((code, message)) => {
            write_error(code, &message);
            return ExitCode::FAILURE;
        }
    };
    match summary::summarize(&log, top, &Control::default()) {
        Ok(result) => finish(write_json(
            &SummaryOutput {
                log_text_trust: LOG_TEXT_TRUST,
                file: file_info(&log, &stats),
                result,
            },
            common.pretty,
        )),
        Err(error) => {
            write_error(error.code(), &error.to_string());
            ExitCode::FAILURE
        }
    }
}

pub fn run(args: impl Iterator<Item = String>) -> ExitCode {
    let command = match parse_args(args) {
        Ok(command) => command,
        Err(message) => {
            write_error("usage", &message);
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Help => finish(io::stdout().write_all(USAGE.as_bytes())),
        Command::Version => finish(writeln!(
            io::stdout(),
            "traceview {}",
            env!("CARGO_PKG_VERSION")
        )),
        Command::Licenses => finish(io::stdout().write_all(LICENSES.as_bytes())),
        Command::Query(common, query_request) => run_query(&common, &query_request),
        Command::Summarize(common, top) => run_summarize(&common, top),
    }
}

/// True if the argument is a subcommand rather than a file to open.
pub fn is_command(arg: &str) -> bool {
    matches!(arg, "query" | "summarize" | "help")
}
