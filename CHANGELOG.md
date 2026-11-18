# Changelog

Each release's changes, newest first. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

**Reading logs**

- Windows Update's `ReportingEvents.log` is read as rows, in the viewer and the command line. Each row's time, with its milliseconds and offset from UTC, is in the Time column, and a `Failure` is an error. The operation, such as `Content Install`, is the component, the program that asked, such as `wa_3rd_party_host_32.exe`, the thread, and the event's name the source. Before, each whole row was the message, with no time. Indexes of logs of 64 MB or more are rebuilt once, the next time each is opened.

**Viewer**

- ▲ and ▼ buttons beside the Find box move to the previous and next match, as Shift+F3 and F3 do, and leave the cursor in the box.
- A Pause button at the end of the status bar pauses live updates, as Ctrl+U does, and shows Resume while paused.
- End in the log goes back to following new rows, and now ends a pause too.
- View > Fonts is now View > Fonts and row spacing. With row spacing selected, its buttons say Less space and More space, rather than the font buttons, which read as if they changed the font.
- Double-clicking in the Find, Filter or any other text box selects the word under the pointer, as in Windows' own text boxes.

**Error lookup**

- On a ConfigMgr client or site server, ConfigMgr's own codes (0x87D0… and 0x87D1…) get text, such as "Item not found" for 0x87D00215, and many Windows Update codes get text beside their names. It comes, in English, from the copy of CMTrace that ConfigMgr installs, which TraceView reads as a file but never runs. `lookup`, summaries and the Error Lookup window all use it.
- On other PCs, such as one logs are copied to, a copy of `CMTrace.exe` in the folder holding `traceview.exe` gives the same text.

**Command line**

- `--help` says that `--count-by` gives JSON unless `--format lines` is added.
- `--from` and `--to` take times without seconds, such as `2026-10-15T10:06`. As a `--to`, that takes in the whole minute.
- `--text` and `--regex` can be given together, or more than once, to find only the rows matching every one. Before, using both was an error.
- `query --context N` shows the N rows before and after each match, from its own log, so there's no need to open the file to see what led up to an error. In lines, they're indented, `--` separates the groups, and matches close together share one. In JSON, each match gets `before` and `after` lists.
- Summaries list the longest gaps of a day or more with no rows, five at most, in time order. For a folder, that's no rows in any log, as when a PC was off or asleep for weeks, which before could only be found with `--count-by day`. In JSON, they're `gaps`, each with `from`, `to` and `days`.
- `--unless-followed-by` says why nothing closed each match it finds. Either the same thing started again first, and whether that one finished, or it's still open at the end of its log. Before, an attempt that was abandoned and then retried successfully looked the same as one that hung. In lines, a label in brackets says which, and the header counts each. In JSON, it's `unclosed` and `next_match`, with the counts as `started_again` and `still_open`.
- `query --count-by template` gives each template its first row as an example, with its log and row number, so the versions, KB numbers and codes a template takes out can still be read. `7-Zip 26.04 (MSI-x64)` used to show only as `<n>-Zip <n>.<n> (MSI-x<n>)`. `--no-examples` leaves them out.
- `query --first-line` matches and shows only each row's first line, for SCCM entries that span many lines, such as WMI dumps, which used to match deep inside and flood results. Lines say how many were left out, as in `[+14 lines]`, and JSON has `more_lines`. It's off unless asked for.
- The AI agent skill mentions `--first-line`, gaps, `--context`, giving `--text` more than once, and `--fields` and `--max-width` for keeping lines short. Run `traceview setup` again to update it.

## 0.10.0

A start page and folder view, a command line built for AI agents to read logs with, and `traceview setup` to get Claude Code, Codex and the ChatGPT app using it. Still a beta, tested on Windows 11.

**Viewer**

- A start page: buttons to open a log or a folder, recent folders and logs, and this PC's usual log folders with their key logs, such as `AppEnforce.log`, each one click away.
- File > Open folder... (Ctrl+Shift+O), or dropping a folder, lists its logs with how many errors and warnings each has, so the ones worth reading stand out.
- The event histogram is on by default.
- With one log open, Split view asks for the log to show beside it, rather than only saying in the status bar that it needs two.

**AI agents**

- `traceview setup`, or Tools > Set up for AI agents..., gets AI agents using TraceView with no editing by hand. For your Windows account only, it copies TraceView to `%LOCALAPPDATA%\Programs\TraceView`, adds that folder to your PATH, and adds a skill telling Claude Code, Codex and the ChatGPT app how to use it, for whichever are installed. It says what will change and asks first. `traceview setup --remove`, or Tools > Remove AI agent setup..., undoes it.
- `traceview setup --print`, or Tools > Copy instructions for AI agents, gives the instructions for any other agent.
- The skill tells agents to start with `summarize --format lines`, then narrow down with `query`. To update TraceView and its skills later, run `traceview setup` from the newer EXE.

**Command line**

- `query` and `summarize` take several logs, or folders of them. A query merges them into one timeline by time. A summary lists the logs with problems, worst first, and the worst problems across all of them.
- Summary patterns keep an example row, with its real numbers and codes, and summaries look up the error codes in their problems.
- `traceview lookup <code>` explains an error code, as Error Lookup does in the app.
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
- `query --unless-followed-by <text>` keeps only matches with no row containing that text after them before the next match, such as an `Entering CS mode` never followed by `Leaving CS mode`.
- `--exclude <pattern>` leaves out rows matching a regular expression, such as known noise, in queries and summaries. It can be given more than once. A summary says how many rows, errors and warnings it left out.
- Rows show their offset from UTC when the log writes one, as ConfigMgr and Intune logs do: `"utc_offset": "-06:00"` in JSON, and after the time in lines, as in `2026-11-01T01:30:00.000-05:00`.

**Error Lookup**

- WMI codes, such as 0x80041010, are explained, with text from Windows' own `wmiutils.dll`.
- Windows Update (0x8024…) and Delivery Optimization (0x80D0…) codes, which Windows has no text for, show their names, such as `WU_E_PT_WINHTTP_NAME_NOT_RESOLVED`, in the app, the details pane, `traceview lookup` and summaries.

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
