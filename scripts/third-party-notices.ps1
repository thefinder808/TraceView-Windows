# Writes THIRD-PARTY-NOTICES.txt: every crate built into traceview.exe, for x64
# and ARM64, with its license files. The EXE carries the file, so
# `traceview --licenses` can show it wherever the EXE is copied. With -Check,
# it fails if the file is out of date instead of writing it, which CI uses.
#
#   pwsh -File scripts/third-party-notices.ps1 [-Check]
param([switch]$Check)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$output = Join-Path $root 'THIRD-PARTY-NOTICES.txt'

# Crates from crates.io that traceview uses as normal dependencies, directly or
# through others. Build and test dependencies don't end up in the EXE.
$packages = @{}
foreach ($target in 'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc') {
    $json = cargo metadata --format-version 1 --locked --filter-platform $target `
        --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $metadata = $json | ConvertFrom-Json
    $byId = @{}
    foreach ($package in $metadata.packages) { $byId[$package.id] = $package }
    $nodes = @{}
    foreach ($node in $metadata.resolve.nodes) { $nodes[$node.id] = $node }
    $queue = [Collections.Generic.Queue[string]]::new()
    $queue.Enqueue(($metadata.packages | Where-Object name -EQ 'traceview').id)
    $seen = @{}
    while ($queue.Count -gt 0) {
        $id = $queue.Dequeue()
        if ($seen.ContainsKey($id)) { continue }
        $seen[$id] = $true
        foreach ($dependency in $nodes[$id].deps) {
            if ($dependency.dep_kinds | Where-Object { $null -eq $_.kind }) {
                $queue.Enqueue($dependency.pkg)
            }
        }
    }
    foreach ($id in $seen.Keys) {
        if ($byId[$id].source) { $packages[$id] = $byId[$id] }
    }
}

# Each distinct license text once, with the crates it covers.
$texts = [ordered]@{}
$list = foreach ($package in $packages.Values | Sort-Object name, version) {
    $name = "$($package.name) $($package.version)"
    $folder = Split-Path $package.manifest_path
    $files = @(Get-ChildItem $folder -File |
            Where-Object Name -Match '^(licen[cs]e|copying|copyright|notice|unlicense)' |
            Sort-Object Name)
    if ($package.license_file) { $files += Get-Item (Join-Path $folder $package.license_file) }
    # Some crates are published without their repository's license files.
    # Copies taken from the repository at the released tag sit in licenses\,
    # in a folder named after the repository.
    $kept = Join-Path $root ('licenses\' + ($package.repository -split '/')[-1])
    if ($files.Count -eq 0 -and $package.repository -and (Test-Path $kept)) {
        $files = @(Get-ChildItem $kept -File | Sort-Object Name)
    }
    if ($files.Count -eq 0) { throw "$name has no license file to include" }
    foreach ($file in $files | Sort-Object FullName -Unique) {
        $text = ((Get-Content $file.FullName -Raw) -replace "`r`n", "`n").Trim()
        if (-not $texts.Contains($text)) { $texts[$text] = [Collections.Generic.List[string]]::new() }
        $texts[$text].Add("$name ($($file.Name))")
    }
    '{0,-36} {1}' -f $name, $package.license
}

$rule = '=' * 78
$notices = @(
    'Third-party notices for TraceView'
    ''
    'TraceView is built with these open-source Rust crates. Their licenses follow,'
    'each with the crates it covers.'
    ''
    $list
    foreach ($entry in $texts.GetEnumerator()) {
        ''
        $rule
        "Covers: $($entry.Value -join ', ')"
        $rule
        ''
        $entry.Key
    }
) -join "`n"
$notices += "`n"

if ($Check) {
    $current = if (Test-Path $output) { (Get-Content $output -Raw) -replace "`r`n", "`n" } else { '' }
    if ($current -ne $notices) {
        throw 'THIRD-PARTY-NOTICES.txt is out of date. Run scripts/third-party-notices.ps1 and commit it.'
    }
    'THIRD-PARTY-NOTICES.txt is up to date.'
} else {
    [IO.File]::WriteAllText($output, $notices)
    'Wrote {0}: {1} crates, {2} license texts, {3:N0} KB' -f $output, $packages.Count, $texts.Count, ($notices.Length / 1KB)
}
