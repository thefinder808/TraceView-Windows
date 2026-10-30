# Compares ways of getting frames to the screen, measured over a real RDP session.
# Each path scrolls at 60 Hz while RDP's own counters record what it sent.
# The session's display must be drawn: don't minimize the RDP client or lock the screen.
# The script waits up to -WaitMinutes for that, and rejects runs where the display drops.

param([int]$WaitMinutes = 0, [switch]$PixelCheckOnly)

$root = Split-Path $PSScriptRoot -Parent
$probe = Join-Path $root 'target\release\d2d-probe.exe'
$log = Join-Path $root 'test-data\perf\sccm-5gb.log'
$outDir = Join-Path $root 'spike-output\presentation'
if (-not $PixelCheckOnly) {
    Remove-Item $outDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $outDir | Out-Null
}

Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System.Collections.Generic;
public static class PixelCheck {
    public static int DistinctColors(int[] pixels) {
        return new HashSet<int>(pixels).Count;
    }
    public static long Differences(int[] a, int[] b) {
        if (a.Length != b.Length) return -1;
        long count = 0;
        for (int i = 0; i < a.Length; i++) if (a[i] != b[i]) count++;
        return count;
    }
}
'@

function Read-Pixels($path) {
    $image = [System.Drawing.Bitmap]::new($path)
    try {
        $bounds = [System.Drawing.Rectangle]::new(0, 0, $image.Width, $image.Height)
        $data = $image.LockBits($bounds, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $pixels = [int[]]::new($image.Width * $image.Height)
        [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $pixels, 0, $pixels.Length)
        $image.UnlockBits($data)
        , $pixels
    } finally {
        $image.Dispose()
    }
}

function Test-DisplayDrawn {
    try {
        $check = [System.Drawing.Bitmap]::new(4, 4)
        [System.Drawing.Graphics]::FromImage($check).CopyFromScreen(0, 0, 0, 0, $check.Size)
        $true
    } catch {
        $false
    }
}

if ($PixelCheckOnly) {
    $files = Get-ChildItem (Join-Path $root 'spike-output') -Filter 'text-d2d-*.png' | Select-Object -First 2
    $first = Read-Pixels $files[0].FullName
    $second = Read-Pixels $files[1].FullName
    "$($files[0].Name): $([PixelCheck]::DistinctColors($first)) colors, $([PixelCheck]::Differences($first, $first)) differences with itself"
    "$($files[1].Name): $([PixelCheck]::Differences($first, $second)) differences with the first"
    exit 0
}

# Wait for a drawn display rather than report numbers from one that isn't.
$deadline = (Get-Date).AddMinutes($WaitMinutes)
while (-not (Test-DisplayDrawn)) {
    if ((Get-Date) -gt $deadline) {
        "STOPPED: the display wasn't drawn within $WaitMinutes minutes. Restore the RDP window and rerun."
        exit 1
    }
    Start-Sleep -Seconds 5
}
"Display is being drawn. Starting at $(Get-Date -Format T)."
Start-Sleep -Seconds 5

function Get-SentBytes {
    (Get-Counter '\RemoteFX Network(*)\Total Sent Bytes').CounterSamples |
        Where-Object InstanceName -Match 'rdp' |
        Measure-Object CookedValue -Sum |
        Select-Object -ExpandProperty Sum
}

$paths = @(
    @{ Name = 'hwnd-software'; Args = @('--software') },
    @{ Name = 'hwnd-default'; Args = @() },
    @{ Name = 'flip-warp'; Args = @('--flip', 'warp') },
    @{ Name = 'flip-warp-hints'; Args = @('--flip', 'warp', '--scroll-hints') },
    @{ Name = 'flip-hardware'; Args = @('--flip', 'hardware') },
    @{ Name = 'flip-hardware-hints'; Args = @('--flip', 'hardware', '--scroll-hints') }
)

'--- launch to first present, median of 8 runs after one warm-up'
foreach ($path in $paths | Where-Object Name -NotMatch 'hints') {
    $out = Join-Path $outDir "launch-$($path.Name).json"
    $times = foreach ($i in 1..9) {
        Remove-Item $out -ErrorAction SilentlyContinue
        $process = Start-Process $probe -ArgumentList (@('--bench', "`"$out`"", '--launch-only') + $path.Args) -PassThru
        if ($process.WaitForExit(30000) -and (Test-Path $out)) {
            (Get-Content $out -Raw | ConvertFrom-Json).launch_to_first_present_ms
        }
    }
    $sorted = @($times | Select-Object -Skip 1 | Sort-Object)
    if ($sorted.Count) {
        '{0,-20} {1,5:N0} ms (best {2:N0}, worst {3:N0})' -f $path.Name, $sorted[[int]($sorted.Count / 2)], $sorted[0], $sorted[-1]
    } else {
        '{0,-20} FAILED' -f $path.Name
    }
}

'--- idle baseline, nothing scrolling'
$before = Get-SentBytes
Start-Sleep -Seconds 10
'{0:N1} KB/s sent while idle' -f (((Get-SentBytes) - $before) / 10 / 1KB)

'--- 60 Hz wheel scrolling, 10 seconds per run, two rounds'
foreach ($round in 1, 2) {
    foreach ($path in $paths) {
        $name = "$($path.Name)-$round"
        $out = Join-Path $outDir "$name.json"
        $argList = @("`"$log`"", '--bench', "`"$out`"", '--frames', '600', '--stage', 'wheel', '--paced', '--capture') + $path.Args
        $sentBefore = Get-SentBytes
        $process = Start-Process $probe -ArgumentList $argList -PassThru
        $null = $process.Handle
        Start-Sleep -Seconds 1
        $samples = (Get-Counter -SampleInterval 1 -MaxSamples 8 -Counter @(
                '\RemoteFX Graphics(*)\Input Frames/Second',
                '\RemoteFX Graphics(*)\Output Frames/Second',
                '\RemoteFX Graphics(*)\Average Encoding Time',
                '\RemoteFX Graphics(*)\Frame Quality',
                '\RemoteFX Graphics(*)\Frames Skipped/Second - Insufficient Server Resources',
                '\RemoteFX Graphics(*)\Frames Skipped/Second - Insufficient Network Resources',
                '\RemoteFX Graphics(*)\Frames Skipped/Second - Insufficient Client Resources'
            )).CounterSamples | Where-Object InstanceName -Match 'rdp'
        if (-not $process.WaitForExit(60000)) { $process.Kill(); "${name}: TIMED OUT"; continue }
        $cpuSeconds = $process.TotalProcessorTime.TotalSeconds
        $sent = (Get-SentBytes) - $sentBefore
        $err = [IO.Path]::ChangeExtension($out, 'err')
        if (Test-Path $err) { "${name}: ERROR $(Get-Content $err -Raw)"; continue }
        if (-not (Test-Path $out)) { "${name}: no report, exit $($process.ExitCode)"; continue }
        $report = Get-Content $out -Raw | ConvertFrom-Json
        $average = {
            param($pattern)
            ($samples | Where-Object Path -Match $pattern | Measure-Object CookedValue -Average).Average
        }
        $capture = Join-Path $outDir "$name-end.png"
        if (-not (Test-DisplayDrawn)) { "${name}: REJECTED, the display stopped being drawn"; continue }
        '{0,-23} {1,6:N0} KB/s | RDP in {2,3:N0} out {3,3:N0} fps | skipped server {4,3:N0} net {5,3:N0} client {6,3:N0} | encode {7,5:N1} ms | quality {8,3:N0} | frame p50 {9,5:N2} p99 {10,5:N2} ms | cpu {11,5:N1}s | priv {12,4:N0} MB | colors {13}' -f `
            $name, ($sent / 10 / 1KB),
            (& $average 'input frames'), (& $average 'output frames'),
            (& $average 'server resources'), (& $average 'network resources'), (& $average 'client resources'),
            (& $average 'encoding time'), (& $average 'frame quality'),
            $report.stages.wheel.frame_ms.p50, $report.stages.wheel.frame_ms.p99,
            $cpuSeconds, $report.private_mb, [PixelCheck]::DistinctColors((Read-Pixels $capture))
    }
}

'--- on-screen pixels compared with hwnd-software at the same row (0 means identical)'
$reference = Read-Pixels (Join-Path $outDir 'hwnd-software-1-end.png')
foreach ($path in $paths) {
    $capture = Join-Path $outDir "$($path.Name)-1-end.png"
    if (Test-Path $capture) {
        '{0,-20} {1} differing pixels' -f $path.Name, [PixelCheck]::Differences($reference, (Read-Pixels $capture))
    }
}

'--- unpaced cost of drawing and presenting, all scroll stages'
foreach ($path in $paths | Where-Object Name -NotMatch 'hints') {
    if (-not (Test-DisplayDrawn)) { "$($path.Name): REJECTED, the display stopped being drawn"; continue }
    $out = Join-Path $outDir "unpaced-$($path.Name).json"
    $process = Start-Process $probe -ArgumentList (@("`"$log`"", '--bench', "`"$out`"", '--frames', '300') + $path.Args) -PassThru
    if (-not $process.WaitForExit(120000)) { $process.Kill(); "$($path.Name): TIMED OUT"; continue }
    if (-not (Test-Path $out)) { "$($path.Name): no report"; continue }
    $stages = (Get-Content $out -Raw | ConvertFrom-Json).stages
    '{0,-20} wheel p50 {1,6:N2} p99 {2,6:N2} | jump p50 {3,6:N2} p99 {4,6:N2} ms' -f $path.Name,
        $stages.wheel.frame_ms.p50, $stages.wheel.frame_ms.p99,
        $stages.random_jump.frame_ms.p50, $stages.random_jump.frame_ms.p99
}
