# Architecture

TraceView for Windows is a fast, customizable log viewer in the spirit of CMTrace. It's the Windows counterpart to the [macOS TraceView](https://github.com/thefinder808/TraceView), and it shares that app's behavior and spec but not its code.

**Status:** the engine, headless CLI and foundations of the GUI are built (milestones 1 and 2). The GUI opens a log and shows it in a fast, accessible grid, but it has no live tail, search or tabs yet. The stack was agreed on 2026-10-30, and the evidence for it is in [spike-results.md](spike-results.md). WinPE is out of scope for v1, because logs from WinPE are normally moved elsewhere to read. It can be revisited for v2, and a GDI renderer might be needed then.

## Goals, in priority order

1. **Never make someone wait.** Instant launch, multi-gigabyte files, no UI stalls.
2. **Run anywhere an admin needs it.** One copyable EXE, with no installer or runtime, and usable over RDP and on VMs with no GPU.
3. **Be very customizable.** Themes, columns, highlight rules and parsers.
4. **Never get in the way of the system being diagnosed.** Opening a log must not affect the app writing it.

## Layout

```
crates/
  engine/       tv-engine: all log logic, with no UI dependency and no unsafe code
    reader.rs     positional file reads and change detection
    encoding.rs   UTF-8, UTF-16 and Windows-1252 handling
    level.rs      severity levels and the keyword fallback
    time.rs       timestamp parsing
    parse.rs      splits one row into fields, for every supported format
    custom.rs     formats defined by a regex, and choosing one for a file
    index.rs      parallel scan into compact per-row arrays
    cache.rs      on-disk index cache
    file.rs       an open log: row reading, and live refresh split into prepare and apply
    query.rs      search, with serializable query and result types
    merge.rs      the merged timeline of several logs
    overview.rs   counts by level and over time, for the severity chips and histogram
    summary.rs    severity counts, repeated message patterns, error spikes
    examples/     gen_log (test data), evict_cache (cold-disk testing)
    tests/        robustness (random input), perf_budgets (speed limits)
  tv-ui/        our toolkit: geometry, the Canvas interface, theme tokens and five themes, input,
                scroll bar, text field, menu bar, context menu, tab bar, list view, splitter, chip bar,
                command palette with fuzzy matching. No unsafe, no Windows
  tv-render/    Canvas on Direct2D and DirectWrite, through a flip-model swap chain on WARP
  tv-platform/  Win32: window, message loop, input, DPI, clipboard, dialogs, UI Automation, console handling,
                contrast themes, per-user file type registration
  tv-errors/    Error Lookup: finds codes in text and asks Windows what they mean. Its only unsafe code is in messages.rs
  traceview/    the EXE. No unsafe
    cli.rs        query and summarize
    gui/          app (connects the window and renderer to the workspace)
                  workspace (menus, tabs, focus, layout, input routing)
                  tab (one log or a merged set: filter, find, details, live tail)
                  log_view (grid), row_cache (background row loading), details (pane under the grid)
                  search (filter syntax, and matches kept current as logs grow), watch (live tail)
                  sidebar and places (open tabs, bookmarks, nearby logs, Windows log folders, recent logs)
                  overview (severity chips and histogram), command_palette (Ctrl+K, Go to row, presets)
                  rules (highlight rules) and rules_panel (their editor, Ctrl+H)
                  columns (per-format column layouts) and columns_panel (their editor)
                  fonts (fonts, sizes and row spacing) and fonts_panel (their editor)
                  keymap (shortcuts) and shortcut_panel (waits for a new one's keys)
                  formats_panel (the log formats editor)
                  themes (theme files) and themes_panel (the theme editor)
                  layouts (named arrangements of panes), reload (live reload of edited files)
                  settings (traceview.toml), export (CSV)
                  lookup (Error Lookup panel), accessibility (screen reader tree)
                  snapshots (whole-window pixel tests), bench (scripted checks, feature-gated)
  d2d-probe/    an earlier grid, kept only for run-presentation-test.ps1
fuzz/           coverage-guided fuzz targets, run on Linux in CI
scripts/        check-perf.ps1 (engine budgets), check-gui-perf.ps1 (GUI budgets),
                check-gui.ps1 (end to end), run-presentation-test.ps1 (RDP delivery),
                make-icon.ps1 (the icon, from the Mac app's), third-party-notices.ps1,
                build-release.ps1 (release zips, which release.yml signs)
licenses/       license files for crates published without them
docs/images/    README screenshots, drawn by the snapshot tests
LICENSE, THIRD-PARTY-NOTICES.txt, deny.toml (license and advisory rules)
CHANGELOG.md, SECURITY.md (how to report a vulnerability)
```

The design for the GUI, and how it was measured, is in [GUI-DESIGN.md](GUI-DESIGN.md).

## How a log is opened

1. Read the start of the file to find its encoding and format.
2. Look for a cached index:
   - **Hit:** path, size and modified time all match, so it's used as is.
   - **Extended:** the file grew and its fingerprint still matches, so only new rows are scanned.
   - **Miss:** anything else, including any damage to the entry, causes a full scan.
3. Otherwise split the file into 16 MiB chunks and scan them in parallel. The chunk holding a row's first byte owns that row, so rows can safely span chunks.
4. Store 10 bytes per row in three compact arrays: a 5-byte file offset (up to 1 TiB), a severity byte, and a 4-byte timestamp. Row text stays on disk.
5. Parse fields only when a row is displayed, searched or summarized.

## Key rules

**File access.** The engine uses positional reads, never memory maps. While a file is mapped, a writer can't truncate or shrink it (OS error 1224), and a viewer must not break the app writing a log. A test enforces this. Reads were also about 40% faster than mapping on a cold disk, though warm text searches are about twice as slow because bytes get copied.

**Rows.** A row is a line in plain logs and a whole `<![LOG[...]LOG]!>` entry in SCCM logs, because SCCM messages can span lines. An SCCM row number can therefore differ from a text editor's line number, so results also report the byte offset.

**Encodings.** UTF-8 and UTF-16 (both byte orders, with or without a BOM) are recognized. UTF-16 rows split only on real newlines, never on a stray `0x0A` byte. Text that isn't valid UTF-8 displays as Windows-1252, though searches on those files only match ASCII reliably.

**Severity.** Structured levels win: SCCM's `type` attribute, or a stated `ERROR:` or `[WARN]`. Otherwise whole-word keywords decide, using the same lists as the Mac app. One deliberate exception: CBS and DISM lines can be raised above their stated level, because CBS writes real failures at `Info`. Severity comes from the first 64 KiB of a message, and from at most about 1 MiB of a row while indexing.

**Timestamps.** Stored as wall-clock time as the log wrote it. Time zones are read but not applied, so times only compare correctly within one zone. Formats with no year take it from the file's modified date. Merging relies on this, so files written in different time zones won't line up.

**Filtering.** The `regex` crate runs directly on bytes, in parallel, and guarantees linear-time matching, so a bad pattern can't hang the app. Level and time filters use only the index, with no file reads. Scans can be cancelled.

**Long rows.** Rows are read up to 1 MiB and results flag anything cut off, so one enormous row can't exhaust memory. Messages in results are capped at 4,096 characters.

**Live tail.** A watcher thread per file checks it every 250 ms by opening it and reading its size, which also works on network shares and on files a writer holds open, where change notifications are late or missing. It reads and indexes new rows itself (`prepare_refresh`), then the UI thread puts them in place (`apply_refresh`, about 15 µs) only if it can take the write lock at once. A search holding the log just delays the update to the next check, so the window never waits. The path is reopened on each change, so SCCM's rename-and-recreate rotation shows the new file. Filters and finds then search only the changed rows, and cached rows from the change on are dropped, with a generation number so a block read before the change is never stored after it.

**Merging.** A merged tab orders rows from several logs by time. A row with no time, or one earlier than the row before it, keeps its place after that row, so multi-line output and clock jumps stay together. Each merged row names its file and row, and the grid, filter, find, copy and details all work through that. When live tail changes a file, the timeline is merged again in the background.

**Error codes.** Messages come from Windows at run time through `FormatMessageW`: the system table, plus message tables in `ntdll.dll`, `winhttp.dll` and `qmgr.dll`, loaded as data from full System32 paths so none of their code runs. ConfigMgr codes use `srsresources.dll` from an installed console, which is never bundled. No message text is copied into TraceView, so it's in the user's language and matches their Windows. Codes Windows has no text for, such as Windows Update's, still show their hex, signed and facility forms.

**Settings.** One TOML file, `traceview.toml`, holds the theme, layout, recent logs, filter presets, bookmarks, highlight rules, column layouts, fonts, changed shortcuts, custom log formats and named layouts. Themes are files of their own, beside it. It lives in `%APPDATA%\TraceView`, unless a `traceview.toml` sits beside the EXE, which makes that copy portable. It's read before the first frame, so the window opens in the right theme, again whenever it's edited elsewhere (see live reload), and saved on a background thread because `%APPDATA%` can be a network share. Each save writes a new file and swaps it in, so a crash never leaves half a file. A file with a mistake gives the defaults and a message, and it's never overwritten, so the mistake can be fixed by hand, and it loads as soon as it's fixed. Lists are capped, so a hand-edited or shared file can't make the menus unusable.

**Severity chips and histogram.** Both cover the whole log and come from the index alone: counting 26.9M rows and a histogram takes about 17 ms. They're counted on a background thread when a log opens and at most once a second during live tail. Chips filter to one exact level, which the engine's query gained for them. The histogram places each untimed row at the time of the row before it, as merging does, so its buckets add up to the chip counts, and it keeps the shadows of earlier spikes as a live log's range stretches, as the Mac app does.

**Highlight rules.** A rule is filter box text, such as `download failed level:error`, with a color and a style: the whole row, or a marker beside the row number that leaves the severity color alone. Rules run in order and the first match wins. They're checked as the row cache reads rows, using the engine's matcher on one row at a time, so they never scan the log: 10 rules add about 0.1 ms per 128 rows read, on the cache's thread. Changing a rule re-reads only the rows on screen, and they keep their old colors until then, so nothing blanks out while a pattern is typed. Colors come from six per-theme palette entries (row, text and marker shades), and a test holds each one to contrast limits in every theme, including markers on CMTrace's red and yellow rows. Find matches and the selection draw over a rule's color.

**Columns.** Each format keeps its own list of columns in `traceview.toml`: Time, Level, Component, Thread, Source (the file an SCCM row names) and Message, each with an order, a width and whether it shows. A merged tab uses its first file's. Widths are in characters of the grid font, so they'll still fit when fonts become adjustable. The row number always comes first, because bookmark and rule markers sit beside it, and the message always shows. The last shown column runs to the grid's edge. The grid takes its columns from the workspace each frame, except during a header drag, when it keeps its own copy so the drag can preview live, and the result is saved when the mouse is released. A file with odd entries still gives every column once: unknown names are skipped, repeats ignored, widths clamped to 3–400, and anything left out goes at the end as it would by default. Copying rows and the screen reader's row text follow the shown columns in order, while CSV export always writes every field.

**Fonts.** The log grid and the rest of the window each have a font family and size in `traceview.toml`, and the grid has a row spacing. Families are any installed font, listed through DirectWrite's system collection with fixed-width ones marked. Rows are the font's measured line height plus the spacing's padding (1, 3 or 8 pixels), so they always fit their text, and the grid's header and the status bar grow the same way. Log text runs from 8 to 28 pixels. Window text stops at 15, because its bars have fixed heights, and Windows' display scale covers larger. The renderer replaces a family that isn't installed, as when a portable copy moves to another PC, with Consolas or Segoe UI rather than letting Windows pick a font that may not be fixed width, and it checks only when the fonts change, so launch stays at 49 ms.

**Custom formats.** A format a person defines is a name, a regex with named groups (`time`, `level`, `component`, `thread`, `source`, `message`), optional file names with `*` and `?`, and whether dates are day first. Opening a log picks one: a custom format whose file names match comes first, then SCCM, then a custom format whose pattern matches at least 80% of the first 20 lines, then plain text. Each line is a row. A line the pattern doesn't match is shown whole, with no time and its level from its words. The `time` part understands ISO dates, syslog's `Jan 31 10:22:01`, and dates with the year last, as `01/31/2026 1:22:01 PM`. Indexing runs the regex on every line, in parallel, which takes about 2.7 s for the 5 GiB plain log where the built-in reader takes 1.3 s. Patterns run in the `regex` crate's linear-time engine with a 1 MiB size limit, so a shared definition can't hang a scan or use much memory. The index cache records a key for the pattern (version 3 of the cache added it), so an index made with an older pattern is never reused. The CLI reads the same formats from `traceview.toml`.

**Theme files.** Every color a theme has is a named token, such as `grid_background`, and the six rule colors each have row, text and marker shades. A theme file is TOML in a `themes` folder beside `traceview.toml`, so a portable copy carries its themes. It names a built-in theme to start from (or says whether it's dark) and lists only the colors it changes, so a shared one can be a few lines. Files are read at launch and whenever one changes, at most 50 of up to 64 KiB each, and one with a mistake, an unknown color name, or another theme's name is skipped with a message rather than stopping the others. Views take themes as `&'static`, as the built-in ones are, so a theme read from a file, or each version of one being edited, is kept for as long as the app runs. Each distinct version is kept once, which costs a few hundred bytes per change. The editor never changes a built-in theme: the first change makes a copy and saves it, and every later change is saved at once. Renaming a theme renames its file, and a name is refused if another theme has it, or if its file name matches any file in the folder, even one with a mistake, so nothing is written over. A test holds every built-in theme to WCAG contrast limits (4.5 to 1 for text, 3 to 1 for bars and the bookmark marker), and the editor uses the same check to point out weak pairs in a theme being edited, without refusing them. The title bar is recolored whenever the theme's values change, not only its name.

**Live reload.** A thread checks `traceview.toml` and the themes folder every second, by size and modified time only, as live tail does, since change notifications from network shares are unreliable. When the settings file changes, it's read and compared with the text TraceView last read or wrote, so its own saves never come back as reloads, and parsed on that thread. The window then puts the result in place as it does at launch: theme, panes, rules, columns, fonts, shortcuts, presets, layouts and bookmarks. Changed log formats apply to logs opened afterwards, as with in-app edits. An editor of settings that's open is closed, since what it shows is out of date. A file with a mistake keeps the settings in use and says what's wrong, and fixing it loads it, even when the mistake was there at launch. A deleted file is left alone, and the next save writes it again.

The edited file wins over the window. The writer thread knows the text the file held when it was last read or written, and before each save it checks the file still holds that. If it doesn't, the file was edited elsewhere and not yet loaded, so the save is dropped and the edit is loaded instead. The window tells the writer it has loaded new text through the same queue saves use, after any save sent before it, so the window never waits on the file, even on a slow share. The new text is written beside the file before the check, so the check comes just before the swap. An edit saved elsewhere in the instant between the check and the swap would still be replaced, which no file API can rule out without the other program's help.

Theme files reload as a set when any of them changes. The menu lists them again, and the theme in use updates if its file changed, except while the theme editor is open, since it decides the theme then. The editor's own saves are read back too, which is harmless because each distinct theme is kept once. Problems with theme files are said only when they change, so a broken file isn't announced on every keystroke in the editor.

**Layouts.** A layout is a name and whether the sidebar, severity chips, histogram and details pane show, with the sidebar's width. CMTrace (the grid and details pane) and Everything are built in, and saved ones follow them in `traceview.toml`, up to 50. Each is a menu command numbered from `command::LAYOUTS`, so the View menu, the palette and screen readers all get them the way they get themes, and the built-in two have stable shortcut names (`layout_cmtrace`, `layout_everything`). Nothing records which layout is in use: the menu checks every layout that matches what the window shows now, so changing a pane by hand simply unchecks it. The status bar isn't part of a layout, since it carries row counts, live state and every message. The sidebar's width is saved as the width chosen, not the width it's laid out at, because a sidebar not yet shown has no width, and a narrow window shrinks it only for as long as it's narrow.

**File types.** Registering for `.log` and `.lo_` writes only under `HKEY_CURRENT_USER`: a `TraceView.Log` file type whose open command is this EXE's path, quoted, with `"%1"`; an entry in each extension's `OpenWithProgids`, so other programs listed there stay; and capabilities under `Software\TraceView` named in `RegisteredApplications`, which is what puts TraceView on Windows' Default apps page. Windows keeps the default choice itself, behind a hash a program can't forge, so TraceView registers and then opens that page. Taking it back removes exactly those keys and values. The window runs both as commands and reports back, so the workspace and its tests never touch the registry, and the registry code's own tests write under a scratch key they remove.

**Contrast themes.** The window reads whether a Windows contrast theme is on, and its six colors, at launch and whenever Windows says settings or system colors changed. While one is on, the workspace draws with a theme made from those colors in place of the chosen one, kept once per set of colors as file themes are. Its backgrounds are all the background color and only highlights are filled, so every pair a view draws is one the contrast theme defines, and a test holds the mapping to the same contrast limits as the built-in themes for palettes like Windows' own four. A `contrast` flag on the theme does the rest: text on a highlight takes the selected-text color through `Theme::on_highlight`, rules draw as markers, find matches are framed, and the title bar is left to Windows.

**The EXE's resources.** `build.rs` compiles one resource file into the `traceview` binary with the Windows SDK's `rc.exe`, through the `winresource` crate: the manifest (per-monitor DPI, long paths and the console policy), the icon, and version info whose numbers come from `Cargo.toml`. The icon is the Mac app's, rebuilt by `scripts/make-icon.ps1`: sizes the Mac set has are used as they're drawn, and the in-between sizes Windows wants are scaled from the next larger one. The window class and a registered `.log` file both point at it, as resource 1.

**Licenses.** TraceView is MIT. `THIRD-PARTY-NOTICES.txt` holds the license files of every crate built into the EXE for x64 and ARM64, made by `scripts/third-party-notices.ps1` from `cargo metadata`, with each distinct text once. The script fails if a crate has no license file. Crates published without theirs, as AccessKit's are, use copies from the repository at the released tag, kept in `licenses/`. The EXE carries both files, so `traceview --licenses` shows them wherever it's copied. CI checks the notices are current, and `cargo deny` fails on a license not in `deny.toml`, a crate with a RustSec advisory, or a crate from anywhere but crates.io.

**Menus.** A menu taller than the window wraps into more columns, as Windows' own can, so long lists of themes and layouts stay reachable. Items are found by their rectangles, so clicks, hovering and screen reader bounds needed no change.

**Shortcuts.** Every command has a stable name, such as `find`, and a default shortcut, and `traceview.toml` holds only the ones changed, as `find = "Ctrl+J"` or `""` for none, so better defaults can still arrive later. A changed shortcut wins over a default with the same keys. Shortcuts run before text boxes see a key, so each needs Ctrl or a function key, and typing can never run a command. The keys Windows and the app use for editing (Ctrl+A, C, V, X, Y and Z), for picking a tab (Ctrl+1 to 9) and for the menus (Alt and F10) can't be taken, and neither can arrows and other movement keys, which the grid and boxes use with Ctrl. Plus and minus ignore Shift, since many keyboards need it to type them. Menus and the palette read every shortcut from the keymap, so they always show the current ones.

**CSV export.** Log text is untrusted, so any cell that starts like a spreadsheet formula (`=`, `+`, `-`, `@`) gets a leading apostrophe and stays plain text. Files are UTF-8 with a byte order mark, which Excel needs, and cells are capped at what Excel can hold. The file appears only once it's complete.

**Untrusted input.** Log files can contain anything. The engine forbids `unsafe` code, cached offsets are bounds-checked so a damaged cache can only produce wrong rows, and random-input tests and fuzzing check that nothing panics. The index cache holds offsets, levels, times and the log's path, never log text, and its size is capped at 2 GiB with the oldest entries removed first.

## Supported formats

| Format | Example | Notes |
|---|---|---|
| SCCM / CMTrace | `<![LOG[msg]LOG]!><time="..." ...>` | Multi-line entries are one row |
| CBS and DISM | `2026-04-23 14:25:00, Info  CBS  msg` | Windows servicing logs |
| Classic syslog | `Apr 23 14:25:00 host proc[1]: msg` | Year from the file's date |
| Dated syslog | `2026-03-08 13:46:47-07 host proc[1]: msg` | |
| Bracketed timestamp | `[2026-04-19 19:54:24.115] [info] msg` | |
| Prefix-tagged | `[VM] 2026-03-28 15:17:57 [info] msg` | |
| logfmt | `time="..." level=info msg="..."` | Message escapes are left as written |
| Timestamp first | `2026-04-23T14:32:01 [WARN] msg` | Slashed dates work too, as in `WindowsUpdate.log` |
| Custom | Defined by a pattern in `traceview.toml` or Tools > Log formats | One row per line |
| Anything else | Bare text | Severity from keywords |

Not yet supported, from testing against real logs and the Mac app's list: JSON lines, CSV, and `setupapi.dev.log`'s `!` and `!!!` severity markers and section timestamps. `Error Control = 1` in that log also reads as an error today. `DirectX.log`'s `MM/DD/YY` dates aren't parsed, so its rows have no time.

## Command line

```bash
traceview query app.log --regex "error 0x[0-9a-f]{8}" --min-level warning --limit 50
```

```bash
traceview summarize app.log --top 20
```

Filters: `--text`, `--regex`, `--case-sensitive`, `--min-level`, `--level` (exactly that level), `--component`, `--thread`, `--from` and `--to`.

Results are JSON on stdout. Errors are JSON on stderr, with exit code 1, or exit code 2 for incorrect usage. Pages are capped at 1,000 rows, and `next_offset` gives the next page. Output marks log text as untrusted (`"log_text_trust": "untrusted"`).

## Measured performance

Ryzen 9 5900X (24 threads), NVMe, 5 GiB logs with 26.9M (SCCM) and 47.8M (plain) rows. `scripts/check-perf.ps1` enforces limits scaled to file size, and they assume a warm disk.

| Operation | SCCM | Plain | Limit (warm) |
|---|---|---|---|
| First open, cold disk | 2.8 s | 3.0 s | Not enforced |
| First open, warm disk | 1.18 s | 1.30 s | 5 s |
| Reopen with cached index, warm | 0.05 s | 0.11 s | 0.5 s |
| Reopen with cached index, cold | 0.13 s | 0.24 s | Not enforced |
| Text or regex search | 0.85 s | 0.93 s | 2.5 s |
| Level filter | 3 ms | 5 ms | 0.5 s |
| Summary | 1.2 s | 1.5 s | 5 s |
| Read 60 visible rows, p99 | Under 0.5 ms | Under 0.5 ms | 2 ms |
| 100 checks of an unchanged live log | 3 ms | 3 ms | 50 ms |
| Merge two copies (10 GiB in all) | 2.2–2.4 s | 2.3–2.8 s | 0.5 s per GiB |
| Level counts and a 1,000-bucket histogram | 17 ms | 28 ms | 10 ms per GiB |
| Put a live refresh in place | 0.015 ms | | Not enforced |
| Index memory | 257 MB | 456 MB | 10 bytes per row |

The EXE, CLI and GUI together, is 2.6 MB and needs only DLLs that ship with Windows. A cold open can't beat the disk's read speed, so slower drives will take longer.

## Decisions

| Decision | Choice | Why |
|---|---|---|
| Language | Rust | Regex on raw bytes, no GC pauses, no runtime to ship |
| Code sharing with the Mac app | None | Mac app logic and views share one library, so port from its spec and behavior |
| File access | Positional reads | Memory maps block a writer's truncate and were slower cold |
| Text | DirectWrite | Windows's own text stack: system fonts, ClearType, shaping and fallback |
| Drawing | Direct2D behind a renderer boundary | About 80x faster than egui without a GPU, 49 ms launch, about 3.8 ms at 4K. The boundary keeps the choice below reversible |
| Presenting frames | Flip-model swap chain on a D3D11 WARP device | Measured over real RDP: 60 ms launch, about 1.5 ms per frame, no skipped frames. Same behavior on every machine, with no GPU driver dependency. If WARP can't be created, fall back to the HWND software target (untested) |
| GPU use | None in v1, possible later behind the renderer boundary | Hardware halves CPU while scrolling but adds about 190 ms to launch. One option is to start on WARP and move to hardware in the background |
| Scroll and dirty hints | Not used | No measurable benefit over RDP in testing, and they add complexity. Worth retesting on a slow link |
| Controls | Our own themeable toolkit, native only for OS dialogs | Full control of theming, with accessibility and input built in from the start |
| C runtime | Static | The EXE runs without the VC++ redistributable |

### Alternatives considered

| Option | Why not, for now |
|---|---|
| WinUI 3 | Best native look, with accessibility and IME included. Costs the single copyable EXE and limits theming. This is the one real alternative |
| Direct3D 11 glyph atlas (Windows Terminal, Zed) | Built for maximum throughput, but software drawing already fits the budget at 4K. Zed warns of "awful performance" on the software GPU. Can be added behind the renderer boundary if ever needed |
| egui | Failed the no-GPU test |
| Skia | Adds megabytes and offers no Windows-specific advantage over the system's own stack |
| GDI | Available nearly everywhere, but weaker for text and themed drawing. Only relevant if WinPE support is added later |

### Costs of the UI choice

Direct2D draws only what we tell it to, so these all become our work:
- Tabs, menus, sidebar, settings and dialogs
- Text input with IME support through the Text Services Framework
- Accessibility through UI Automation, which must be built deliberately rather than added later (AccessKit is worth evaluating)
- High-DPI handling

The upside is that no framework limits theming or layout on anything we draw ourselves, which directly serves the customization goal.

## Quality gates

- **Every push:** formatting, lint with warnings as errors, and all tests, in GitHub Actions on Windows. That includes real frames drawn into memory and checked pixel by pixel. An ARM64 EXE is also built, and kept for 14 days for testing on ARM64 PCs.
- **Robustness:** random and log-shaped bytes go through the whole pipeline on every test run. Set `PROPTEST_CASES` for longer runs.
- **Fuzzing:** coverage-guided on Linux in CI, because `libFuzzer` doesn't link with the MSVC toolchain here. The logic it covers isn't platform-specific, except for file access.
- **Performance:** `scripts/check-perf.ps1` and `scripts/check-gui-perf.ps1`, run locally, because shared CI machines are too variable for timing limits. The GUI benchmark sizes its window in physical pixels, not scaled by the display, so its budgets mean the same at any display scale.
- **End to end:** `scripts/check-gui.ps1`, run locally, drives the real window through UI Automation.
- **Releases:** a `v*` tag tests and builds with no access to signing, then a separate job in the `release` environment signs with Azure Artifact Signing through OpenID Connect, checks the signatures, and makes a draft release. Every workflow pins its actions to exact commits. See [RELEASING.md](RELEASING.md).

## AI-agent access

The goal is one engine with several front ends in the same EXE:

1. **Headless CLI with JSON output.** Built, as `traceview query` and `traceview summarize`.
2. **Local stdio MCP server** (`traceview mcp`), a thin layer over the CLI's functions. Not built. Tools:
   - `open_log`
   - `summarize`: groups repeated messages into patterns with counts, first and last seen, and spikes
   - `query`: always paged and capped
   - `get_context`, `lookup_error`, `correlate`, `tail`
3. **Links between tech and agent.** Not built. Findings cite a file and row, and a `traceview://` link or `traceview open --line` jumps there in the GUI. A "Copy for AI" action packages selected rows with context, format and looked-up error codes.

### Rules

- The engine crate must not depend on any UI code.
- Query and result types must be serializable with `serde`, so every front end shares them.

### Security requirements for the MCP work

- Log text is untrusted and can contain prompt injection, so tool results must mark it as data.
- The server is read-only over logs.
- Redaction of sensitive data is available as an option.
- Stdio only, with no network listener.
- Access is limited to folders the tech allows.
- Agent control of the GUI is opt-in.

## Open questions

- **The GUI** is designed in [GUI-DESIGN.md](GUI-DESIGN.md), including how one EXE serves both the CLI and GUI.
- **A real no-GPU VM and slow RDP links** are still untested.
- **ConfigMgr and Windows Update error text.** Windows has none for Windows Update codes, and ConfigMgr's needs a console installed, which hasn't been tested.
- **Time zones,** which merging files from different zones will need.
- **Index memory.** 10 bytes per row is simple, but timestamps could be packed further if large logs need it.
