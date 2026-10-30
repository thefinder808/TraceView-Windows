# Runs the engine's performance limits against 5 GiB logs, in both main formats.
# Generates the logs if they're missing. Reads them first, so the limits measure
# the engine and not the disk. Exits non-zero if a limit is broken.

$root = Split-Path $PSScriptRoot -Parent
$perfDir = Join-Path $root 'test-data\perf'
New-Item -ItemType Directory -Force $perfDir | Out-Null

cargo build --release -p tv-engine --example gen_log
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$generator = Join-Path $root 'target\release\examples\gen_log.exe'

$failed = $false
foreach ($kind in 'sccm', 'plain') {
    $log = Join-Path $perfDir "$kind-5gb.log"
    if (-not (Test-Path $log)) {
        & $generator $kind 5 $log
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    }

    # Warm the disk cache, because cold-disk speed is limited by the drive.
    $stream = [IO.File]::OpenRead($log)
    $buffer = [byte[]]::new(16MB)
    while ($stream.Read($buffer, 0, $buffer.Length) -gt 0) { }
    $stream.Dispose()

    "=== $kind ==="
    $env:TV_PERF_LOG = $log
    cargo test --release -p tv-engine --test perf_budgets -- --ignored --nocapture --test-threads=1 2>&1 |
        Where-Object { $_ -match 'limit|bytes per row|^test |test result|panicked' }
    if ($LASTEXITCODE -ne 0) { $failed = $true }
}
Remove-Item Env:TV_PERF_LOG
if ($failed) {
    'FAILED: a performance limit was broken.'
    exit 1
}
'All performance limits passed.'
