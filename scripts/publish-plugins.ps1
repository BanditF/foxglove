# Publishes plugins (#190): zips on GitHub Releases, the signed catalog on GitHub Pages.
#
#   powershell -File scripts\publish-plugins.ps1 -Crate plugin-focus-timer [-Channel beta]
#       Dry run: packs, works out tags and URLs, and updates the catalog in target\plugin-site.
#   ... -Release      also creates the releases (plugin-<slug>-vX.Y.Z, never "latest") and checks
#                     the uploaded bytes by downloading them back.
#   powershell -File scripts\publish-plugins.ps1 -Resign
#       Only bumps the sequence and expiry (the weekly re-sign).
#   powershell -File scripts\publish-plugins.ps1 -Pages
#       Pushes target\plugin-site\plugins\v2 to the gh-pages branch, once catalog.json is signed.
#
# Signing is the maintainer's step, with the offline-held catalog key; the script prints the
# command and never reads a key. Order: zips first, verified, then the signed catalog.
param(
    [string[]]$Crate = @(),
    [ValidateSet('stable', 'beta')] [string]$Channel = 'stable',
    [switch]$Release,
    [switch]$Resign,
    [switch]$Pages,
    [int]$Days = 30,
    [string]$Repo = 'BanditF/foxglove',
    [string]$Base = 'https://banditf.github.io/foxglove/plugins/v2/'
)
$ErrorActionPreference = 'Stop'
# -File passes 'a,b' as one string.
$Crate = @($Crate | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
    $site = Join-Path $target 'plugin-site\plugins\v2'
    New-Item -ItemType Directory -Force $site | Out-Null
    $catalog = Join-Path $site 'catalog.json'

    & "$PSScriptRoot\build-slot.ps1" cargo build -q -p foxglove-plugin-trust --bin plugin-catalog
    if ($LASTEXITCODE) { throw 'building plugin-catalog failed' }
    $tool = Join-Path $target 'debug\plugin-catalog.exe'
    function Catalog { & $tool @args; if ($LASTEXITCODE) { throw "plugin-catalog $($args[0]) failed" } }

    # Start from what's published (the root and the current catalog), or a new catalog.
    if (-not (Test-Path $catalog)) {
        foreach ($name in 'root.json', 'root.json.minisig', 'catalog.json') {
            try { Invoke-WebRequest "$Base$name" -OutFile (Join-Path $site $name) -UseBasicParsing | Out-Null }
            catch { Write-Host "  ($name not published yet)" }
        }
        if (-not (Test-Path $catalog)) {
            $epoch = 0
            $rootJson = Join-Path $site 'root.json'
            if (Test-Path $rootJson) { $epoch = (Get-Content $rootJson -Raw | ConvertFrom-Json).epoch }
            Catalog new $catalog --epoch $epoch
        }
    }

    if ($Pages) {
        $sig = "$catalog.minisig"
        if (-not (Test-Path $sig) -or (Get-Item $sig).LastWriteTime -lt (Get-Item $catalog).LastWriteTime) {
            throw 'catalog.json is not signed (or changed since): sign it first'
        }
        $pagesDir = Join-Path $target 'gh-pages'
        git fetch -q origin gh-pages 2>$null
        if (Test-Path $pagesDir) { git worktree remove --force $pagesDir }
        if ($LASTEXITCODE -eq 0 -and (git ls-remote --heads origin gh-pages)) {
            git worktree add -q $pagesDir origin/gh-pages
            Push-Location $pagesDir; git checkout -q -B gh-pages; Pop-Location
        } else {
            git worktree add -q --orphan -b gh-pages $pagesDir
        }
        $dest = Join-Path $pagesDir 'plugins\v2'
        New-Item -ItemType Directory -Force $dest | Out-Null
        Copy-Item (Join-Path $site '*') $dest -Force
        Push-Location $pagesDir
        git add plugins/v2
        git commit -q -m "Plugin catalog $((Get-Content $catalog -Raw | ConvertFrom-Json).sequence)"
        git push -q origin gh-pages
        Pop-Location
        git worktree remove --force $pagesDir
        Write-Host "Published $Base"
        return
    }

    foreach ($c in $Crate) {
        powershell -NoProfile -ExecutionPolicy Bypass -File scripts\pack-plugin.ps1 $c | Out-Null
        if ($LASTEXITCODE) { throw "packing $c failed" }
        $manifest = Get-Content "crates\$c\plugin.toml" -Raw
        $id = [regex]::Match($manifest, '(?m)^id\s*=\s*"([^"]+)"').Groups[1].Value
        $version = [regex]::Match($manifest, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
        $slug = $id.Split('.')[-1]
        $tag = "plugin-$slug-v$version"
        $asset = "$id-$version-windows-x86_64.zip"
        $zip = Join-Path $target "plugins\$asset"
        Copy-Item (Join-Path $target "plugins\$id-$version.zip") $zip -Force
        $url = "https://github.com/$Repo/releases/download/$tag/$asset"
        $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
        Write-Host "$id $version ($Channel): $tag, $hash"

        if ($Release) {
            gh release view $tag --repo $Repo *> $null
            if ($LASTEXITCODE) {
                $extra = @('--latest=false')
                if ($Channel -eq 'beta') { $extra += '--prerelease' }
                gh release create $tag $zip --repo $Repo --title "$id $version" --notes "Foxglove plugin $id $version ($Channel)." @extra
                if ($LASTEXITCODE) { throw "creating $tag failed" }
            } else {
                Write-Host "  $tag exists; releases are immutable, so it's only checked"
            }
            $check = Join-Path $target "plugins\$asset.check"
            Invoke-WebRequest $url -OutFile $check -UseBasicParsing | Out-Null
            $got = (Get-FileHash $check -Algorithm SHA256).Hash.ToLower()
            Remove-Item $check
            if ($got -ne $hash) { throw "$url serves different bytes ($got): the catalog must not list it" }
        }
        Catalog add $catalog --dir (Join-Path $target "plugins\$id") --zip $zip --url $url --channel $Channel
    }

    if ($Crate.Count -or $Resign) {
        Catalog bump $catalog --days $Days
        Remove-Item "$catalog.minisig" -ErrorAction SilentlyContinue
        Write-Host ''
        Write-Host 'Next (maintainer, with the catalog key; it never passes through an agent):'
        Write-Host "  minisign -S -s <path to catalog.key> -m $catalog"
        Write-Host '  then: powershell -File scripts\publish-plugins.ps1 -Pages'
        if (-not $Release -and $Crate.Count) { Write-Host '  (dry run: no releases were created; run again with -Release first)' }
    }
} finally {
    Pop-Location
}
