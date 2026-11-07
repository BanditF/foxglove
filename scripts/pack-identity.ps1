# Builds Foxglove's sparse identity package (#179) from app\packaging\identity:
#   powershell -File scripts\pack-identity.ps1                     # unsigned, to validate the manifest
#   powershell -File scripts\pack-identity.ps1 -Thumbprint <sha1>  # signed with a certificate from the user's store
#   powershell -File scripts\pack-identity.ps1 -Stage <exe folder> # also copies the msix and Assets\ next to the exe
# Output: target\identity\foxglove-identity.msix (or -Out <folder>). Signing is a human step (#90):
# the certificate's subject must equal the Publisher in AppxManifest.xml, and private keys never
# pass through an agent. -Stage puts the package's logos (Assets\) next to the exe as well: the
# sparse manifest's logo paths resolve in the external location, so without them Settings shows
# a blank icon. The MSI ships the same files (app\wix\main.wxs, -dSparseIdentity).
param([string]$Thumbprint, [string]$Out, [string]$Stage)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.*\x64\makeappx.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $sdk) { throw 'makeappx.exe not found: install the Windows 10/11 SDK.' }
if (-not $Out) { $Out = Join-Path $root 'target\identity' }
New-Item -ItemType Directory -Force $Out | Out-Null
$source = Join-Path $root 'app\packaging\identity'
$msix = Join-Path $Out 'foxglove-identity.msix'
# /nv: a sparse package has no payload (the exe lives outside it), so only the schema is checked.
& $sdk.FullName pack /o /d $source /p $msix /nv
if ($LASTEXITCODE -ne 0) { throw "makeappx failed ($LASTEXITCODE)" }
if ($Thumbprint) {
    $signtool = Join-Path $sdk.DirectoryName 'signtool.exe'
    & $signtool sign /fd SHA256 /sha1 $Thumbprint /tr http://timestamp.digicert.com /td SHA256 $msix
    if ($LASTEXITCODE -ne 0) { throw "signtool failed ($LASTEXITCODE)" }
}
if ($Stage) {
    if (-not (Test-Path (Join-Path $Stage 'foxglove.exe'))) { throw "no foxglove.exe in $Stage" }
    Copy-Item $msix (Join-Path $Stage 'foxglove-identity.msix') -Force
    $assets = Join-Path $Stage 'Assets'
    New-Item -ItemType Directory -Force $assets | Out-Null
    Copy-Item (Join-Path $source 'Assets\*.png') $assets -Force
    Write-Host "Staged the package and its logos in $Stage"
}
Write-Host "Built $msix$(if (-not $Thumbprint) { ' (unsigned: Windows will not register it)' })"
