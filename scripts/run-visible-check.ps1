# Measures scrolling on a display that is actually being drawn.
# Keep this window's desktop visible while it runs: don't minimize the RDP client
# or lock the screen. Watch each window and check that rows really scroll.

$root = Split-Path $PSScriptRoot -Parent
$egui = Join-Path $root 'target\release\traceview.exe'
$d2d = Join-Path $root 'target\release\d2d-probe.exe'
$log = Join-Path $root 'test-data\perf\sccm-5gb.log'
$outDir = Join-Path $root 'spike-output'
New-Item -ItemType Directory -Force $outDir | Out-Null

Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
public static class VisibleCheck {
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
}
'@
"Remote session: $([VisibleCheck]::GetSystemMetrics(0x1000) -ne 0)"

function Invoke-Check($name, $exe, $extra, $frames) {
    $out = Join-Path $outDir "visible-$name.json"
    $err = [IO.Path]::ChangeExtension($out, 'err')
    Remove-Item $out, $err -ErrorAction SilentlyContinue
    $process = Start-Process $exe -ArgumentList (@("`"$log`"", '--bench', "`"$out`"", '--frames', $frames) + $extra) -PassThru
    if (-not $process.WaitForExit(280000)) { $process.Kill(); "${name}: TIMED OUT"; return }
    if (Test-Path $err) { "${name}: ERROR $(Get-Content $err -Raw)"; return }
    if (-not (Test-Path $out)) { "${name}: no report"; return }
    $jump = (Get-Content $out -Raw | ConvertFrom-Json).stages.random_jump.frame_ms
    '{0,-24} random jump p50 {1,7:N1} ms, p99 {2,7:N1} ms' -f $name, $jump.p50, $jump.p99
}

Invoke-Check 'direct2d-software' $d2d @('--software') 300
Invoke-Check 'direct2d-default' $d2d @() 300
Invoke-Check 'egui-default' $egui @() 300
Invoke-Check 'egui-software' $egui @('--software-adapter') 60
