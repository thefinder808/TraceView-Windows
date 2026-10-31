# Checks the GUI against its budgets on a 5 GiB log, and fails if one is broken.
# Frame times exclude delivering pixels to the screen if the display isn't drawn,
# as when an RDP client is minimized. The script says which case it measured.

$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\release\traceview.exe'
$log = Join-Path $root 'test-data\perf\sccm-5gb.log'
$outDir = Join-Path $root 'spike-output\gui-perf'
Remove-Item $outDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $outDir | Out-Null

cargo build --release -p traceview --features bench 2>&1 | Where-Object { $_ -match '^error' }
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Add-Type -AssemblyName System.Drawing
$drawn = try {
    $check = [System.Drawing.Bitmap]::new(4, 4)
    [System.Drawing.Graphics]::FromImage($check).CopyFromScreen(0, 0, 0, 0, $check.Size)
    $true
} catch { $false }
"Display being drawn: $drawn"

$script:failures = 0
function Check($name, $value, $limit, $unit) {
    $state = if ($value -le $limit) { 'PASS' } else { $script:failures++; 'FAIL' }
    '{0}  {1,-34} {2,8:N2} {3}  (limit {4} {3})' -f $state, $name, $value, $unit, $limit
}

function Invoke-Bench($name, $arguments) {
    $report = Join-Path $outDir "$name.json"
    Remove-Item $report, ([IO.Path]::ChangeExtension($report, 'err')) -ErrorAction SilentlyContinue
    $process = Start-Process $exe -ArgumentList (@('--gui-bench', "`"$report`"") + $arguments) -PassThru
    if (-not $process.WaitForExit(240000)) {
        $process.Kill()
        "FAIL  ${name}: timed out"
        $script:failures++
        return $null
    }
    $errorFile = [IO.Path]::ChangeExtension($report, 'err')
    if (Test-Path $errorFile) {
        "FAIL  ${name}: $(Get-Content $errorFile -Raw)"
        $script:failures++
        return $null
    }
    if (-not (Test-Path $report)) {
        "FAIL  ${name}: no report, exit $($process.ExitCode)"
        $script:failures++
        return $null
    }
    Get-Content $report -Raw | ConvertFrom-Json
}

'--- launch to first frame, median of 8 runs after a warm-up'
$launches = foreach ($run in 1..9) {
    $result = Invoke-Bench 'launch' @('--launch-only')
    if ($result) { $result.launch_ms }
}
$sorted = @($launches | Select-Object -Skip 1 | Sort-Object)
Check 'launch' $sorted[[int]($sorted.Count / 2)] 100 'ms'

# Sizes are device-independent, so these are larger in pixels on a scaled display.
foreach ($size in @(@('1200x760', 8), @('1920x1050', 8))) {
    "--- scrolling at $($size[0])"
    $result = Invoke-Bench "scroll-$($size[0])" @("`"$log`"", '--frames', '300', '--window', $size[0])
    if ($result) {
        foreach ($stage in $result.stages) {
            Check "$($stage.name), worst frame" $stage.frame_ms.max $size[1] 'ms'
            '      p50 {0:N2} ms, p99 {1:N2} ms, {2} of {3} frames still loading rows' -f `
                $stage.frame_ms.p50, $stage.frame_ms.p99, $stage.frames_with_placeholders, $stage.frames
        }
        "      window is $($result.window_pixels -join 'x') pixels"
        Check 'memory beyond the row index' $result.overhead_mb 50 'MB'
        "      $($result.presentation), index $([math]::Round($result.index_mb)) MB, private $([math]::Round($result.private_mb)) MB"
    }
}

# Leave the shipping build in place, not the benchmark build.
cargo build --release -p traceview 2>&1 | Where-Object { $_ -match '^error' }
Check 'EXE size' ((Get-Item $exe).Length / 1MB) 5 'MB'

''
if ($script:failures -gt 0) {
    "FAILED: $($script:failures) budgets broken."
    exit 1
}
'All budgets met.'
