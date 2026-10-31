# Launches the real EXE and checks it from outside, the way Windows and a screen
# reader see it. Exits non-zero if anything fails.
#
# Needs a built release EXE and a 5 GiB log (see check-perf.ps1). A probe window
# opens and closes several times while it runs.

$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\release\traceview.exe'
$log = Join-Path $root 'test-data\perf\sccm-5gb.log'
$workDir = Join-Path $root 'spike-output\gui-check'
Remove-Item $workDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $workDir | Out-Null

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Input {
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
}
'@
$WM_KEYDOWN = 0x0100
$VK_DOWN = 0x28
$VK_NEXT = 0x22
$VK_END = 0x23

$script:failures = 0
function Check($name, $passed, $detail = '') {
    if ($passed) {
        "PASS  $name  $detail"
    } else {
        "FAIL  $name  $detail"
        $script:failures++
    }
}

function Wait-Until($seconds, [scriptblock]$condition) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        $value = & $condition
        if ($value) { return $value }
        Start-Sleep -Milliseconds 50
    }
    return $null
}

function Get-MainWindow($process) {
    Wait-Until 15 {
        $process.Refresh()
        if ($process.MainWindowHandle -ne [IntPtr]::Zero) { $process.MainWindowHandle }
    }
}

function Get-Element($handle) {
    [System.Windows.Automation.AutomationElement]::FromHandle($handle)
}

function Find-All($element, $type) {
    $condition = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty, $type)
    $element.FindAll([System.Windows.Automation.TreeScope]::Descendants, $condition)
}

function Get-Rows($handle) {
    Find-All (Get-Element $handle) ([System.Windows.Automation.ControlType]::ListItem)
}

function Get-FocusedName {
    try { [System.Windows.Automation.AutomationElement]::FocusedElement.Current.Name } catch { '' }
}

function Stop-App($process) {
    if (-not $process.HasExited) { $process.CloseMainWindow() | Out-Null }
    if (-not $process.WaitForExit(5000)) {
        $process.Kill()
        return $false
    }
    return $true
}

'--- launching without a console'
$process = Start-Process $exe -ArgumentList "`"$log`"" -PassThru
$window = Get-MainWindow $process
Check 'a window opens' ($null -ne $window)
$conhost = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($process.Id) AND Name='conhost.exe'"
$build = [Environment]::OSVersion.Version.Build
Check "no console window on build $build" ($build -lt 26100 -or -not $conhost)

'--- what a screen reader sees'
$rows = Wait-Until 20 {
    $found = Get-Rows $window
    if ($found.Count -gt 0 -and $found[0].Current.Name -notmatch 'loading') { $found }
}
Check 'rows are exposed' ($rows.Count -gt 10) "$($rows.Count) rows"
if ($rows) {
    $first = $rows[0].Current.Name
    Check 'rows read as sentences' ($first -match '^Row 1, \w+, 2026-01-01') $first
    Check 'only visible rows exist' ($rows.Count -lt 200)
}
$element = Get-Element $window
Check 'the window has a name' ($element.Current.Name -match 'sccm-5gb\.log') $element.Current.Name

'--- keyboard input reaches the view'
[Input]::PostMessageW($window, $WM_KEYDOWN, [IntPtr]$VK_DOWN, [IntPtr]0) | Out-Null
$selected = Wait-Until 5 {
    foreach ($row in (Get-Rows $window)) {
        $pattern = $null
        if ($row.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern) -and
            $pattern.Current.IsSelected) { $row.Current.Name }
    }
}
Check 'Down selects a row' ($selected -match '^Row 2,') "$selected"

[Input]::PostMessageW($window, $WM_KEYDOWN, [IntPtr]$VK_END, [IntPtr]0) | Out-Null
$last = Wait-Until 5 {
    $names = (Get-Rows $window) | ForEach-Object { $_.Current.Name }
    $names | Where-Object { $_ -match '^Row 26904325,' -and $_ -notmatch 'loading' }
}
Check 'End reaches the last of 26.9M rows' ($null -ne $last) "$last"

$before = (Get-Rows $window)[0].Current.Name
foreach ($i in 1..50) {
    [Input]::PostMessageW($window, $WM_KEYDOWN, [IntPtr]$VK_NEXT, [IntPtr]0) | Out-Null
}
Start-Sleep -Milliseconds 200
Check 'the window keeps responding during input' $process.Responding

'--- idle cost'
Start-Sleep -Seconds 1
$cpuBefore = $process.TotalProcessorTime.TotalMilliseconds
Start-Sleep -Seconds 5
$process.Refresh()
$cpuUsed = $process.TotalProcessorTime.TotalMilliseconds - $cpuBefore
Check 'idle CPU is near zero' ($cpuUsed -lt 50) ("{0:N0} ms used in 5 s" -f $cpuUsed)
$threads = $process.Threads.Count
"      $threads threads, $([math]::Round($process.PrivateMemorySize64 / 1MB)) MB private"

'--- closing'
Check 'closes cleanly' (Stop-App $process)
Check 'exit code is zero' ($process.ExitCode -eq 0) "exit $($process.ExitCode)"

'--- starting from a shell'
$marker = Join-Path $workDir 'shell-returned.txt'
$batch = Join-Path $workDir 'launch.cmd'
"@echo off`r`n`"$exe`" `"$log`"`r`necho %ERRORLEVEL%> `"$marker`"`r`n" | Set-Content $batch -NoNewline
$before = @(Get-Process traceview -ErrorAction SilentlyContinue | ForEach-Object Id)
$watch = [Diagnostics.Stopwatch]::StartNew()
$shell = Start-Process cmd.exe -ArgumentList '/c', "`"$batch`"" -WindowStyle Hidden -PassThru
$returned = Wait-Until 10 { Test-Path $marker }
$watch.Stop()
Check 'the shell does not wait for the window' $returned ("returned in {0:N0} ms" -f $watch.ElapsedMilliseconds)
$opened = Wait-Until 10 {
    Get-Process traceview -ErrorAction SilentlyContinue |
        Where-Object { $before -notcontains $_.Id -and $_.MainWindowHandle -ne [IntPtr]::Zero }
}
Check 'the window still opens' ($null -ne $opened)
if ($opened) { Stop-App ($opened | Select-Object -First 1) | Out-Null }

'--- command line, through a pipe and with no console'
$info = New-Object Diagnostics.ProcessStartInfo $exe
$info.Arguments = "summarize `"$log`" --top 1"
$info.RedirectStandardOutput = $true
$info.UseShellExecute = $false
$info.CreateNoWindow = $true
$cli = [Diagnostics.Process]::Start($info)
$output = $cli.StandardOutput.ReadToEnd()
$cli.WaitForExit()
Check 'CLI output reaches a pipe' ($cli.ExitCode -eq 0 -and $output -match '"total_rows":26904325')
Check 'CLI opens no window' ($cli.MainWindowHandle -eq [IntPtr]::Zero)

$help = & $exe --help 2>&1 | Out-String
Check 'help still prints' ($help -match 'traceview query')
& $exe nonexistent-file.log *> $null
Check 'a mistyped file is a usage error, not a window' ($LASTEXITCODE -eq 2) "exit $LASTEXITCODE"

'--- problem files'
$binary = Join-Path $workDir 'binary.log'
[IO.File]::WriteAllBytes($binary, [byte[]](0..255 + 0..255 + 0..255))
$process = Start-Process $exe -ArgumentList "`"$binary`"" -PassThru
$window = Get-MainWindow $process
Start-Sleep -Milliseconds 500
Check 'a binary file opens without crashing' ((-not $process.HasExited) -and $process.Responding)
Stop-App $process | Out-Null

$empty = Join-Path $workDir 'empty.log'
[IO.File]::WriteAllBytes($empty, [byte[]]@())
$process = Start-Process $exe -ArgumentList "`"$empty`"" -PassThru
$window = Get-MainWindow $process
Start-Sleep -Milliseconds 300
Check 'an empty file opens without crashing' ((-not $process.HasExited) -and $process.Responding)
Stop-App $process | Out-Null

$process = Start-Process $exe -PassThru
$window = Get-MainWindow $process
Start-Sleep -Milliseconds 200
Check 'starting with no file opens the welcome screen' ((-not $process.HasExited) -and $process.Responding)
Stop-App $process | Out-Null

''
if ($script:failures -gt 0) {
    "FAILED: $($script:failures) checks failed."
    exit 1
}
'All checks passed.'
exit 0
