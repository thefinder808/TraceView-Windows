# Builds traceview.exe for x64 and ARM64, then packages each in a zip with
# its licenses, plus SHA256SUMS.txt, all in dist\. With no switches it does
# both, unsigned, for trying a release build locally:
#
#   pwsh -File scripts/build-release.ps1
#
# The release workflow builds with -BuildOnly in a job that has no access to
# signing, signs the EXEs in another job, then packages them there with
# -PackageOnly -RequireSigned. -Tag makes sure a tag like v0.9.0 matches the
# version in Cargo.toml, and that CHANGELOG.md has a "## 0.9.0" section.
# Packaging then writes that section to dist\release-notes.md, as the
# release's notes.
param(
    [switch]$BuildOnly,
    [switch]$PackageOnly,
    [switch]$RequireSigned,
    [string]$Tag
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$dist = Join-Path $root 'dist'
$targets = [ordered]@{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }

$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
if ($Tag) {
    if ($Tag -notmatch '^v\d+\.\d+\.\d+$') { throw "A release tag looks like v1.2.3, not '$Tag'" }
    if ($Tag -ne "v$version") { throw "The tag is $Tag, but Cargo.toml says $version" }

    # The release's notes: everything from "## <version>" to the next "## ".
    # Checked before building, so a release can't get as far as signing
    # without them.
    $notes = $null
    $inSection = $false
    foreach ($line in Get-Content (Join-Path $root 'CHANGELOG.md') -Encoding utf8) {
        if ($line -match '^## ') {
            if ($inSection) { break }
            $inSection = $line -match "^## $([regex]::Escape($version))(\s|$)"
            if ($inSection) { $notes = [Collections.Generic.List[string]]::new() }
        } elseif ($inSection) {
            $notes.Add($line)
        }
    }
    if (-not $notes -or -not ($notes -join '').Trim()) {
        throw "CHANGELOG.md needs a '## $version' section, which becomes the release's notes"
    }
}

if (-not $PackageOnly) {
    if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
    foreach ($arch in $targets.Keys) {
        "Building $version for $arch"
        cargo build --release --locked -p traceview --target $targets[$arch]
        if ($LASTEXITCODE -ne 0) { throw "The $arch build failed" }
        $folder = New-Item -ItemType Directory -Force (Join-Path $dist $arch)
        Copy-Item (Join-Path $root "target\$($targets[$arch])\release\traceview.exe") $folder
    }
}
if ($BuildOnly) { return }

# signtool is the check Windows itself makes. It isn't on the PATH, so take
# the newest one in the Windows SDK, if there is one.
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName | Select-Object -Last 1

$sums = [Collections.Generic.List[string]]::new()
foreach ($arch in $targets.Keys) {
    $exe = Join-Path $dist "$arch\traceview.exe"
    if (-not (Test-Path $exe)) { throw "There's no $exe to package. Build first." }
    $signature = Get-AuthenticodeSignature $exe
    if ($signature.Status -eq 'Valid' -and $signature.TimeStamperCertificate) {
        Write-Host "$arch is signed by $($signature.SignerCertificate.Subject)"
        if ($signtool) {
            & $signtool.FullName verify /pa /q $exe
            if ($LASTEXITCODE -ne 0) { throw "signtool rejects the signature on $exe" }
        }
    } elseif ($RequireSigned) {
        # Only the status: Windows' message for it is about running scripts.
        $state = if ($signature.Status -eq 'Valid') { 'signed but not timestamped' } else { $signature.Status }
        throw "$exe isn't signed and timestamped ($state)"
    } else {
        Write-Warning "$arch is NOT signed. Don't share this build."
    }

    $name = "TraceView-$version-$arch"
    $folder = New-Item -ItemType Directory -Force (Join-Path $dist $name)
    Copy-Item $exe, (Join-Path $root 'LICENSE'), (Join-Path $root 'THIRD-PARTY-NOTICES.txt') $folder
    $zip = Join-Path $dist "$name.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path (Join-Path $folder '*') -DestinationPath $zip
    Remove-Item $folder -Recurse -Force
    $sums.Add(('{0}  {1}' -f (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant(), (Split-Path $zip -Leaf)))
}
# The format sha256sum -c reads.
[IO.File]::WriteAllText((Join-Path $dist 'SHA256SUMS.txt'), ($sums -join "`n") + "`n")
"Wrote $((Get-ChildItem $dist -Filter *.zip).Name -join ', ') and SHA256SUMS.txt in $dist"
if ($notes) {
    [IO.File]::WriteAllText((Join-Path $dist 'release-notes.md'), ($notes -join "`n").Trim() + "`n")
    "Wrote release-notes.md from CHANGELOG.md's $version section"
}
