# Builds Foxglove's sparse identity package (#179) from app\packaging\identity:
#   powershell -File scripts\pack-identity.ps1                  # unsigned, to validate the manifest
#   powershell -File scripts\pack-identity.ps1 -Thumbprint <sha1>  # signed with a certificate from the user's store
# Output: target\identity\foxglove-identity.msix. Signing is a human step (#90): the certificate's
# subject must equal the Publisher in AppxManifest.xml, and private keys never pass through an agent.
param([string]$Thumbprint)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.*\x64\makeappx.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
if (-not $sdk) { throw 'makeappx.exe not found: install the Windows 10/11 SDK.' }
$out = Join-Path $root 'target\identity'
New-Item -ItemType Directory -Force $out | Out-Null
$msix = Join-Path $out 'foxglove-identity.msix'
& $sdk.FullName pack /o /d (Join-Path $root 'app\packaging\identity') /p $msix /nv
if ($LASTEXITCODE -ne 0) { throw "makeappx failed ($LASTEXITCODE)" }
if ($Thumbprint) {
    $signtool = Join-Path $sdk.DirectoryName 'signtool.exe'
    & $signtool sign /fd SHA256 /sha1 $Thumbprint /tr http://timestamp.digicert.com /td SHA256 $msix
    if ($LASTEXITCODE -ne 0) { throw "signtool failed ($LASTEXITCODE)" }
}
Write-Host "Built $msix$(if (-not $Thumbprint) { ' (unsigned: Windows will not register it)' })"
