# Changelog

Each release's changes, newest first. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

**Viewer**

- A start page: buttons to open a log or a folder, recent folders and logs, and this PC's usual log folders with their key logs, such as `AppEnforce.log`, each one click away.
- File > Open folder... (Ctrl+Shift+O), or dropping a folder, lists its logs with how many errors and warnings each has, so the ones worth reading stand out.
- The event histogram is on by default.
- With one log open, Split view asks for the log to show beside it, rather than only saying in the status bar that it needs two.

**AI agents**

- `traceview setup`, or Tools > Set up for AI agents..., gets AI agents using TraceView with no editing by hand. For your Windows account only, it copies TraceView to `%LOCALAPPDATA%\Programs\TraceView`, adds that folder to your PATH, and adds a skill telling Claude Code, Codex and the ChatGPT app how to use it, for whichever are installed. It says what will change and asks first. `traceview setup --remove`, or Tools > Remove AI agent setup..., undoes it.
- `traceview setup --print`, or Tools > Copy instructions for AI agents, gives the instructions for any other agent.
- The skill tells agents to use `summarize --format lines` and `query --newest-first`. Run `traceview setup` again to update a skill written before.

**Command line**

- `query` and `summarize` take several logs, or folders of them. A query merges them into one timeline by time. A summary lists the logs with problems, worst first, and the worst problems across all of them.
- `query --format lines` prints one short line per row, at under half the size of the JSON. Each row starts with its log and row number, as in `CBS.log:11503`, whether one log was queried or several. In JSON, one log's rows also name their log now.
- `query --newest-first` starts from the newest match, so `--limit 10` gives the last ten.
- `summarize --format lines` prints a line per log with problems, per problem pattern, per error spike and per error code, at about a third of the JSON's size. `summarize --no-examples` leaves the example rows out of the JSON.
- Lines can be trimmed with `--fields`, such as `--fields time,message`, `--max-width` and `--no-header`.
- `--help` says where the index cache is kept, and that TraceView never writes into the folders it reads.
- A summary counts a log's rotated copies as part of it: `AppEnforce.lo_` and `AppEnforce-20261020-123456.log` add to `AppEnforce.log`, `setupapi.dev.20261014_095916.log` to `setupapi.dev.log`, and `CbsPersist_20261105172003.log` to `CBS.log`. An error repeated across them is one problem, with one count.
- `--current-only` leaves rotated copies out of a folder, and `--with-history` adds a named log's rotated copies from beside it.
- `--recurse` reads subfolders too, labeling their logs by their path in the folder. Without it, the output names the subfolders it didn't read.
- `summarize --from` and `--to` summarize only the rows in a time range.
- `query --count-by day|hour|file|level|component|template` counts the matches in groups instead of listing them.
- `--exclude <pattern>` leaves out rows matching a regular expression, such as known noise, in queries and summaries. It can be given more than once. A summary says how many rows, errors and warnings it left out.
- Rows show their offset from UTC when the log writes one, as ConfigMgr and Intune logs do: `"utc_offset": "-06:00"` in JSON, and after the time in lines, as in `2026-11-01T01:30:00.000-05:00`.

**Error Lookup**

- WMI codes, such as 0x80041010, are explained, with text from Windows' own `wmiutils.dll`.
- Windows Update (0x8024…) and Delivery Optimization (0x80D0…) codes, which Windows has no text for, show their names, such as `WU_E_PT_WINHTTP_NAME_NOT_RESOLVED`, in the app, the details pane, `traceview lookup` and summaries.
- Summary patterns keep an example row, with its real numbers and codes, and summaries look up the error codes in their problems.
- `traceview lookup <code>` explains an error code, as Error Lookup does in the app.

## 0.9.0

The first release, a beta. It's been tested on Windows 11, but not yet on Windows 10, Server 2019 or with Narrator.

**Reading logs**

- Opens plain, SCCM (CMTrace) and custom-format logs of any size, with severity colors, and follows them live, including through SCCM's log rotation.
- Find, filter by words, `/regex/`, level, component, thread and time, and merge open logs into one timeline.
- Error Lookup for Windows, HRESULT, NTSTATUS, WinHTTP and BITS codes, with text from Windows itself.
- Severity chips, the event histogram, a sidebar of nearby and Windows logs, bookmarks, filter presets, split view with time sync, a details pane and a command palette.
- Five themes, including CMTrace's Classic, and Windows contrast themes followed whenever they're on.
- Screen readers get every control through UI Automation, and everything works from the keyboard.

**Making it yours**

- Highlight rules, columns, fonts and row spacing, keyboard shortcuts, custom log formats, theme files and named layouts.
- Plain-text settings in `traceview.toml`, which load as they're edited. A `traceview.toml` beside the EXE makes a portable copy.
- Tools > Use TraceView for .log files, for your Windows account only.

**Command line**

- `traceview query` and `traceview summarize` return JSON, for scripts and AI agents. `traceview --licenses` shows the licenses of TraceView and its open-source parts.

**The release**

- Signed EXEs for x64 and ARM64 PCs, in zips with their licenses and SHA-256 checksums.
