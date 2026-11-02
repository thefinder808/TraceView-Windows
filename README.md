# TraceView for Windows

A fast, customizable Windows log viewer inspired by CMTrace, and the counterpart to the [macOS TraceView](https://github.com/thefinder808/TraceView).

**Early days.** It can stand in for CMTrace: tabs, live tail, filter, find, Error Lookup and merged logs all work. TraceView's own features, such as charts and bookmarks, come next. See [the GUI design](docs/GUI-DESIGN.md) and [the architecture](docs/ARCHITECTURE.md).

## Build

Needs Rust and the Visual Studio C++ build tools.

```bash
cargo build --release -p traceview
```

```bash
cargo test --workspace
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
| Ctrl+U | Pause live updates |
| Ctrl+Shift+M | Merge the open logs into one timeline |
| Alt or F10 | The menus |

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

## License

MIT
