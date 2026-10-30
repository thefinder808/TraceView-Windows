# TraceView for Windows

A fast, customizable Windows log viewer inspired by CMTrace, and the counterpart to the [macOS TraceView](https://github.com/thefinder808/TraceView).

**This repo is a technology spike, not a usable app yet.** See [the spike results](docs/spike-results.md) and [the architecture](docs/ARCHITECTURE.md).

## Build

Needs Rust and the Visual Studio C++ build tools.

```bash
cargo build --release --workspace --examples
```

```bash
cargo test --workspace
```

## Try it

```bash
target/release/traceview.exe path/to/some.log
```

You can also drop a file on the window. Enter filters by text, and the Regex box switches to regular expressions.

## License

MIT
