# GUI design

**Status:** agreed, with milestones 2 to 4 built. Each one's "Where it landed" section says what's proven and what's still open. This builds on [ARCHITECTURE.md](ARCHITECTURE.md), and it ports the behavior of the [macOS app](https://github.com/thefinder808/TraceView) rather than its code.

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
| Offers to become the `.log` viewer | Yes, but only from the Tools menu, never as a prompt at launch, and per-user only |

Standard Windows keys are used throughout: Ctrl+O, Ctrl+F, F3 and Shift+F3, Ctrl+G, Ctrl+W, Ctrl+Tab, Ctrl+1-9, Ctrl+K for the command palette. CMTrace's own keys work too: Ctrl+L for Error Lookup and Ctrl+U to pause, so Filter is Ctrl+Shift+F. Every command is remappable.

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
| Layout | Sidebar, chips, histogram and details pane can each be hidden. Named layouts switch between them |

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

## Where milestone 3 landed

**What it does:**

- **Tabs.** Logs open from the Open dialog, which takes several files at once, from drag and drop, or from the command line. Ctrl+Tab and Ctrl+1-9 switch tabs. Ctrl+W or a middle-click closes one.
- **Menu bar.** File, Edit, View and Tools, all usable from the keyboard with Alt or F10.
- **Filter** (Ctrl+Shift+F). Takes words, "quoted phrases", `/regex/`, `level:`, `component:`, `thread:`, `from:` and `to:`. The old filter stays on screen until a new one finishes.
- **Find** (Ctrl+F, F3, Shift+F3). Matching rows are highlighted and counted, as in "3 of 120", which also covers CMTrace's Highlight. Ctrl+E and Ctrl+Shift+E jump between errors.
- **Details pane.** Shows the selected row's fields and whole message, and what Windows says its error codes mean.
- **Error Lookup** (Ctrl+L). Takes a code or a pasted log line, and opens with the selected row's first code.
- **Live tail.**
  - New rows appear within about a quarter of a second, and log rotation is followed.
  - A tab follows new rows while it's at the bottom, which is where logs open, as in CMTrace.
  - Ctrl+U pauses.
- **Merge** (Ctrl+Shift+M). Merges the open logs into one timeline in a new tab, which keeps updating.
- **Themes.** Classic and Console, or follow Windows.

**Budgets,** with the 5 GiB, 26.9M-row log:

| Measure | Limit | Measured |
|---|---|---|
| Launch to first frame | 100 ms | 47 ms |
| Worst frame, 1200x760 | 8 ms | 2.0 ms |
| Worst frame, 1920x1050 | 8 ms | 2.4 ms |
| CPU while idle | 0% | 0 ms used over 5 s |
| CPU while following a small live log | | 16 ms used over 5 s |
| A line written to the file appears | | 261 ms |
| Memory beyond the row index | 50 MB | 32-34 MB |
| EXE size | 5 MB | 2.6 MB |

The RDP window was minimized for these frame times, so they leave out presenting to the screen. Milestone 2's on-screen figures are a better guide to real frame times.

**Differences from this design:**

- **Ctrl+L is Error Lookup,** as in CMTrace, so Filter moved to Ctrl+Shift+F.
- **Merge works on open tabs,** rather than through a checkbox in the Open dialog. You open several logs at once, then press Ctrl+Shift+M.
- **Error Lookup has no Intune or Windows Update text,** because Windows has none. Those codes still show their hex, signed and facility forms. ConfigMgr text needs a console installed, and that hasn't been tested.
- **Live tail checks each file every 250 ms** rather than waiting for change notifications. Windows reports changes late for files a writer holds open, and unreliably on network shares.
- **Highlight rules in chosen colors** waited for milestone 5. Find covered one highlight until then.
- **IME** is built: IMM32's composition window sits at the caret, and the IME is on only while a text box has focus. It hasn't been tried with a real IME, because none is installed here.

**Not covered by automation, so these need a person:** choosing several files in the Open dialog, drag and drop, an IME, Narrator, and a day of real use alongside CMTrace.

## Where milestone 4 landed

**What it adds:**

- **Severity chips** above the grid count each level in the whole log. Clicking one filters to that level, alone or with the filter box. Clicking it again, or All, shows every level.
- **Event histogram** (View > Histogram): 60 buckets with errors stacked on top and warnings below.
  - Earlier spikes leave a shadow as a live log's range stretches.
  - Clicking a bucket selects its worst row.
- **Sidebar** (Ctrl+B): open tabs, bookmarks across open logs, logs in the same folder, the usual Windows log folders that exist here, and recent logs.
- **Command palette** (Ctrl+K): every menu command with its shortcut, plus open tabs, recent logs and filter presets.
  - Typing `:` and a number goes to that row, which is what Ctrl+G opens it for.
- **Bookmarks:** Ctrl+D toggles one, and F2 and Shift+F2 step through them. They're saved per file and listed in the sidebar. Right-clicking a row offers it too (see below).
- **Filter presets:** View > Save filter as preset stores the filter text and the active chip under a name. They're applied or deleted from the palette.
- **Split view** (Ctrl+\\): two logs side by side, each with its own filter, find, chips and histogram.
  - The focused pane is marked and gets the keyboard.
  - Ctrl+Shift+S syncs the panes, so scrolling one lines the other up at the same time.
- **CSV export** (File > Export rows as CSV): the selection, or every row shown.
- **Themes:** Classic, Console, Dark, Light and Neon, all contrast-tested.
- **Saved settings** in `traceview.toml`: theme, layout, recent logs, presets and bookmarks.
- **Native ARM64 EXE,** built in CI.

**Budgets,** with the 5 GiB log and the RDP window minimized:

| Measure | Limit | Measured |
|---|---|---|
| Launch to first frame | 100 ms | 49 ms |
| Worst frame, 1920x1050 | 8 ms | 2.4 ms |
| CPU while idle | 0% | 0 ms used over 5 s |
| Memory beyond the row index | 50 MB | 34-36 MB |
| EXE size | 5 MB | 3.1 MB |

**Differences from this design:**

- **Console is the Mac's light default theme,** as it is on the Mac. Earlier milestones used the name for the Mac's dark theme, which is now called Dark and is what dark mode uses.
- **The row drawer isn't built.** The details pane already shows the whole message, so it waits until it's clear that it's wanted.
- **Presets live in the palette** rather than as pills in the filter bar.
- **Chips show exact counts** up to 9,999, then shorten them, as in 12.3k.
- **Only the focused pane** of a split view is in the screen reader tree. Screen reader users work in one pane at a time anyway.

**Not covered by automation, so these need a person:** the ARM64 EXE on a real ARM64 PC, how the sidebar, chips and split view feel, and Narrator with the new controls.

## Milestone 5

**Highlight rules** (Tools > Highlight rules, Ctrl+H, as CMTrace's Highlight is):

- **A rule is filter box text,** such as `download failed level:error`, `component:PolicyAgent` or `/0x8007[0-9a-f]{4}/`. That covers "pattern and optional level or component" with a syntax people already know.
- **Each rule has a color and a style.** The six colors are red, orange, yellow, green, blue and purple. The whole row can take the color, which replaces the severity color, or a marker can sit beside the row number, which keeps it.
- **The first rule that matches a row wins.** Rules are reordered with Ctrl+Up and Ctrl+Down, or the Move buttons.
- **View > Use highlight rules** turns them all off or on together. Opening the editor turns them on.
- **Every change applies at once,** so rows light up while a pattern is typed, and it's saved to `traceview.toml` straight away.
- **The editor** is one floating panel: the pattern box, then the colors, then the style, On and Move/Delete buttons, then the list of rules ending in a "New rule" slot. Typing into the slot adds a rule. Tab moves between the parts, and screen readers get a modal dialog with radio buttons for the colors and styles. Rows that match say "highlighted blue" or "marked blue".

**Differences from this design:**

- **Colors are named palette entries, not any color.** Each theme defines its own shade of each, tested for contrast, so a rule picked in Classic stays readable in Dark. A theme file can change the shades.
- **There's no text-color-only style.** Text in a chosen color can't be held to contrast against every severity row, and the marker already keeps the severity visible.
- **No format condition.** With two formats, the filter syntax's level, component and thread cover what's needed.
- **Rules live in `traceview.toml`,** not a file of their own. Edits made there by hand load while TraceView runs.

**Not covered by automation:** using the editor with a mouse and with Narrator on a real screen.

**Columns:**

- **Each format has its own columns:** Time, Level, Component, Thread, Source and Message, in any order and width, shown or hidden. SCCM logs now show Thread by default, as CMTrace does, and other logs look as before. Source is the source file an SCCM row names, and it starts hidden.
- **With the mouse,** dragging a header's edge resizes its column and dragging the header moves it, with a line showing where it will land. The pointer changes over an edge.
- **With the keyboard,** View > Columns, or right-clicking the header, opens an editor for the current tab's format. Space shows or hides a column, Ctrl+Up and Ctrl+Down move it, and minus and plus make it narrower or wider. Reset brings back the defaults. Screen readers get a modal dialog with a list and buttons.
- **The row number always comes first and the message always shows.** The last column fills the rest of the grid.
- **Copy (Ctrl+C) follows the shown columns,** in their order, and so does each row's text for screen readers. CSV export still writes every field, and gained a Source column.

**Budgets** with the 5 GiB SCCM log after these two steps: worst frame 2.6 ms at 1920x1050, launch 49 ms, 38–41 MB beyond the row index, a 3.2 MB EXE. All within limits. Memory rose about 4 MB, mostly from keeping each loaded row's source file name.

**Differences from this design:**

- **The Row column can't be hidden or moved,** because bookmark and rule markers sit beside it.
- **No horizontal scrolling.** Columns past the window's right edge are cut off, so the last column should be the one that can lose text.
- **A merged tab uses its first file's columns.** Merging SCCM and other logs together gives one layout either way.

**Not covered by automation:** dragging headers by hand, where the feel of the grab zones matters.

**Fonts and row spacing:**

- **View > Fonts** lists the log font, the row spacing and the window font. Smaller and Larger, or minus and plus, change whichever is selected, and Reset goes back to Consolas 13 and Segoe UI 12.
- **Choose font** opens the command palette's list of installed fonts. Typing narrows it, fixed-width fonts say so, and the one in use comes first. Choosing one, or pressing Esc, goes back to the editor.
- **Ctrl+Plus, Ctrl+Minus and Ctrl+0** make log text larger, smaller or the default size, anywhere, and so does Ctrl with the mouse wheel. The status bar says the new size.
- **Row spacing** is compact, normal or roomy, added to the font's measured line height. Normal with Consolas 13 is the 18-pixel row from before.
- **A font this PC doesn't have** draws in the default, and the editor says so, which matters for a portable copy carried between PCs.

**Differences from this design:**

- **Window text tops out at 15 pixels.** The bars and lists have fixed heights, so a larger window font would be clipped. Windows' display scale already makes everything larger, and TraceView follows it.
- **Row height is a choice of three spacings,** not a number, so a row can never be too short for its font.
- **The number pad's keys** used to arrive as lowercase letters, which no shortcut used, and now map to nothing except plus and minus.

**Keyboard shortcuts:**

- **Tools > Keyboard shortcuts** lists every command with its shortcut, in the command palette, so typing finds one. Enter on a command waits for the new keys. Backspace removes the shortcut, Delete restores the default, and Esc leaves it alone. The list comes back as it was afterwards, and its last entry resets every shortcut.
- **Any command can have a shortcut,** including menu commands that had none, and next and previous tab, which were Ctrl+Tab and Ctrl+Shift+Tab only.
- **Taking keys another command has moves them,** and the status bar says so, as in "Next error is now Ctrl+F, which Find had".
- **Keys the app keeps say why when pressed,** as in "Ctrl+V is kept for editing". A shortcut needs Ctrl or a function key, so typing in a box never runs one.
- **Menus and the palette show the current shortcuts.**

**Differences from this design:**

- **Ctrl+1 to Ctrl+9, copy, select all and Exit can't be changed.** They're the keys Windows and CMTrace users already rely on for those, and the text boxes use the same editing keys.
- **One shortcut per command,** rather than several.
- **Changes are made in the app,** or by hand in `traceview.toml`, which loads while TraceView runs.

**Custom log formats:**

- **A format is a regex with named parts:** `time`, `level`, `component`, `thread`, `source` and `message`, plus a name, optional file names such as `u_ex*.log`, and whether dates are day first.
- **Logs pick their format when they open:** a format naming the file wins, then SCCM, then a format matching at least 80% of the first 20 lines, then plain text. The status bar shows the format's name, and the CLI's output carries it too.
- **Tools > Log formats** edits them. One box edits the selected format's name, pattern or file names, picked with the buttons below it, and the list ends in a "New format" slot. While a pattern is typed it's tried on the open log's first 20 lines, saying how many match and what the first match's parts are.
- **"Use for this file"** adds the open log's name to the format and reads the log again with it. Otherwise changes apply to logs opened afterwards.
- **Each custom format has its own column layout,** showing the parts its pattern names, with the level and message always.
- **Indexing with a pattern takes about twice as long** as the built-in reader: 2.7 s for the 5 GiB plain log, against 1.3 s.

**Differences from this design:**

- **One row per line.** Lines that don't match, such as a stack trace's, show as rows of their own with no time. Joining them to the row above needs the indexer to find rows by pattern, which is a larger change.
- **Times need a date.** A `time` part with only a time of day isn't read yet.

**Theme files:**

- **A theme is one TOML file** in a `themes` folder beside `traceview.toml`. It names a built-in theme to start from and lists only the colors it changes, including the shades of the six rule colors, so a shared theme can be a few lines. Files in the folder show in the View menu after the built-in themes.
- **View > Edit theme** lists every color of the theme in use with a swatch, 58 in all. The box takes a color as `#RRGGBB`, and the whole window, the editor included, recolors as soon as the color is complete. Up and Down pick a color without leaving the box.
- **A built-in theme is never changed.** The first change makes a copy, such as "Classic copy", saves it and switches to it. Every later change is saved at once.
- **The Name button** edits the theme's name, which is also its file's, taken on Enter. **Dark** sets whether the title bar is dark. **Open folder** shows the themes folder in File Explorer.
- **Weak contrast is pointed out,** as in "Low contrast: Primary text: 1.2 to 1, below 4.5", using the same check the built-in themes pass. It's a warning, not a refusal.
- **A file with a mistake** is skipped, and the status bar says which file and why.

**Differences from this design:**

- **Metrics aren't edited in the app.** A file can set the header and status bar heights (16 to 48 pixels), but the editor only changes colors, since the font settings already size rows.
- **No color picker.** Colors are typed or pasted as hex. A picker would be a large control for something done rarely.
- **No deleting themes in the app.** Open folder, then delete the file, and it leaves the menu within a second.

**Not covered by automation:** using the editor with Narrator, and Open folder on a real desktop.

**Named layouts:**

- **A layout says which panes show:** the sidebar and its width, the severity chips, the histogram and the details pane.
- **Two are built in.** CMTrace shows the grid and details pane only, and Everything shows every pane. Neither can be deleted or replaced, and both can be given shortcuts in Tools > Keyboard shortcuts.
- **View > Save layout as...** asks for a name in the palette, as saving a filter preset does. Saving under a name already used replaces that layout.
- **Each layout is a View menu item,** checked when the window shows what it does. Changing a pane by hand unchecks it. Ctrl+K lists them too, along with "Delete layout" for saved ones.
- **Menus too tall for the window wrap into a second column,** so a long list of themes and layouts stays reachable.

**Differences from this design:**

- **The status bar always shows.** It carries row counts, Live or Paused, export progress and every message, and it's one 22-pixel bar.
- **Split view isn't part of a layout,** since it needs two open logs.
- **Saved layouts have no shortcuts of their own.** Ctrl+K and a few letters of the name reach one, as with filter presets.

**Fixed along the way:** the sidebar's width was saved as 0 whenever settings were saved in a session where the sidebar never showed, so it next opened at its narrowest. Its chosen width is saved now.

**Live reload:**

- **Edits to `traceview.toml` and theme files load within a second** while TraceView runs, and the status bar says "Settings reloaded". That covers the theme, panes, rules, columns, fonts, shortcuts, presets, layouts and bookmarks. New log formats apply to logs opened afterwards, as they do when edited in the app.
- **A hand edit wins.** TraceView never saves over an edit it hasn't loaded yet. If it was about to save a change of its own, that change is dropped and the edit is loaded. An editor of settings that's open, such as Highlight rules, closes and says why.
- **A file with a mistake** keeps the settings in use and says what's wrong and on which line. TraceView saves nothing until it's fixed, and fixing it loads it. That's true at launch too, where it used to need a restart, and theme files now still load when the settings file has a mistake.
- **A deleted settings file** is left alone, and the next change made in the app writes it again.
- **Theme files** reload as a set. A changed one recolors the window if it's in use, a new one joins the menu, and a deleted one leaves it.

**Differences from this design:**

- **Checks run once a second,** not on change notifications, which network shares don't deliver reliably. A check reads only sizes and times.
- **Comments and spacing in `traceview.toml` don't survive** TraceView's next save, which writes the whole file again. Edits to values do.

**Fixed along the way:**

- **Hidden severity chips** were still laid out, invisibly, at the window's corner, so screen readers listed them. They exist only while they show now.
- **The end-to-end check** ran the EXE against the real settings in `%APPDATA%`, so its test logs ended up in the Recent list. It runs a portable copy with its own settings now, and it checks live reload in the real window.

**Fixed along the way:** a frame now always brings screen readers up to date. Before, what came from layout, such as the severity chips' counts and an editor's buttons, could stay out of date for them until the next input. The end-to-end check caught it as an intermittent failure once the index cache had to be rebuilt.

## Since milestone 5

**The row menu:**

- **Right-clicking a row** opens a menu with Add bookmark, or Remove bookmark if it has one, and Copy, each with its shortcut. Shift+F10 and the menu key open it for the selected row, as they open a context menu anywhere in Windows, and screen readers get it as a menu.
- **A row outside the selection becomes the selection.** One inside it leaves the selection as it is, so rows can be selected, right-clicked and copied together. The bookmark item is always for the row that was clicked.
- **It's a toolkit widget,** drawn and stepped through by the same code as the menu bar's menus, so more row actions can join it later.

**Opening `.log` files:**

- **Tools > Use TraceView for .log files...** registers TraceView, for this Windows user only, as a program for `.log` and `.lo_` files, then opens Windows' Default apps page at TraceView. Windows doesn't let a program make itself the default, so the person picks it there. It needs no administrator rights and never prompts at launch.
- **Tools > Stop using TraceView for .log files** takes the registration back. A type whose default was TraceView then asks again which program to use.
- **The registration names this EXE's path.** If a portable copy moves, the menu offers "Use this copy for .log files..." to point it at the new place.

**Differences from this design:**

- **It's in the Tools menu,** as there's no settings window to hold a button.
- **Each file opens its own window,** as CMTrace does. Opening several into one window's tabs needs a single running copy to hand files to, which is a larger change.
- **The file icon came later,** with milestone 6. Registering now gives `.log` files TraceView's icon.

**Windows contrast themes:**

- **While a contrast theme is on,** TraceView draws only with its colors, whatever theme is chosen, and switches back live when it's turned off or recolored. The chosen theme is kept for then, and View shows "Theme: Windows contrast, while it's on".
- **Colors pair only as Windows intends:** text, inactive text and links on the background, and selected text on the selection. Nothing else is filled.
- **Errors and criticals are in the link color,** and debug rows in the inactive-text color. The Lvl column and the chips name every level, as always.
- **Hovered and highlighted items use the selection pair,** as do selected text in boxes, chips, menus, lists and the palette.
- **Rules show as markers and find matches are framed,** since a contrast theme has no fills to spare for them.
- **The title bar is left to Windows,** which draws it in the contrast theme's colors.
- **Edit theme is off meanwhile,** since an edit wouldn't show.

**Not covered by automation:** a real contrast theme switched on in Windows Settings, and the Default apps page. Tests use a contrast theme's colors directly, and register in a scratch registry key.

**The GUI benchmark measures fixed pixel sizes.** Its window sizes used to be scaled by the display, so at 200% the 1920x1050 run drew about 3460x2100 pixels, with frame buffers about four times larger. That put memory at 47-51 MB against the 50 MB budget, on `main` as well. The sizes are now physical pixels, so the budgets mean the same at any scale. At 200%, with the RDP window minimized: launch 48 ms, worst frame 2.95 ms at 1920x1050, 38-41 MB beyond the row index.

## Milestone 6

**Decided at the start:**

- **Updates notify, never install.** TraceView will check GitHub at most once a day at launch, and from Help > Check for updates, and offer to open the download page. A setting turns the daily check off. It's left until the repository is public, because a private repository's releases can't be read without signing in.
- **The repository stays private for now,** so releases are private too.
- **Signing uses Microsoft's own GitHub action** with an existing Azure Artifact Signing account. It signs in to Azure with OpenID Connect, so no Azure secret is stored in GitHub.
- **The first release is 0.9.0, a beta,** tested on Windows 11 only.

**The EXE's identity:**

- **The icon is the Mac app's,** in the sizes Windows uses. It shows in Explorer, the taskbar, the title bar and on registered `.log` files.
- **Version info** gives Explorer's Properties and Task Manager the name TraceView, its version and its publisher.
- **Help > About TraceView** shows the version, the copyright and the license, in a standard Windows box that screen readers read.

**Releases:** a `v*` tag builds and tests on a machine with no access to signing, then a second job signs both EXEs, checks the signatures and creates a draft release of two zips and their checksums. [RELEASING.md](RELEASING.md) has the steps and the one-time setup.

**Docs for the people using it:** the README now starts with what TraceView is, how to download it and check its signature, and how to use it. It also covers portable copies, the command line, what it writes and where, and how to remove it all. Building from source comes after. The screenshots are frames the snapshot tests draw, of their sample log, so they show no one's paths. `CHANGELOG.md`, `SECURITY.md` and a Buy Me a Coffee link (in the README and `.github/FUNDING.yml`) match the Mac app's.

**Before the repository goes public:**

- The privacy sweep over the full history, every ref and the object database.
- Turn on private vulnerability reporting, which `SECURITY.md` points to.
- The update check, which needs public releases to read.

**Licenses:** TraceView is MIT, as the Mac app is. The crates it's built with have their licenses in `THIRD-PARTY-NOTICES.txt`, which `traceview --licenses` also prints, and CI fails on a license that isn't allowed or a crate with a known vulnerability.

## After 0.9.0

**The start page,** which replaces a line of text in the empty window:

- **Two buttons,** Open log... and Open folder..., with their shortcuts, then a list: recent folders, recent logs, and this PC's usual log folders with their key logs under each, such as `AppEnforce.log` and `CAS.log` under ConfigMgr client. Every entry opens with one click or Enter.
- **The key logs are a fixed list per folder,** in `places.rs`, and only those that exist are listed. They're found by the same background job that finds the sidebar's Windows folders, so the page adds nothing to launch time.
- **It's built from toolkit parts:** the sidebar's list widget, which brings keyboard, wheel and Narrator support, and two drawn buttons. The main button uses the chips' selected colors, so it follows every theme, including Windows contrast themes.

**Folders,** for logs copied from another PC:

- **File > Open folder... (Ctrl+Shift+O), dropping a folder,** a recent folder, or a folder on the start page lists that folder's logs on the start page, newest first.
- **Each log is marked as the histogram is:** red for errors, yellow for warnings only, and gray for neither, with the counts beside it. A background thread opens each log in turn and counts levels from its index, so the list is usable at once and fills in. Logs over 256 MB aren't counted.
- **The folder stays listed** while its logs are open, so closing the last tab goes back to it. Back, or Escape, leaves it.
- **Opened while logs are open,** the folder shows over them, with the tabs still above it. Choosing a log, clicking a tab or Escape goes back to the logs.

**The histogram is on by default.** It's the quickest way to see when a log's errors happened. The CMTrace layout still hides it, and a settings file that already turned it off keeps it off.

**Not covered by automation:** the folder dialog, dropping a folder, and Narrator reading the start page.

## Milestones

| Milestone | Delivers | Done when |
|---|---|---|
| **2. Foundations** | The risk tests above, then a window, renderer, and a real grid with scrolling, selection and keyboard. Opens files from a dialog or drag and drop | Opens 5 GB instantly, stays within budget, and Narrator can read it |
| **3. CMTrace replacement** | Live tail, find, filter, highlighting, Error Lookup, merge, tabs, Classic theme | You can use it instead of CMTrace all day |
| **4. TraceView** | Chips, histogram, row drawer, split view, sidebar, presets, bookmarks, command palette, all themes | Feature parity with the Mac app, where it applies |
| **5. Customization** | Rules, custom formats, column layouts, keyboard remapping, portable mode, and the in-app editors for them | Everything in the table above works |
| **6. Release** | Signing with Azure Artifact Signing, an update mechanism, installer-free packaging, documentation | A signed public build |
| **Later** | MCP server, `traceview://` links, "Copy for AI" | See [ARCHITECTURE.md](ARCHITECTURE.md) |

Theme tokens and the rule engine go in from milestone 2, so colors are never hard-coded even before their editors exist. Milestone 5 is about exposing them.

## Decisions in this draft

The main choices made for the GUI, and why. Any of them can be revisited.

| Decision | Pick | Why |
|---|---|---|
| Minimum Windows | Windows 10 1809 and Server 2019 | Admins live on these. Going lower adds risk for little reach. Needs the VM tests above |
| Default theme | Classic in light mode, Dark in dark mode, following the system setting | CMTrace users get what they expect, and Mac users get familiarity |
| Default on first launch | The start page, then the grid, tabs, chips, histogram and filter bar, with the sidebar collapsed | Opening a log takes one click, and the histogram shows when errors happened. The CMTrace layout is one menu item away |
| Menu bar | Our own, themed, and on by default | Admins expect one. Native Win32 menus can't follow dark themes |
| Title bar | Native frame, colored by DWM | Free Snap Layouts and correct behavior |
| Updates | Decide at milestone 6 | A portable EXE changes the options, and it's not urgent |
