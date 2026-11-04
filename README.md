# TraceView for Windows

A fast, customizable Windows log viewer inspired by CMTrace, and the counterpart to the [macOS TraceView](https://github.com/thefinder808/TraceView).

**Early days.** It can stand in for CMTrace, and most of TraceView's own features are in: severity chips, the event histogram, the sidebar, bookmarks, filter presets, split view, the command palette and five themes. So is customization: highlight rules, columns, fonts, shortcuts, custom log formats, theme files, named layouts, and settings that load as they're edited. See [the GUI design](docs/GUI-DESIGN.md) and [the architecture](docs/ARCHITECTURE.md).

## Build

Needs Rust and the Visual Studio C++ build tools, with the Windows SDK they include for its resource compiler.

```bash
cargo build --release -p traceview
```

```bash
cargo test --workspace
```

For ARM64 PCs, CI builds a native EXE on every push, kept as the `traceview-arm64` artifact. To build it yourself, add Visual Studio's ARM64 C++ build tools, then:

```bash
cargo build --release --target aarch64-pc-windows-msvc -p traceview
```

## Try it

Open a log in the viewer, or start it empty and press Ctrl+O or drop files on the window:

```bash
target/release/traceview.exe path/to/some.log
```

| Keys | Action |
|---|---|
| Up, Down, Page Up, Page Down, Home, End | Move the selection |
| Shift with any of those, or Shift+click | Extend the selection |
| Ctrl+Up, Ctrl+Down, mouse wheel | Scroll without moving the selection |
| Ctrl+A, Ctrl+C | Select all, copy as tab-separated text |
| Ctrl+O | Open logs, each in its own tab |
| Ctrl+Tab, Ctrl+1-9, Ctrl+W | Switch tabs, close a tab |
| Ctrl+F, F3, Shift+F3 | Find, and move between matches |
| Ctrl+Shift+F | Filter, as in `failed level:warning component:PolicyAgent` or `/regex/` |
| Ctrl+E, Ctrl+Shift+E | Next and previous error |
| Ctrl+L | Error Lookup |
| Ctrl+H | Highlight rules: color or mark rows that match, as in `download failed level:error` |
| Drag a header, or its edge | Move or resize a column. Right-click the header, or use View > Columns, to show and hide them |
| Ctrl+Plus, Ctrl+Minus, Ctrl+0, Ctrl+wheel | Larger, smaller or default log text. View > Fonts picks fonts and row spacing |
| Ctrl+K | Command palette: every command, open tab, recent log and preset |
| Ctrl+G | Go to a row |
| Ctrl+D, F2, Shift+F2 | Toggle a bookmark, then step through them |
| Right-click a row, Shift+F10 | The row's menu: add or remove its bookmark, or copy the selected rows |
| Ctrl+B | Sidebar |
| Ctrl+\ | Split view, with Ctrl+Shift+S to sync the panes by time |
| Ctrl+U | Pause live updates |
| Ctrl+Shift+M | Merge the open logs into one timeline |
| Alt or F10 | The menus |

Every shortcut here can be changed, and any command can be given one, in Tools > Keyboard shortcuts.

Logs TraceView doesn't know can be taught to it in Tools > Log formats, with a regex whose named groups say where the time, level, component, thread, source and message are.

View > Layout: CMTrace hides everything but the grid and details pane, and Layout: Everything shows every pane. View > Save layout as... names the panes you have open, to switch back to later.

Tools > Use TraceView for .log files... lets double-clicked `.log` and `.lo_` files open in TraceView, for your Windows account only. Windows contrast themes are followed whenever they're on.

Settings live in `traceview.toml` and themes in the `themes` folder beside it, in `%APPDATA%\TraceView`, or beside the EXE for a portable copy. Both can be edited by hand while TraceView runs, and load within a second.

View > Edit theme changes any color of the theme in use, with the window recoloring as you type. Editing a built-in theme makes a copy, saved as a small file in the `themes` folder beside `traceview.toml`. Copy a file into that folder to share a theme.

Search a log from the command line, with results as JSON:

```bash
target/release/traceview.exe query path/to/some.log --text "failed" --min-level warning --limit 20 --pretty
```

Summarize a log's severity counts, repeated messages and error spikes:

```bash
target/release/traceview.exe summarize path/to/some.log --pretty
```

Run `traceview --help` for every option.

## Checks

These need generated 5 GiB logs, which `check-perf.ps1` creates in the git-ignored `test-data/perf` folder.

```bash
pwsh -File scripts/check-perf.ps1
```

```bash
pwsh -File scripts/check-gui-perf.ps1
```

```bash
pwsh -File scripts/check-gui.ps1
```

## Releases

A `v*` tag builds, signs and drafts a release, as [docs/RELEASING.md](docs/RELEASING.md) describes. `pwsh -File scripts/build-release.ps1` makes the same zips locally, unsigned.

## License

MIT, in [LICENSE](LICENSE). The licenses of the crates TraceView is built with are in [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt), and `traceview --licenses` prints both.
