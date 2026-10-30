# Architecture

TraceView for Windows is a fast, customizable log viewer in the spirit of CMTrace. It's the Windows counterpart to the [macOS TraceView](https://github.com/thefinder808/TraceView), and it shares that app's behavior and spec but not its code.

**Status:** spike. The stack below was agreed on 2026-10-30, and the evidence for it is in [spike-results.md](spike-results.md). WinPE is out of scope for v1, because logs from WinPE are normally moved elsewhere to read. It can be revisited for v2, and a GDI renderer might be needed then.

## Goals, in priority order

1. **Never make someone wait.** Instant launch, multi-gigabyte files, no UI stalls.
2. **Run anywhere an admin needs it.** One copyable EXE, with no installer or runtime, and usable over RDP and on VMs with no GPU.
3. **Be very customizable.** Themes, columns, highlight rules and parsers.

## Current layout

```
crates/
  engine/      tv-engine: all log logic, with no UI dependency
    index.rs     parallel scan into one packed offset and one severity byte per row
    file.rs      opens a log and combines the map, index and cache
    cache.rs     on-disk index cache, keyed by path, size and modified time
    parse.rs     splits a row into fields, only when shown or filtered
    filter.rs    parallel regex filter over raw bytes
    examples/    gen_log (test data), mapped_file_behavior, cold_read
  app/         traceview: the spike's egui UI and scripted benchmark
  d2d-probe/   the same grid in Direct2D and DirectWrite, for comparison
scripts/       benchmark runners
docs/          this file and the spike results
```

`app` and `d2d-probe` are spike code. Neither is the final app.

## How a log is opened

1. Check the file. UTF-16 logs are rejected with a clear error for now.
2. Look for a cached index. A cache entry must match the file's path, size and modified time, and any mismatch or damage causes a rebuild.
3. Otherwise split the file into 16 MiB chunks and scan them in parallel. The chunk holding a row's first byte owns that row, so rows can safely span chunks.
4. Store 6 bytes per row in two compact arrays: a 5-byte file offset (up to 1 TiB) and a severity byte. Row text stays on disk.
5. Parse fields only when a row is displayed or filtered.

**Row format.** A row is a line in plain logs and a whole `<![LOG[...]LOG]!>` entry in SCCM logs, because SCCM messages can span lines.

**Filtering.** The `regex` crate runs directly on bytes, in parallel, without decoding to UTF-16. It guarantees linear-time matching, so a bad pattern can't hang the app. Results are row numbers in file order. Filtering runs on a background thread and can be cancelled.

**Grid.** Only visible rows are drawn. Scroll position is an `f64` in rows, because `f32` pixel offsets lose accuracy on files this large.

## Decisions

| Decision | Choice | Why |
|---|---|---|
| Language | Rust | Regex on raw bytes, no GC pauses, no runtime to ship |
| Code sharing with the Mac app | None | Mac app logic and views share one library, so port from its spec and behavior |
| File access | Positional reads (planned) | Memory maps block a writer's truncate and were slower cold. The spike engine still maps |
| Text | DirectWrite | Windows's own text stack: system fonts, ClearType, shaping and fallback |
| Drawing | Direct2D behind a renderer boundary | About 80x faster than egui without a GPU, 49 ms launch, about 3.8 ms at 4K. The boundary keeps the choice below reversible |
| Presenting frames | To be measured | The spike's HWND render target is the simple path. Flip-model swap chains support dirty and scroll rectangles, which Microsoft says help most over Remote Desktop, but they need a D3D11 device, and GPU device setup cost about 190 ms of launch in the probe |
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

### Costs of the UI recommendation

Direct2D draws only what we tell it to, so these all become our work:
- Tabs, menus, sidebar, settings and dialogs
- Text input with IME support through the Text Services Framework
- Accessibility through UI Automation, which must be built deliberately rather than added later (AccessKit is worth evaluating)
- High-DPI handling

The upside is that no framework limits theming or layout on anything we draw ourselves, which directly serves the customization goal.

## Later direction: AI-agent access

Not part of the spike, and nothing here is built yet beyond the rules under "Rules for now".

The goal is one engine with several front ends in the same EXE:

1. **Headless CLI with JSON output** (`traceview query`, `traceview summarize`), shipped with the first engine milestone.
2. **Local stdio MCP server** (`traceview mcp`), a thin layer over the CLI's functions. Tools:
   - `open_log`
   - `summarize`: groups repeated messages into patterns with counts, first and last seen, and spikes
   - `query`: always paged and capped
   - `get_context`, `lookup_error`, `correlate`, `tail`
3. **Links between tech and agent.** Findings cite a file and line, and a `traceview://` link or `traceview open --line` jumps there in the GUI. A "Copy for AI" action packages selected rows with context, format and looked-up error codes.

### Rules for now

- The engine crate must not depend on egui or any UI code.
- Query and result types must be serializable with `serde`, so the CLI and MCP server can reuse them. `Format`, `Level` and `OpenStats` already are. The real query and result types get designed after the spike.

### Security requirements for that work

- Log text is untrusted and can contain prompt injection, so tool results must mark it as data.
- The server is read-only over logs.
- Redaction of sensitive data is available as an option.
- Stdio only, with no network listener.
- Access is limited to folders the tech allows.
- Agent control of the GUI is opt-in.

## Open questions

- Real RDP behavior with a visible session (`scripts/run-visible-check.ps1`).
- UTF-16 log support and live tail design, both of which the read-based engine must handle.
