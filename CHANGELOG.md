# Changelog

Each release's changes, newest first. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

**Viewer**

- A start page: buttons to open a log or a folder, recent folders and logs, and this PC's usual log folders with their key logs, such as `AppEnforce.log`, each one click away.
- File > Open folder... (Ctrl+Shift+O), or dropping a folder, lists its logs with how many errors and warnings each has, so the ones worth reading stand out.
- The event histogram is on by default.
- With one log open, Split view asks for the log to show beside it, rather than only saying in the status bar that it needs two.

**Command line**

- `query` and `summarize` take several logs, or folders of them. A query merges them into one timeline by time. A summary lists the logs with problems, worst first, and the worst problems across all of them.
- `query --format lines` prints one short line per row, at under half the size of the JSON.
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
