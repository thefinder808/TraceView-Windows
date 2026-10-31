# GUI design

**Status:** agreed, with milestone 2 built. See [Where milestone 2 landed](#where-milestone-2-landed) for what's proven and what's still open. This builds on [ARCHITECTURE.md](ARCHITECTURE.md), and it ports the behavior of the [macOS app](https://github.com/thefinder808/TraceView) rather than its code.

## What success looks like

Someone who uses CMTrace every day double-clicks a log, and it feels like CMTrace with the slowness and limits removed. It opens instantly, scrolling never hitches, and red still means error. Everything else is there when they want it and quiet when they don't.

### Budgets

These become automated checks, like the engine's limits.

| Measure | Limit | Basis |
|---|---|---|
| Launch to first frame | 100 ms | The probe measured 60 ms with no controls |
| Frame, 4K | 8 ms, worst case | The probe measured about 3.8 ms for the grid alone |
| Key or click to updated screen | One frame | |
| CPU while idle | 0% | Nothing draws unless something changed |
| Memory beyond the row index | 50 MB | The probe used about 25 MB |
| EXE size | 5 MB | The CLI is 1.9 MB and the probe adds under 1 MB |
| UI stalls on a slow disk or share | None | File reads never run on the UI thread |

## Layout

```
+--------------------------------------------------------------------+
| TraceView - PolicyAgent.log                               _  []  X |  native frame, themed
+----------+---------------------------------------------------------+
| OPEN     | PolicyAgent.log * | ccmexec.log *  | +                  |  tabs, with a live dot
|  Policy..|---------------------------------------------------------|
|  ccmexec | [All 1.8k] [Crit 0] [Err 186] [Warn 267] [Info 621]     |  severity chips
|          | ...........#####.##............................|        |  event histogram
| NEARBY   | [Filter...........] [.*] [Aa]   Err Warn Info   Saved v |  filter bar
|  (folder)|---------------------------------------------------------|
|          | Row    Time          Level  Component    Message         |
| WINDOWS  | 1697   14:40:32.627  INFO   PolicyAgent  Connected ...   |
|  CCM     | 1698   14:40:34.432  ERR    PolicyAgent  Download fail.. |  log grid
|  Intune  | 1699   14:40:35.697  WARN   CcmExec      Retrying ...    |
|  CBS     |                                                         |
|  Panther |                                         [Jump to bottom]|
|          |---------------------------------------------------------|
| RECENT   | 1,769 rows | 186 shown | UTF-8 | 164 KB | SCCM | Live    |  status bar
+----------+---------------------------------------------------------+
```

- **Same structure as the Mac app,** so moving between them is effortless. Split view uses the same layout, twice.
- **The sidebar can collapse to nothing.** With every extra hidden, the window is a tab bar and a grid, which is CMTrace's whole appeal.
- **The sidebar's Windows section replaces the Mac's Console-style browser.** It lists the usual log folders that exist on this machine: `CCM\Logs`, Intune Management Extension, `Logs\CBS`, `Logs\DISM`, `Panther`, `INF\setupapi.*`. It's a list of locations, not a scan, so it adds nothing to launch time.
- **A "Nearby" section lists other logs in the same folder.** Troubleshooting usually means hopping between neighboring logs.

### Window frame

Use the native Windows frame with its caption colored through DWM to match the theme. This keeps Snap Layouts, the system menu, and correct resize behavior for free. A fully custom title bar costs a lot and buys little in a tool like this.

## Keeping CMTrace's habits

These are what make it feel familiar, so they come first:

| CMTrace behavior | Plan |
|---|---|
| New lines scroll into view at the bottom | Same. Scrolling up pauses it, and returning to the bottom resumes it |
| Red for errors, yellow for warnings | The default in the Classic theme, and the same meaning in every theme |
| Highlight rows containing a word, in a chosen color | Yes, as saved highlight rules |
| Find and Find Next (F3) | Yes, plus regex. Find moves between matches, and filter hides the rest |
| Filter by text, component, time and thread | Yes |
| Error Lookup | Yes, with a Windows code database: HRESULT, Win32, NTSTATUS, SCCM, Intune |
| Merge selected files into one live view | Yes, including live append |
| One small EXE, copied anywhere | Yes |
| Offers to become the `.log` viewer | Yes, but only from a Settings button, never as a prompt at launch, and per-user only |

Standard Windows keys are used throughout: Ctrl+O, Ctrl+F, F3 and Shift+F3, Ctrl+G, Ctrl+W, Ctrl+Tab, Ctrl+1-9, Ctrl+K for the command palette. Every command is remappable.

## From the Mac app

Severity chips, the clickable histogram with spike preservation, the expand-in-place row drawer, saved filter presets, bookmarks, split view with scroll-sync, merged view, the command palette, and CSV export. Mac-only features (unified log streaming, `.ips` crash reports) are dropped.

Windows-specific additions to consider later: live Event Log streaming and `.evtx` files, in place of the unified log.

## Customization

This is a main goal, so it's a system rather than a settings page. Everything is plain text files in `%APPDATA%\TraceView`, editable in the app or by hand, and they reload live. If a `traceview.toml` sits beside the EXE, the app uses that folder instead, which makes it fully portable on a USB stick or an admin share.

| Customizable | How |
|---|---|
| Themes | Every color and metric is a named token. Ships with Classic (CMTrace colors), Console (the Mac default), Light, Dark and Neon. A theme is one small file, easy to share |
| Fonts and density | Any installed font, size and row height, separately for the grid and the UI |
| Columns | Show, hide, reorder and resize per format, including Thread and Source |
| Highlight rules | Pattern and optional level, component or format, mapped to a row color, text color or marker. Ordered, and they can be turned on or off as a set |
| Filter presets | Saved and named, with their own shortcuts |
| Custom formats | A regex with named groups (`time`, `level`, `component`, `message`) adds a format with no code. This also covers logs the app doesn't know about yet |
| Keyboard | Every command can be rebound |
| Layout | Sidebar, chips, histogram and status bar can each be hidden. Named layouts switch between them |

**No plugins or scripts in v1.** Regex-based formats and rules run in the same linear-time engine as search, so a shared theme or rule file can't hang the app or run code. A scripting system would add a security risk in a tool that reads untrusted logs, and I'd want a real need before accepting that.

## Structure

```
crates/
  engine/      exists today: log logic, no UI, no unsafe
  tv-platform/ Win32: window, messages, DPI, clipboard, dialogs, input, UI Automation
  tv-render/   DirectWrite and Direct2D on a flip-model swap chain
  tv-ui/       our widget toolkit: layout, focus, text field, list, menu. Knows nothing about logs
  traceview/   the EXE: CLI, and the app built from the above
```

- **`unsafe` stays contained.** Windows API calls live in `tv-platform` and `tv-render`. `tv-ui`, the engine and the app forbid `unsafe`, and the compiler enforces it.
- **The renderer is a small interface,** roughly fill, text, clip and present. This is the boundary that keeps GPU or GDI options open without touching anything above it.
- **The toolkit is only what this app needs.** It isn't a general framework, so it stays small: text field, button, list, tree, tab bar, menu, tooltip, scroll bar, splitter.

### Drawing

A widget tree is kept in memory, and any change redraws the whole window. That's the simplest approach, and at about 4 ms for a 4K frame there's no need for anything cleverer. Nothing draws when nothing changes, which gives 0% idle CPU. Live tail draws at most 60 times a second, however fast rows arrive.

### Threads

Goal one says never make someone wait, so the UI thread never touches the disk.

```
UI thread        window messages, layout, drawing
Row loader       reads rows into a cache, ahead of where you're scrolling
Engine pool      opening, searching, summarizing, each cancellable
File watcher     notices log changes and triggers refresh
```

If a row isn't cached yet, as on a slow share, the grid draws its number and a dim placeholder, then fills it in when it arrives. Scrolling never blocks.

**Noticing changes:** `ReadDirectoryChangesW` on the log's folder, plus a slow timer as a fallback, because change notifications aren't reliable on every network share.

### Text

- **Layout cache:** shaped text is cached per visible cell, so scrolling doesn't reshape rows it has already seen.
- **Measuring mode:** GDI-classic looks best at 100% scale (it's what the probe used), and natural mode suits higher DPI. The app picks by monitor, per monitor.
- **Very long rows:** only what fits on screen is laid out, so a 1 MB line costs the same as a short one.

### Accessibility

Built from the first widget, because it's very hard to add afterward. Every widget reports its role, name and state through UI Automation, and the grid exposes its visible rows and announces selection. AccessKit is the leading candidate, since its Windows adapter implements UI Automation and it's designed for custom toolkits. The whole app must also work by keyboard alone, and it must respect Windows high-contrast mode.

### One EXE for CLI and GUI

Windows 11 24H2 added a [console allocation policy](https://learn.microsoft.com/en-us/windows/console/console-allocation-policy) for exactly this case. The plan is:

1. Build as a console program with `consoleAllocationPolicy` set to `detached`.
2. Subcommands (`query`, `summarize`, later `mcp`) behave as normal console programs, and shells wait for them.
3. With no subcommand, or just a file path, the app starts the GUI. If that came from a shell, it relaunches itself detached and returns at once, so the shell doesn't sit waiting until the window closes.
4. Older Windows ignores the setting and shows a console window briefly. There the app closes it as early as it can. Server 2022 and earlier, and Windows 10, are in this group, so this flash needs measuring on those systems.

If the flash is too visible, the fallback is a small `traceview-cli.exe` beside the main EXE, which works everywhere but gives up pure single-file use for the CLI. The GUI would still be one file.

## What must be proven first

These are the biggest unknowns, and each gets a small test before anything depends on it.

| Risk | Test | If it fails |
|---|---|---|
| Console flash on older Windows | Run the EXE on Windows 10 and Server 2022 VMs | Separate CLI EXE |
| AccessKit with a custom toolkit and virtual grid | Navigate a prototype grid with Narrator | Implement UI Automation directly |
| IME input in a custom text field | Type Japanese and Chinese into a prototype through the Text Services Framework | Fall back to the older IMM32 messages, or a native `EDIT` control for input only |
| WARP unavailable | Force device creation to fail | Fall back to the HWND software target |
| Real no-GPU VM over RDP | Run the probe in a Hyper-V VM | Tune, or reconsider the presentation path |

## Where milestone 2 landed

**Budgets, measured on a live RDP session at 200% scale with a 5 GiB, 26.9M-row log:**

| Measure | Limit | Measured |
|---|---|---|
| Launch to first frame | 100 ms | 48 ms |
| Worst frame, 2400x1520 | 8 ms | 2.9 ms |
| Worst frame, 3460x2100 | 8 ms | 5.1 ms |
| CPU while idle | 0% | 0 ms used over 5 s |
| Memory beyond the row index | 50 MB | 40-48 MB, growing with window size |
| EXE size | 5 MB | 2.3 MB |
| Frames showing a loading row | | 1 of 2,400 across all scroll stages |

The launch figure has no log open. A log with a cached index adds about 50-110 ms, in the background. A first open of a 5 GiB log takes 1-3 s, also in the background, with progress shown.

`scripts/check-gui-perf.ps1` enforces these. `scripts/check-gui.ps1` drives the real EXE from outside through UI Automation.

**Risk tests:**

| Risk | Result |
|---|---|
| One EXE on Windows 11 24H2+ | **Passed.** No console from Explorer, a shell returns in about 90 ms, and CLI pipes work |
| Console flash on older Windows | **Not tested.** Needs a VM with Windows 10 or Server 2022 |
| AccessKit with a virtual grid | **Passed through UI Automation, with one change.** Its Windows adapter gives table rows no selection support, so the grid is exposed as a list. Names, focus and selection all read correctly. Nobody has tried it with Narrator yet |
| IME in a custom text field | **Moved to milestone 3.** There's no text field until then, and testing needs an IME installed |
| WARP unavailable | **Fallback logic tested,** by simulating the failure. Not yet seen on a machine that really lacks it |
| Real no-GPU VM over RDP | **Not tested.** Needs a VM |

**Differences from this design, so far:**

- **No text layout cache.** Drawing text directly already fits the budget, so it isn't needed yet.
- **Rows scroll whole, not by pixels,** which keeps text sharp and matches CMTrace.
- **`crates/d2d-probe` is still here,** because `scripts/run-presentation-test.ps1` uses it. It goes once that test moves to the real app.

**Not covered by automation, so these need a person:** the file dialog, drag and drop, mouse dragging, how scrolling feels, and Narrator.

## Milestones

| Milestone | Delivers | Done when |
|---|---|---|
| **2. Foundations** | The risk tests above, then a window, renderer, and a real grid with scrolling, selection and keyboard. Opens files from a dialog or drag and drop | Opens 5 GB instantly, stays within budget, and Narrator can read it |
| **3. CMTrace replacement** | Live tail, find, filter, highlighting, Error Lookup, merge, tabs, Classic theme | You can use it instead of CMTrace all day |
| **4. TraceView** | Chips, histogram, row drawer, split view, sidebar, presets, bookmarks, command palette, all themes | Feature parity with the Mac app, where it applies |
| **5. Customization** | Rules, custom formats, column layouts, keyboard remapping, portable mode, and the in-app editors for them | Everything in the table above works |
| **6. Release** | Signing through the WinSign pipeline, an update mechanism, installer-free packaging, documentation | A signed public build |
| **Later** | MCP server, `traceview://` links, "Copy for AI" | See [ARCHITECTURE.md](ARCHITECTURE.md) |

Theme tokens and the rule engine go in from milestone 2, so colors are never hard-coded even before their editors exist. Milestone 5 is about exposing them.

## Decisions in this draft

These are my picks. Each can change if you see it differently.

| Decision | Pick | Why |
|---|---|---|
| Minimum Windows | Windows 10 1809 and Server 2019 | Admins live on these. Going lower adds risk for little reach. Needs the VM tests above |
| Default theme | Classic in light mode, Console in dark mode, following the system setting | CMTrace users get what they expect, and Mac users get familiarity |
| Default on first launch | Grid, tabs, chips and filter bar, with the sidebar and histogram collapsed | Starts close to CMTrace, and reveals more as needed |
| Menu bar | Our own, themed, and on by default | Admins expect one. Native Win32 menus can't follow dark themes |
| Title bar | Native frame, colored by DWM | Free Snap Layouts and correct behavior |
| Updates | Decide at milestone 6 | A portable EXE changes the options, and it's not urgent |
