# Builds the Hideout installer (H5.6) as an UNSIGNED LOCAL artifact:
#   <target dir>\hideout-installer\hideout-x64.msi
# It only builds. It never runs the MSI, hideout.exe or any install step, signs, tags or publishes.
# -DryRun lists the files, registry values and shortcuts the MSI would write and builds nothing
# (no WiX, no cargo needed). Needs WiX Toolset 3.14 (candle.exe, light.exe) on PATH for a real build.
# See docs/packaging.md "Building the Hideout installer".
param(
    [switch]$DryRun,
    [string]$Version = '0.1.0',
    [string]$OutDir
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

if ($DryRun) {
    python (Join-Path $repo 'scripts/hideout_installer_plan.py')
    exit $LASTEXITCODE
}

if ($Version -notmatch '^\d{1,3}\.\d{1,3}\.\d{1,5}$') { throw "Version must be MAJOR.MINOR.PATCH (MSI limits 255/255/65535), got '$Version'" }
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'target' }
if (-not $OutDir) { $OutDir = Join-Path $targetDir 'hideout-installer' }
foreach ($tool in 'candle.exe', 'light.exe') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool not found: install WiX Toolset 3.14 and put its bin folder on PATH (WiX 4+ is not compatible)." }
}

Push-Location $repo
try {
    # Without the `lab-switches` feature: the installed hub ignores the lab, developer, sideload and
    # home-folder environment variables and `--lab` (docs/packaging.md). Never add `--features`.
    cargo build --release --locked -p hideout --no-default-features
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    $exe = Join-Path $targetDir 'release/hideout.exe'
    if (-not (Test-Path $exe)) { throw "No hideout.exe at $exe" }
    $wix = Join-Path $repo 'crates/hideout/wix'
    $obj = Join-Path $OutDir 'obj'
    New-Item -ItemType Directory -Force $obj | Out-Null
    $msi = Join-Path $OutDir 'hideout-x64.msi'
    candle.exe -nologo -arch x64 "-dVersion=$Version" "-dHideoutExe=$exe" -out "$obj\" (Join-Path $wix 'hideout.wxs')
    if ($LASTEXITCODE -ne 0) { throw 'candle failed' }
    # ICE61 (same-version upgrades) is expected, as for Foxglove's MSI.
    light.exe -nologo -ext WixUIExtension -ext WixUtilExtension -sice:ICE61 -out $msi (Join-Path $obj 'hideout.wixobj')
    if ($LASTEXITCODE -ne 0) { throw 'light failed' }
    if (-not (Test-Path $msi)) { throw 'No MSI was built' }
    python (Join-Path $repo 'scripts/msi_tables.py') $msi --product hideout
    if ($LASTEXITCODE -ne 0) { throw 'the built MSI failed its table checks (scripts/msi_tables.py)' }
    Write-Host "Built (unsigned, not installed): $msi"
}
finally { Pop-Location }
