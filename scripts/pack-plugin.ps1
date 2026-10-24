# Builds a plugin crate and packs it for the Plugins tab: plugin.toml plus its program, zipped, with the
# catalog entry (archive and file SHA-256s) to add to catalog.json before signing (#159).
#   powershell -File scripts\pack-plugin.ps1 plugin-focus-timer
param([Parameter(Mandatory)] [string]$Crate)
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $dir = "crates\$Crate"
    $manifest = Get-Content "$dir\plugin.toml" -Raw
    $id = [regex]::Match($manifest, '(?m)^id\s*=\s*"([^"]+)"').Groups[1].Value
    $version = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
    $program = [regex]::Match($manifest, '(?m)^run\s*=\s*\[\s*"([^"]+)"').Groups[1].Value
    cargo build --release -p $Crate
    if ($LASTEXITCODE) { throw "build failed" }

    # Honour a shared CARGO_TARGET_DIR (the Test lab uses one).
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    $stage = "$target\plugins\$id"
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item "$dir\plugin.toml" $stage
    Copy-Item "$target\release\$program" $stage
    $zip = "$target\plugins\$id-$version.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path "$stage\*" -DestinationPath $zip

    $files = [ordered]@{}
    Get-ChildItem $stage -File -Recurse | Sort-Object FullName | ForEach-Object {
        $rel = $_.FullName.Substring((Resolve-Path $stage).Path.Length + 1).Replace('\', '/')
        $files[$rel] = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()
    }
    $entry = [ordered]@{
        id = $id
        version = $version
        sha256 = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
        files = $files
    }
    Write-Host "Packed $zip"
    Write-Host "Catalog entry:"
    $entry | ConvertTo-Json -Depth 4
} finally {
    Pop-Location
}
