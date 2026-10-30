# Architecture

TraceView for Windows is a fast, customizable log viewer in the spirit of CMTrace. It's the Windows counterpart to the [macOS TraceView](https://github.com/thefinder808/TraceView), and it shares that app's behavior and spec but not its code.

**Status:** the engine and headless CLI are built (milestone 1). There's no GUI yet, only a grid probe used for rendering tests. The stack was agreed on 2026-10-30, and the evidence for it is in [spike-results.md](spike-results.md). WinPE is out of scope for v1, because logs from WinPE are normally moved elsewhere to read. It can be revisited for v2, and a GDI renderer might be needed then.

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
    index.rs      parallel scan into compact per-row arrays
    cache.rs      on-disk index cache
    file.rs       an open log: row reading and following a live file
    query.rs      search, with serializable query and result types
    summary.rs    severity counts, repeated message patterns, error spikes
    examples/     gen_log (test data), evict_cache (cold-disk testing)
    tests/        robustness (random input), perf_budgets (speed limits)
  traceview/    the EXE: headless CLI today, GUI and MCP server later
  d2d-probe/    the log grid in Direct2D, for rendering tests. Not the final GUI
fuzz/           coverage-guided fuzz targets, run on Linux in CI
scripts/        check-perf.ps1, run-presentation-test.ps1
```

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

**Timestamps.** Stored as wall-clock time as the log wrote it. Time zones are read but not applied, so times only compare correctly within one zone. Formats with no year take it from the file's modified date. That matters for correlating across files, which isn't built yet.

**Filtering.** The `regex` crate runs directly on bytes, in parallel, and guarantees linear-time matching, so a bad pattern can't hang the app. Level and time filters use only the index, with no file reads. Scans can be cancelled.

**Long rows.** Rows are read up to 1 MiB and results flag anything cut off, so one enormous row can't exhaust memory. Messages in results are capped at 4,096 characters.

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
| Anything else | Bare text | Severity from keywords |

Not yet supported, from testing against real logs and the Mac app's list: JSON lines, CSV, and `setupapi.dev.log`'s `!` and `!!!` severity markers and section timestamps. `Error Control = 1` in that log also reads as an error today. `DirectX.log`'s `MM/DD/YY` dates aren't parsed, so its rows have no time.

## Command line

```bash
traceview query app.log --regex "error 0x[0-9a-f]{8}" --min-level warning --limit 50
```

```bash
traceview summarize app.log --top 20
```

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
| Index memory | 257 MB | 456 MB | 10 bytes per row |

The CLI EXE is 1.9 MB and needs only DLLs that ship with Windows. A cold open can't beat the disk's read speed, so slower drives will take longer.

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

- **Every push:** formatting, lint with warnings as errors, and all tests, in GitHub Actions on Windows.
- **Robustness:** random and log-shaped bytes go through the whole pipeline on every test run. Set `PROPTEST_CASES` for longer runs.
- **Fuzzing:** coverage-guided on Linux in CI, because `libFuzzer` doesn't link with the MSVC toolchain here. The logic it covers isn't platform-specific, except for file access.
- **Performance:** `scripts/check-perf.ps1`, run locally, because shared CI machines are too variable for timing limits.

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

- **GUI and CLI in one EXE.** Windows treats an EXE as either a console or a GUI program. A console EXE flashes a console window when launched as a GUI, and a GUI EXE doesn't make a shell wait for it or show its output. Options include a `.com` console stub beside the EXE, as Visual Studio does, or attaching to the parent console. This needs deciding when the GUI starts, and it affects the single-EXE goal.
- **A real no-GPU VM and slow RDP links** are still untested.
- **Live file watching.** `refresh` detects changes when called. What triggers it (`ReadDirectoryChangesW` or a timer) is decided with the GUI's live tail.
- **Error-code database.** The Mac app's is built around macOS domains. Windows needs HRESULT, Win32, NTSTATUS and SCCM codes.
- **Time zones,** which correlating across files will need.
- **Index memory.** 10 bytes per row is simple, but timestamps could be packed further if large logs need it.
