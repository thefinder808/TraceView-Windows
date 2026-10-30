# Head-to-head: the egui app versus the Direct2D probe on the same log and script.
# Each row reports scroll frame times and the CPU time each frame costs.

$root = Split-Path $PSScriptRoot -Parent
$egui = Join-Path $root 'target\release\traceview.exe'
$d2d = Join-Path $root 'target\release\d2d-probe.exe'
$outDir = Join-Path $root 'spike-output'
$log = Join-Path $root 'test-data\perf\sccm-5gb.log'
New-Item -ItemType Directory -Force $outDir | Out-Null

function Invoke-Run($name, $exe, $extra, $frames, $affinity) {
    $out = Join-Path $outDir "$name.json"
    Remove-Item $out -ErrorAction SilentlyContinue
    $argLine = "`"$log`" --bench `"$out`" --frames $frames $($extra -join ' ')"
    if ($affinity) {
        # `start /affinity` applies from process creation, so the thread pool sees it too.
        $marker = "start `"`" /wait /affinity $affinity `"$exe`" $argLine"
        $sw = [Diagnostics.Stopwatch]::StartNew()
        cmd /c $marker
        $sw.Stop()
        $cpu = $null
    } else {
        $process = Start-Process $exe -ArgumentList $argLine -PassThru
        $null = $process.Handle
        if (-not $process.WaitForExit(280000)) { $process.Kill(); "${name}: TIMED OUT"; return }
        $cpu = $process.TotalProcessorTime.TotalMilliseconds
    }
    if (-not (Test-Path $out)) { "${name}: no report"; return }
    $r = Get-Content $out -Raw | ConvertFrom-Json
    $wheel = $r.stages.wheel.frame_ms
    $jump = $r.stages.random_jump.frame_ms
    $sweep = $r.stages.scrollbar_sweep.frame_ms
    $cpuText = if ($cpu) { '{0,8:N0}s cpu' -f ($cpu / 1000) } else { '' }
    '{0,-26} wheel p50 {1,7:N1} p99 {2,7:N1} | jump p50 {3,7:N1} p99 {4,7:N1} | sweep p99 {5,7:N1} ms | fps(p50 jump) {6,6:N0} | priv {7,5:N0} MB {8}' -f `
        $name, $wheel.p50, $wheel.p99, $jump.p50, $jump.p99, $sweep.p99, (1000 / $jump.p50), $r.private_mb, $cpuText
}

function Measure-Launch($name, $exe, $extra, $field) {
    $out = Join-Path $outDir "launch-$name.json"
    $times = foreach ($i in 1..9) {
        Remove-Item $out -ErrorAction SilentlyContinue
        $process = Start-Process $exe -ArgumentList (@('--bench', "`"$out`"", '--launch-only') + $extra) -PassThru
        if ($process.WaitForExit(30000) -and (Test-Path $out)) {
            (Get-Content $out -Raw | ConvertFrom-Json).$field
        }
    }
    $sorted = $times | Select-Object -Skip 1 | Sort-Object
    '{0,-26} launch median {1,5:N0} ms, best {2,5:N0} ms, worst {3,5:N0} ms' -f $name, $sorted[[int]($sorted.Count / 2)], $sorted[0], $sorted[-1]
}

"--- launch, nothing else running"
Measure-Launch 'egui-wgpu-gpu' $egui @('--renderer', 'wgpu') 'launch_to_second_frame_ms'
Measure-Launch 'egui-glow-gpu' $egui @('--renderer', 'glow') 'launch_to_second_frame_ms'
Measure-Launch 'egui-wgpu-software' $egui @('--renderer', 'wgpu', '--software-adapter') 'launch_to_second_frame_ms'
Measure-Launch 'd2d-default' $d2d @() 'launch_to_first_present_ms'
Measure-Launch 'd2d-software' $d2d @('--software') 'launch_to_first_present_ms'

"--- scrolling, 24 threads"
Invoke-Run 'cmp-egui-gpu' $egui @('--renderer', 'wgpu') 600 $null
Invoke-Run 'cmp-d2d-default' $d2d @() 600 $null
Invoke-Run 'cmp-egui-software' $egui @('--renderer', 'wgpu', '--software-adapter') 120 $null
Invoke-Run 'cmp-d2d-software' $d2d @('--software') 600 $null

"--- software rendering at 2500x1300, 24 threads"
Invoke-Run 'cmp-egui-software-large' $egui @('--renderer', 'wgpu', '--software-adapter', '--window', '2500x1300') 60 $null
Invoke-Run 'cmp-d2d-software-large' $d2d @('--software', '--window', '2500x1300') 600 $null

"--- software rendering, pinned to 2 cores like a small VM"
Invoke-Run 'cmp-egui-software-2core' $egui @('--renderer', 'wgpu', '--software-adapter') 30 '3'
Invoke-Run 'cmp-d2d-software-2core' $d2d @('--software') 300 '3'
