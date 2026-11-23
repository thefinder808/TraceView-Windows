# Changelog

Each release's changes, newest first. Versions follow [Semantic Versioning](https://semver.org/).

## 0.15.1

Rows too long for the window now scroll sideways, so their ends can be read in the grid. Still a beta, tested on Windows 11.

**Reading logs**

- Rows too long for the window scroll sideways: with Shift and the mouse wheel, a tilting wheel or a touchpad, the Left and Right keys, or the scroll bar that appears under the rows. The row numbers stay put. Before, the end of a long row could only be read in the details pane or by widening the window.

## 0.15.0

Installing TraceView properly: one step adds it to the Start menu, puts it on your PATH, lets it open `.log`, `.lo_` and `.evtx` files and opens Windows' Default apps page to make it their default. And setup logs with blank lines between entries no longer show an empty row after each. Still a beta, tested on Windows 11.

**Installing**

- Install TraceView... installs it for your Windows account, from the start page of a copy run from somewhere else, from the Tools menu, or as `traceview install`. It copies TraceView to `%LOCALAPPDATA%\Programs\TraceView`, where it keeps itself up to date, adds it to the Start menu, so typing TraceView in Start opens it, adds it to your PATH, and lets it open `.log`, `.lo_` and `.evtx` files. Then Windows' Default apps page opens at TraceView, to make it their default, as Windows doesn't let a program do that itself. It says what it will change and asks first, and needs no administrator rights.
- Setting up for AI agents adds the Start menu shortcut too.
- Tools > Use TraceView for .log files... registers the installed copy when there is one, so double-clicking a log keeps working as updates replace it, and offers `.evtx` files too.
- Tools > Remove AI agent setup... is now Tools > Uninstall TraceView..., and with `traceview setup --remove` also takes back the Start menu shortcut and the file types. A shortcut that opens something else is left alone.

**Reading logs**

- A blank line is part of the row before it, rather than a row of its own. Some Windows setup logs, such as `setupact.log` with the compatibility appraiser's rows, showed an empty row after every row, from blank lines between entries or a line feed before each carriage return. Row numbers now count entries, not lines, in such logs. Indexes of logs of 64 MB or more are rebuilt once, the next time each is opened.

## 0.14.0

Windows event logs, in the viewer and the command line: saved `.evtx` files, including ones on other PCs' shares, and this PC's own logs, followed live, with the messages and column names Event Viewer shows. Still a beta, tested on Windows 11.

**Reading logs**

- The command line reads Windows event logs: a saved `.evtx` file, including one on another PC's share, or one of this PC's logs by the name Event Viewer gives it, such as `eventlog:System` or `eventlog:Microsoft-Windows-TaskScheduler/Operational`. Each event is a row with its time in this PC's time zone, its level, its provider as the component, its event ID as the thread and its computer as the source, with the message Event Viewer shows. So `query`, `summarize` and `trace` work on event logs as on any other, and can merge them with ConfigMgr's logs by time. The System log of 53,000 events on a 24-thread PC reads in under 2 seconds.
- Each event log is read through a temporary copy in `%TEMP%\TraceView`, which is deleted as soon as it's open, so logs such as Security don't stay behind in your profile.
- The AI agent skill says event logs can be read this way. Run `traceview setup` again to update it.

**Viewer**

- File > Open event log... lists this PC's event logs, such as System, Application and the ones Intune and Windows Update write, to open one by typing part of its name. A saved `.evtx` file opens like any log, from File > Open, which now offers `.evtx` files, or by dropping it on the window.
- One of this PC's event logs shows new events as they arrive, within a couple of seconds.
- Event logs' columns are named as in Event Viewer: Source, Event ID and Computer. The tab says how many events it has read while it opens.
- An event log's temporary copy is deleted when its tab closes.

## 0.13.1

The first release that 0.13's copies install by themselves. A log on a mapped network drive is now named with its machine. Still a beta, tested on Windows 11.

**Viewer**

- A log opened from a mapped network drive is named with its machine too. With `Z:` mapped to `\\SRV01\c$`, `Z:\Windows\CCM\Logs\WUAHandler.log` shows as `WUAHandler.log (SRV01)`, in its tab, the title bar and everywhere else a remote log's name shows. Before, only logs opened as `\\SRV01\...` were.

## 0.13.0

TraceView now keeps itself up to date. The copy that setup installs checks GitHub once a day and installs newer signed releases in the background, after checking each one the way Windows does and that it comes from the same publisher. Still a beta, tested on Windows 11.

**Updates**

- The copy that Tools > Set up for AI agents, or `traceview setup`, installs keeps itself up to date. Once a day, and from Help > Check for updates, the viewer asks GitHub for the latest release, and installs a newer TraceView in the background, which starts the next time you open it. Help > What's new shows what changed.
- A download is installed only once it matches the release's `SHA256SUMS.txt`, Windows trusts its signature, it's signed by the same publisher as the copy that's running, and the version inside it is the one listed. Only newer full releases are taken. The command line never checks, so a command an AI agent is running is never interrupted.
- A copy kept anywhere else is told about a new version, and Help > What's new in that version opens its page. A copy that isn't a signed release, such as one built from source, never checks.
- Help > Check for updates automatically turns the daily check off. An administrator can turn checks off for a PC, or one user, with a `DisableUpdates` DWORD of `1` under `SOFTWARE\Policies\TraceView`, as Group Policy and Intune can set.
- This is the first time TraceView connects to the internet. GitHub sees this PC's address and TraceView's version, and nothing else is sent. Checking a signature can also have Windows ask Microsoft, which issued the certificate, whether it was revoked. The README's Privacy section says so.

**Releases**

- Each release also has its two signed EXEs on their own, listed in `SHA256SUMS.txt`, for updates to download.

## 0.12.0

Many more of the logs admins open, read with their times and levels: Windows Installer, PowerShell transcripts, IIS, whose UTC times are shown in local time, Windows Firewall, Group Policy, Netlogon, ConfigMgr site servers and more. Tabs name the machine a remote log is on, and a selected last row follows a live log. Still a beta, tested on Windows 11.

**Reading logs**

- Windows Installer verbose logs, from `msiexec /l*v`, have times: each row's time of day on the date its `=== Logging started:` line gives. `MSI (s)` is the component, the process and thread the thread, and `Return value 3`, an action that failed, is an error. Before, no row had a time.
- PowerShell transcripts mark error records as errors, from Windows PowerShell and PowerShell 7, such as `Get-Item : Cannot find path…` and its `+ CategoryInfo` lines. Their `Start time:` and `Command start time:` lines have times. Before, only `WARNING:` and `VERBOSE:` had a level.
- IIS logs, such as `u_ex261118.log` from a ConfigMgr management or distribution point, and Windows Firewall's `pfirewall.log` are read by the columns their `#Fields:` line names. Each row has its time and the client's address. For IIS, the page asked for is the component, a 5xx status is an error and a 4xx a warning, except 401, which every Windows-authenticated request gets first. For the firewall, the action is the component, and `DROP` is a warning. Before, rows had a time but nothing else.
- IIS logs' times, which IIS writes in UTC, are shown in this PC's time zone, with its daylight saving time, as Event Viewer shows times. So IIS rows line up with ConfigMgr's and other logs' local times in a merged tab, `trace` and time filters, rather than sitting hours away. The command line gives each row's offset, such as `2026-11-18T08:05:09.000-06:00`, and the row's text still has the time IIS wrote. An IIS log from a server in another time zone is shown in this PC's. Indexes of IIS logs of 64 MB or more are rebuilt when the PC's time zone changes.
- Group Policy's debug log, `gpsvc.log`, has its time, its service, such as `GPSVC`, as the component, and its process and thread IDs as the thread, as do Group Policy's other debug logs written the same way. Windows 11 added dates to these logs in late 2025. Older rows give only a time of day, which can't be placed on a timeline, so it stays at the start of the message. Before, each whole row was the message.
- `netlogon.log` has its time, with the year from the file's date, its category, such as `LOGON`, as the component, and the thread some versions write. `[CRITICAL]` rows are errors, and a request that returns an NTSTATUS error, such as a failed logon's `Returns 0xC000006A`, is a warning. Before, each whole row was the message, with no time.
- Both were written from Microsoft's published examples, as their debug logging is off by default.
- ConfigMgr site server logs, such as `sitecomp.log` and `distmgr.log`, are read in the `$$<` format CMTrace also reads, with each row's time, offset from UTC, component and thread. Before, each whole row was the message, with no time.
- `WindowsUpdate.log`, as `Get-WindowsUpdateLog` writes it, has its component, thread ID and process ID in the Component, Thread and Source columns. Before, a colon early in a message made the IDs and the component read as one component, such as `51540 Misc UUS`, and summaries split one problem into several patterns because the IDs' widths vary.
- `setupapi.dev.log` goes by its `!!!` and `!` markers, so `Error Control = 1` is no longer an error, and a failed `[Exit status: FAILURE(…)]` is. Sections' starts and ends have times, and the short name before a colon, such as `dvi`, is the component.
- Office's Click-to-Run logs, such as `%TEMP%\<PC>-20261111-1324.log`, and SharePoint's, in the ULS format, have their time, level, category, thread and process. `Unexpected` is an error, and `Monitorable` and `High` are notices. Before, each whole row was the message, with no time.
- WiX Burn's logs, from the Visual C++ and .NET installers, and the Visual Studio installer's, such as `[0A1C:0B2C][2026-11-18T12:31:32]i001: msg`, have their time and thread, and Burn's `i`, `w` and `e` codes are their levels. Before, they had no time.
- winget's logs have their level and component, from the `<I> [CORE]` after each time.
- Indexes of logs of 64 MB or more are rebuilt once, the next time each is opened.

**Viewer**

- Selecting the last row of a log that's following new rows, by clicking it or with End, keeps the newest row selected as rows arrive, so the details pane shows each one. Before, the selection stayed on that row as it scrolled away. Any other selection stays where it is.
- The Filter and Find boxes show a × at their right end while they have text. Clicking it empties the box, which for Filter shows every row again, and leaves the cursor there for new text.
- The Pause button at the end of the status bar is now a square like the find buttons, showing the pause symbol, and ▶ while paused, rather than the words Pause and Resume.
- A log opened from another PC, such as `\\SRV01\c$\Windows\CCM\Logs\WUAHandler.log`, is named with its machine, as `WUAHandler.log (SRV01)`. This shows in its tab, the title bar and the sidebar, in a merged tab's details pane and exported File column, and on a folder's start page. Before, the same log open from several machines looked the same everywhere but the sidebar. A title too long for its tab now ends in "…", rather than being cut off mid-letter. The file name is shortened first, so a machine such as `(NYCLAP100234)` stays whole, since machines often differ only in their last digits.
- Tabs shrink to share the window's width when there are more than fit, widest first, as in a browser. Before, the later tabs ran off the window's right edge, where only Ctrl+Tab, Ctrl+K or the sidebar reached them. A tab never gets narrower than before, so a great many tabs can still run off the edge.

**Command line**

- Summaries count a template at each level apart, so a pattern's count is how many rows it had at its level. Before, a template that was only sometimes an error, such as a CBS row raised by its HRESULT, was one pattern at its worst level that counted every row, information included. IIS requests, whose status sets the level, would otherwise all have counted as errors.

**AI agents**

- The skill `traceview setup` writes tells agents it's for site server, Windows Installer, PowerShell transcript, IIS, Windows Firewall, Group Policy and Netlogon logs too, and for failed logons, as agents choose a skill by its description. Run `traceview setup` again to update it.
- `traceview setup` also adds TraceView's skill for GitHub Copilot, in `%USERPROFILE%\.copilot\skills\traceview`, when `%USERPROFILE%\.copilot` exists. `traceview setup --remove` takes it back.

**Error Lookup**

- On a PC without CMTrace, such as one logs are copied to, 27 common ConfigMgr codes, such as 0x87D00324, get a short explanation in TraceView's own words, with `TraceView` as its source, rather than no text. The 21 named servicing codes, such as 0x800F0991, get one too. Any other ConfigMgr code says that a copy of `CMTrace.exe` beside `traceview.exe` gives its text, in `lookup`, summaries and the Error Lookup window. Windows' and CMTrace's own text still comes first wherever they have it.
- A ConfigMgr code with no text reads as a ConfigMgr code, rather than as a plain HRESULT.

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
