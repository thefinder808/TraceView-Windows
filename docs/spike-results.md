# Spike results: Rust and egui for TraceView on Windows

Date: 2026-10-30. Branch: `spike/egui`.

## Verdict

**The Rust engine passed. egui failed for this product. Direct2D and DirectWrite is the recommended UI.**

egui is excellent with a GPU, but it runs at about 8 fps without one, while using every core. No-GPU VMs and RDP sessions are normal places for admins to read logs, so that rules it out as the only renderer. The same grid drawn with Direct2D's software renderer takes about 1.5 ms per frame on the same machine.

## Test machine

Ryzen 9 5900X (24 threads), 32 GB RAM, RTX 3080, Samsung 970 EVO NVMe, Windows 11 Pro.

Test logs came from `gen_log`, 5 GiB each:
- SCCM: 26.9M rows, including multi-line entries
- Plain: 47.8M rows

## Results against the tests

| Test | Target | egui | Direct2D probe |
|---|---|---|---|
| Scroll, with GPU (worst frame) | 16.7 ms | 3.5 ms, pass | 1.7 ms, pass |
| Scroll, software rendering, 1400x850 | 16.7 ms | about 120 ms (8 fps), **fail** | about 1.5 ms*, pass |
| Scroll, software rendering, 2500x1300 | 16.7 ms | about 270 ms (4 fps), **fail** | about 2.4 ms*, pass |
| Scroll, software rendering, 3840x2100 | 16.7 ms | Not run | about 3.8 ms*, worst 5.3 ms, pass |
| Scroll, software rendering, 2 cores | 16.7 ms | about 180 ms (6 fps), **fail** | about 1.6 ms*, pass |
| Launch to first frame | 200 ms (proposed) | 450 ms wgpu, 213 ms OpenGL, **fail** | 49 ms software, 235 ms GPU target |
| Text at 100% scale | Sharp | Pass | Pass |
| EXE size | No target | 11 MB | 0.7 MB (grid only, no controls) |
| Private memory on the SCCM log | No target | about 600 MB | about 180 MB |

\* Drawing cost, measured by rendering to a memory bitmap and reading the pixels back. See "What wasn't verified".

This table's runs mostly came from a session whose display wasn't being drawn, so its frame times can exclude getting pixels to a screen. Later runs on a visible display confirmed the picture, with egui at about 125 ms on software rendering and Direct2D at 1.2-2.1 ms. The text results were judged by eye from screenshots. The 3840x2100 run kept 13 px text, so it shows far more rows than a real 200%-scaled 4K display would.

The memory comparison isn't exactly like for like, because the egui run also ran filters and the probe has no controls. Of the probe's memory, 154 MB is the row index.

## Engine results

These don't depend on the UI, so they carry over to any front end.

| Measurement | SCCM | Plain |
|---|---|---|
| First open, cold disk, no cache | 3.41 s | 3.69 s |
| First open, cold disk, 2 cores | 4.27 s | Not run |
| Reopen, cached index, warm disk | 0.04 s | 0.06 s |
| Reopen, cached index, cold disk | 0.09 s | 0.14 s |
| Index memory | 154 MB | 274 MB |
| Regex filter, warm disk | 0.3-0.5 s | 0.3-0.5 s |
| First filter, cold disk | about 3.2 s | about 3.1 s |
| Regex filter, 2 cores | about 2 s | Not run |

The UI stayed responsive during filtering. The slowest p99 frame across runs was about 8 ms, and the single worst frame was 16.5 ms.

## Findings that change the design

1. **Don't memory-map logs.** While a file is mapped, a writer can't truncate or shrink it (OS error 1224). Appending, renaming and deleting still work. A viewer must not break the app writing the log. The test is `examples/mapped_file_behavior.rs`.
2. **Positional reads are faster when cold.** Scanning 5 GiB took 2.4-2.6 s with reads and 3.4 s through a map, and they tie when warm (`examples/cold_read.rs`). A random 16 KiB read took about 0.15 ms typically and about 1 ms at worst when cold, which is fine per frame on an SSD. Slow disks and network shares will need background row loading.
3. **Rows can't simply be lines.** SCCM messages can span lines, so the index treats a whole `<![LOG[...]LOG]!>` entry as one row.
4. **`f32` scroll positions fail at this scale.** The grid keeps its own `f64` position.
5. **Static CRT linking works.** The EXE then needs only DLLs that ship with Windows, with no VC++ redistributable.

## Getting frames to the screen over RDP

Measured on a live, visible RDP session with `scripts/run-presentation-test.ps1`. Each path scrolled at 60 Hz for 10 seconds, over two rounds. RDP's own counters recorded what it sent. Idle traffic was about 0.75 KB/s.

| Path | Launch | Frame p50 / p99 | RDP data | CPU per 10 s scrolling | Private memory |
|---|---|---|---|---|---|
| **Flip-model swap chain, WARP** (chosen) | 60 ms | 1.5 / 2.0 ms | 744-761 KB/s | 3.7-4.1 s | 177 MB |
| Flip-model swap chain, WARP, scroll hints | 60 ms | 1.5 / 2.0 ms | 722-747 KB/s | 3.3-3.7 s | 177 MB |
| Flip-model swap chain, hardware | 250 ms | 1.2 / 1.6 ms | 734-779 KB/s | 1.7-1.9 s | 225 MB |
| Flip-model swap chain, hardware, scroll hints | 250 ms | 1.2 / 1.6 ms | 753-763 KB/s | 1.8 s | 225 MB |
| HWND target, software | 62 ms | 2.4 / 7.9 ms | 884-972 KB/s | 3.0-3.3 s | 182 MB |
| HWND target, default | 248 ms | 1.3 / 1.8 ms | 735-739 KB/s | 1.9 s | 227 MB |

The 5 GiB SCCM log was open for all of these, and 154 MB of the memory is its row index.

Every path delivered 32 frames per second over RDP, with no frames skipped and quality at 100. RDP's rate stayed at 32 fps even though the app drew at 60 Hz, so that appears to be this session's limit.

What this shows:

- **WARP removes the launch penalty.** A software D3D11 device launches as fast as the plain software target, and it avoids that target's 7-8 ms frame spikes. It also sends about 20% less data.
- **D3D11's software path is far better than D3D12's for this work.** Direct2D on D3D11 WARP takes about 1.5 ms per frame. egui on wgpu's D3D12 software adapter took about 125 ms on the same visible session. They're different renderers, so this doesn't isolate the API.
- **Hardware's advantage is CPU.** It uses about half the CPU while scrolling, but it costs 190 ms of launch. Neither path draws anything while idle.
- **Scroll hints showed no measurable benefit.** Differences stayed within round-to-round noise, even though Microsoft describes savings over Remote Desktop. They add complexity, so they're left out for now.
- **Pixels are correct.** Captures from the desktop matched at the expected row (row 1801 after 600 frames of 3 rows). The only differences were the window's antialiased rounded corners, which blend with what's behind them, and the antialiased edge of the scrollbar thumb. All text matched exactly across all paths.

## What wasn't verified

- **Slow or lossy RDP links.** The test connection skipped no frames, and its link speed wasn't recorded. Scroll hints might still help on a constrained link.
- **A real no-GPU VM.** Windows Sandbox isn't installed here. The software paths were forced on a machine that has a GPU, and RDP's encoder may behave differently on a VM without one.
- **Slower hardware.** NVMe only. A cold open can't beat the disk's read speed, so a 5 GiB log needs about 10 s at 500 MB/s and much longer on a hard drive.
- **Real workloads.** Generated logs only, with no live tail, UTF-16 logs (rejected with a clear error for now), or high-DPI displays.
- **Frame-time parity.** egui is measured frame to frame, and the probe around each draw call. The probe also draws no filter box or status text.
- **Positional reads in the full engine.** The spike engine still uses a memory map. Reads were measured as a scan, not integrated.
- **The cost of custom UI.** The probe is a grid only. Tabs, inputs, menus and accessibility are all unbuilt, and that's the main cost of this recommendation.

## Mistakes caught along the way

These are recorded because they affected how far the numbers can be trusted.

- The first Direct2D screenshots were blank. `PrintWindow` skips Direct2D output, and direct window capture returned a single color.
- One rerun appeared to reproduce the timings exactly, but every run had failed and the reports were stale. Scripts must delete old reports first.
- The 1.5 ms figure was only accepted once pixels were read back from an offscreen bitmap and the final frame showed the file's last row.

## Reproducing

```bash
cargo build --release --workspace --examples
```

```bash
target/release/examples/gen_log.exe sccm 5 test-data/perf/sccm-5gb.log
```

```bash
target/release/examples/gen_log.exe plain 5 test-data/perf/plain-5gb.log
```

Then run `scripts/run-spike-bench.ps1` for engine and egui numbers, and `scripts/run-ui-compare.ps1` for the head-to-head. Reports go to `spike-output/`, which isn't committed.
