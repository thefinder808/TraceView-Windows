# TraceView for Windows

A fast, customizable Windows log viewer inspired by CMTrace, and the counterpart to the [macOS TraceView](https://github.com/thefinder808/TraceView).

**There's no GUI yet.** The engine and a command-line tool are working. See [the architecture](docs/ARCHITECTURE.md) for how it works and where it's going.

## Build

Needs Rust and the Visual Studio C++ build tools.

```bash
cargo build --release --workspace --examples
```

```bash
cargo test --workspace
```

## Try it

Search a log, with results as JSON:

```bash
target/release/traceview.exe query path/to/some.log --text "failed" --min-level warning --limit 20 --pretty
```

Summarize a log's severity counts, repeated messages and error spikes:

```bash
target/release/traceview.exe summarize path/to/some.log --pretty
```

Run `traceview --help` for every option.

## Performance checks

This generates two 5 GiB test logs in the git-ignored `test-data/perf` folder if they're missing, then fails if the engine has slowed past its limits:

```bash
pwsh -File scripts/check-perf.ps1
```

## License

MIT
