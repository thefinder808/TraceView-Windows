# Writes crates/tv-errors/src/names.rs: the names of Windows Update and
# Delivery Optimization result codes, read from the Windows SDK's wuerror.h and
# deliveryoptimizationerrors.h. Windows has no message text for these codes, so
# a name such as WU_E_PT_WINHTTP_NAME_NOT_RESOLVED is the best explanation
# there is. Only the names are kept, never the headers' descriptions.
#
# Run it again after installing a newer Windows SDK:
#
#   pwsh -File scripts/code-names.ps1 [-Include <SDK include folder>]
param([string]$Include = '')

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$output = Join-Path $root 'crates\tv-errors\src\names.rs'
$headers = 'wuerror.h', 'deliveryoptimizationerrors.h'

if (-not $Include) {
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Include'
    $Include = Get-ChildItem $kits -Directory |
        Where-Object { Test-Path (Join-Path $_.FullName 'um\wuerror.h') } |
        Sort-Object { [version]$_.Name } |
        Select-Object -Last 1 -ExpandProperty FullName
    if (-not $Include) { throw "No Windows SDK with wuerror.h found in $kits" }
}
$version = Split-Path $Include -Leaf

# Lines such as: #define WU_E_NO_SERVICE _HRESULT_TYPEDEF_(0x80240001L)
$pattern = '^\s*#define\s+([A-Z][A-Z0-9_]+)\s+_HRESULT_TYPEDEF_\(0x([0-9A-Fa-f]{8})L\)'
$names = [Collections.Generic.SortedDictionary[uint32, string]]::new()
foreach ($header in $headers) {
    $path = Join-Path $Include "um\$header"
    foreach ($line in Get-Content $path) {
        if ($line -match $pattern) {
            $code = [Convert]::ToUInt32($Matches[2], 16)
            # The first name a code is given wins.
            if (-not $names.ContainsKey($code)) { $names[$code] = $Matches[1] }
        }
    }
}
if ($names.Count -lt 100) { throw "Only $($names.Count) names found, so the headers have changed shape" }

$lines = [Collections.Generic.List[string]]::new()
$lines.Add('//! Names of Windows Update and Delivery Optimization result codes. Windows')
$lines.Add('//! has no message text for these, so a name is the best explanation there')
$lines.Add("//! is. Written by ``scripts/code-names.ps1`` from Windows SDK $version's")
$lines.Add('//! `wuerror.h` and `deliveryoptimizationerrors.h`. Only the names are kept,')
$lines.Add('//! not the headers'' descriptions. Run the script again rather than editing')
$lines.Add('//! this by hand.')
$lines.Add('')
$lines.Add('/// Codes and their names, sorted by code.')
$lines.Add('pub(crate) const NAMES: &[(u32, &str)] = &[')
foreach ($entry in $names.GetEnumerator()) {
    $lines.Add(('    (0x{0:X8}, "{1}"),' -f $entry.Key, $entry.Value))
}
$lines.Add('];')
[IO.File]::WriteAllText($output, ($lines -join "`n") + "`n", [Text.UTF8Encoding]::new($false))
"Wrote $($names.Count) names from SDK $version to $output"
