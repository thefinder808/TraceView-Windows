# Changelog

Each release's changes, newest first. Versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

**Reading logs**

- ConfigMgr site server logs, such as `sitecomp.log` and `distmgr.log`, are read in the `$$<` format CMTrace also reads, with each row's time, offset from UTC, component and thread. Before, each whole row was the message, with no time.
- `WindowsUpdate.log`, as `Get-WindowsUpdateLog` writes it, has its component, thread ID and process ID in the Component, Thread and Source columns. Before, a colon early in a message made the IDs and the component read as one component, such as `51540 Misc UUS`, and summaries split one problem into several patterns because the IDs' widths vary.
- winget's logs have their level and component, from the `<I> [CORE]` after each time.
- Windows Installer verbose logs, from `msiexec /l*v`, have times: each row's time of day on the date its `=== Logging started:` line gives. `MSI (s)` is the component, the process and thread the thread, and `Return value 3`, an action that failed, is an error. Before, no row had a time.
- PowerShell transcripts mark error records as errors, from Windows PowerShell and PowerShell 7, such as `Get-Item : Cannot find path…` and its `+ CategoryInfo` lines. Their `Start time:` and `Command start time:` lines have times. Before, only `WARNING:` and `VERBOSE:` had a level.
- Office's Click-to-Run logs, such as `%TEMP%\<PC>-20261111-1324.log`, and SharePoint's, in the ULS format, have their time, level, category, thread and process. `Unexpected` is an error, and `Monitorable` and `High` are notices. Before, each whole row was the message, with no time.
- WiX Burn's logs, from the Visual C++ and .NET installers, and the Visual Studio installer's, such as `[0A1C:0B2C][2026-11-18T12:31:32]i001: msg`, have their time and thread, and Burn's `i`, `w` and `e` codes are their levels. Before, they had no time.
- IIS logs, such as `u_ex261118.log` from a ConfigMgr management or distribution point, and Windows Firewall's `pfirewall.log` are read by the columns their `#Fields:` line names. Each row has its time and the client's address. For IIS, the page asked for is the component, a 5xx status is an error and a 4xx a warning, except 401, which every Windows-authenticated request gets first, and times say they're UTC. For the firewall, the action is the component, and `DROP` is a warning. Before, rows had a time but nothing else.
- `setupapi.dev.log` goes by its `!!!` and `!` markers, so `Error Control = 1` is no longer an error, and a failed `[Exit status: FAILURE(…)]` is. Sections' starts and ends have times, and the short name before a colon, such as `dvi`, is the component.
- Indexes of logs of 64 MB or more are rebuilt once, the next time each is opened.

**Command line**

- Summaries count a template at each level apart, so a pattern's count is how many rows it had at its level. Before, a template that was only sometimes an error, such as a CBS row raised by its HRESULT, was one pattern at its worst level that counted every row, information included. IIS requests, whose status sets the level, would otherwise all have counted as errors.

**Viewer**

- Selecting the last row of a log that's following new rows, by clicking it or with End, keeps the newest row selected as rows arrive, so the details pane shows each one. Before, the selection stayed on that row as it scrolled away. Any other selection stays where it is.
- The Filter and Find boxes show a × at their right end while they have text. Clicking it empties the box, which for Filter shows every row again, and leaves the cursor there for new text.
- The Pause button at the end of the status bar is now a square like the find buttons, showing the pause symbol, and ▶ while paused, rather than the words Pause and Resume.
- A log opened from another PC, such as `\\SRV01\c$\Windows\CCM\Logs\WUAHandler.log`, is named with its machine, as `WUAHandler.log (SRV01)`. This shows in its tab, the title bar and the sidebar, in a merged tab's details pane and exported File column, and on a folder's start page. Before, the same log open from several machines looked the same everywhere but the sidebar. A title too long for its tab now ends in "…", rather than being cut off mid-letter.

**Error Lookup**

- On a PC without CMTrace, such as one logs are copied to, 27 common ConfigMgr codes, such as 0x87D00324, get a short explanation in TraceView's own words, with `TraceView` as its source, rather than no text. The 21 named servicing codes, such as 0x800F0991, get one too. Any other ConfigMgr code says that a copy of `CMTrace.exe` beside `traceview.exe` gives its text, in `lookup`, summaries and the Error Lookup window. Windows' and CMTrace's own text still comes first wherever they have it.
- A ConfigMgr code with no text reads as a ConfigMgr code, rather than as a plain HRESULT.

**AI agents**

- The skill `traceview setup` writes tells agents it's for site server, Windows Installer, PowerShell transcript, IIS and Windows Firewall logs too, as agents choose a skill by its description. Run `traceview setup` again to update it.
- `traceview setup` also adds TraceView's skill for GitHub Copilot, in `%USERPROFILE%\.copilot\skills\traceview`, when `%USERPROFILE%\.copilot` exists. `traceview setup --remove` takes it back.

## 0.11.0

Following one ID through every log, the rows around each match, gaps where a PC logged nothing, ConfigMgr's warning signs that are only logged as info, Windows Update's `ReportingEvents.log` read as rows, and faster opens on network shares. Still a beta, tested on Windows 11.

**Reading logs**

- Windows Update's `ReportingEvents.log` is read as rows, in the viewer and the command line. Each row's time, with its milliseconds and offset from UTC, is in the Time column, and a `Failure` is an error. The operation, such as `Content Install`, is the component, the program that asked, such as `wa_3rd_party_host_32.exe`, the thread, and the event's name the source. Before, each whole row was the message, with no time. Indexes of logs of 64 MB or more are rebuilt once, the next time each is opened.
- Logs on network shares open with fewer round trips. A log of 16 MB or less, as nearly every ConfigMgr log is, is read in one go, rather than in four reads that fetched its start twice: an 845 KB log takes 1 read where it took 5, and 14% fewer bytes. Following a log live reads 12 KB per change to check it wasn't replaced, rather than 68 KB.

**Viewer**

- ▲ and ▼ buttons beside the Find box move to the previous and next match, as Shift+F3 and F3 do, and leave the cursor in the box.
- A Pause button at the end of the status bar pauses live updates, as Ctrl+U does, and shows Resume while paused.
- End in the log goes back to following new rows, and now ends a pause too.
- View > Fonts is now View > Fonts and row spacing. With row spacing selected, its buttons say Less space and More space, rather than the font buttons, which read as if they changed the font.
- Double-clicking in the Find, Filter or any other text box selects the word under the pointer, as in Windows' own text boxes.

**Command line**

- `traceview trace <id> <logs or folders>` follows one ID, such as a deployment, update or job GUID, through every log. It lists each row with that ID as one timeline, each with its log, and folds runs of the same message from one log into one line that says how many, as in `[2 more like it, to row 6 (08:53:18)]`. A header names the logs it's in. Before, this took `query --recurse --text <id>` and reading every repeat.
- `query --context N` shows the N rows before and after each match, from its own log, so there's no need to open the file to see what led up to an error. In lines, they're indented, `--` separates the groups, and matches close together share one. In JSON, each match gets `before` and `after` lists.
- Summaries list the longest gaps of a day or more with no rows, five at most, in time order. For a folder, that's no rows in any log, as when a PC was off or asleep for weeks, which before could only be found with `--count-by day`. In JSON, they're `gaps`, each with `from`, `to` and `days`.
- `summarize --symptoms` also looks for signs of trouble that ConfigMgr logs at info level, which problem patterns leave out, such as an update assignment `already in progress state`, a job `waiting for execution`, or a download `reconnected to the existing job`, and says what each can mean. Each was the clue to a real failure.
- `--unless-followed-by` says why nothing closed each match it finds. Either the same thing started again first, and whether that one finished, or it's still open at the end of its log. Before, an attempt that was abandoned and then retried successfully looked the same as one that hung. In lines, a label in brackets says which, and the header counts each. In JSON, it's `unclosed` and `next_match`, with the counts as `started_again` and `still_open`.
- `query --count-by template` gives each template its first row as an example, with its log and row number, so the versions, KB numbers and codes a template takes out can still be read. `7-Zip 26.04 (MSI-x64)` used to show only as `<n>-Zip <n>.<n> (MSI-x<n>)`. `--no-examples` leaves them out.
- `query --first-line` matches and shows only each row's first line, for SCCM entries that span many lines, such as WMI dumps, which used to match deep inside and flood results. Lines say how many were left out, as in `[+14 lines]`, and JSON has `more_lines`. It's off unless asked for.
- `--text` and `--regex` can be given together, or more than once, to find only the rows matching every one. Before, using both was an error.
- `--from` and `--to` take times without seconds, such as `2026-10-15T10:06`. As a `--to`, that takes in the whole minute.
- `--help` says that `--count-by` gives JSON unless `--format lines` is added.
- The AI agent skill mentions `--symptoms`, `trace`, `--first-line`, gaps, `--context`, giving `--text` more than once, and `--fields` and `--max-width` for keeping lines short. Run `traceview setup` again to update it.

**Error Lookup**

- On a ConfigMgr client or site server, ConfigMgr's own codes (0x87D0… and 0x87D1…) get text, such as "Item not found" for 0x87D00215, and many Windows Update codes get text beside their names. It comes, in English, from the copy of CMTrace that ConfigMgr installs, which TraceView reads as a file but never runs. `lookup`, summaries and the Error Lookup window all use it.
- On other PCs, such as one logs are copied to, a copy of `CMTrace.exe` in the folder holding `traceview.exe` gives the same text.
- 21 common servicing codes, 0x800F08xx and 0x800F09xx, get their names, such as `PSFX_E_MISSING_PAYLOAD_FILE` for 0x800F0991 and `CBS_E_SOURCE_MISSING` for 0x800F081F. Windows has no text for them, and they weren't explained at all before. The names come from Microsoft Learn.

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
