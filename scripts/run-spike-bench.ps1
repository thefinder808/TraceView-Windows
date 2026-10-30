# Runs the spike's benchmark matrix and prints one summary line per run.
# Reports go to spike-output\. Assumes a release build and the generated logs.

$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'target\release\traceview.exe'
$coldRead = Join-Path $root 'target\release\examples\cold_read.exe'
$outDir = Join-Path $root 'spike-output'
$cacheDir = Join-Path $env:LOCALAPPDATA 'TraceView\indexes'
New-Item -ItemType Directory -Force $outDir | Out-Null

function Invoke-Bench($name, $log, $extra) {
    $out = Join-Path $outDir "$name.json"
    Remove-Item $out, ([IO.Path]::ChangeExtension($out, 'err')) -ErrorAction SilentlyContinue
    $argList = @("`"$log`"", '--bench', "`"$out`"") + $extra
    $process = Start-Process $exe -ArgumentList $argList -PassThru
    if (-not $process.WaitForExit(300000)) {
        $process.Kill()
        "${name}: TIMED OUT"
        return
    }
    $errFile = [IO.Path]::ChangeExtension($out, 'err')
    if (Test-Path $errFile) { "${name}: ERROR $(Get-Content $errFile -Raw)" }
    if (-not (Test-Path $out)) { "${name}: no report, exit $($process.ExitCode)"; return }
    $r = Get-Content $out -Raw | ConvertFrom-Json
    $s = $r.stages
    '{0,-22} open {1,6:N2}s cache={2,-5} rows={3,9} | frame p99/max ms: wheel {4:N1}/{5:N1} page {6:N1}/{7:N1} jump {8:N1}/{9:N1} sweep {10:N1}/{11:N1} | filter s: {12:N2} {13:N2} {14:N2} | priv {15:N0} MB idx {16:N0} MB | {17}' -f `
        $name, $r.open_seconds, $r.cache_hit, $r.rows,
        $s.wheel.frame_ms.p99, $s.wheel.frame_ms.max,
        $s.page_down.frame_ms.p99, $s.page_down.frame_ms.max,
        $s.random_jump.frame_ms.p99, $s.random_jump.frame_ms.max,
        $s.scrollbar_sweep.frame_ms.p99, $s.scrollbar_sweep.frame_ms.max,
        $s.filter_literal_rare.seconds, $s.filter_regex_error_code.seconds, $s.filter_regex_miss.seconds,
        $r.private_mb, $r.index_mb, $r.adapter
}

foreach ($kind in 'sccm', 'plain') {
    $log = Join-Path $root "test-data\perf\$kind-5gb.log"

    # Cold disk with no index cache: the true first open.
    Remove-Item $cacheDir -Recurse -Force -ErrorAction SilentlyContinue
    & $coldRead evict $log
    Invoke-Bench "$kind-cold-nocache" $log @()

    # Warm disk with a cached index: the common reopen.
    Invoke-Bench "$kind-warm-cached" $log @()

    # Cold log and cold index cache: reopening after a reboot.
    & $coldRead evict $log
    Get-ChildItem $cacheDir -Filter *.idx | ForEach-Object { & $coldRead evict $_.FullName }
    Invoke-Bench "$kind-cold-cached" $log @()
}

$log = Join-Path $root 'test-data\perf\sccm-5gb.log'
Invoke-Bench 'software-1400x850' $log @('--software-adapter')
Invoke-Bench 'software-2500x1300' $log @('--software-adapter', '--window', '2500x1300')
Invoke-Bench 'glow-1400x850' $log @('--renderer', 'glow')
