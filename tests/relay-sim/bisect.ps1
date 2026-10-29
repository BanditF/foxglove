# relay-sim across firmware commits: each commit's widgets directory, this harness.
#   powershell -File tests/relay-sim/bisect.ps1 -Commits 13ff04b,88e3a59 [-Scenarios wake,late]
param(
    [string]$Commits = "13ff04b,88e3a59,1ebdf9d,77fd052,5568e5a,HEAD", # comma separated
    [string]$Scenarios = "", # comma separated; default: all of config.ps1
    [int]$Log = 1
)
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$root = Resolve-Path (Join-Path $here "..\..")
. (Join-Path $here "config.ps1")
if (-not $Scenarios) { $Scenarios = ($RelaySimScenarios + $RelaySimInfo) -join "," }
$rows = @()
foreach ($c in ($Commits -split ",")) {
    $sha = (git -C $root rev-parse --short $c).Trim()
    $dir = Join-Path $root "target\relay-sim\fw-$sha"
    $w = Join-Path $dir "widgets"
    if (Test-Path $w) { Remove-Item -Recurse -Force $w }
    New-Item -ItemType Directory -Force $w | Out-Null
    $files = git -C $root ls-tree --name-only "${sha}:boards/shields/nice_view_custom/widgets"
    foreach ($f in $files) {
        $text = git -C $root show "${sha}:boards/shields/nice_view_custom/widgets/$f"
        $text | Set-Content -Encoding utf8 (Join-Path $w $f)
    }
    $runLog = Join-Path $dir "run.log"
    $ErrorActionPreference = "Continue"
    powershell -ExecutionPolicy Bypass -File (Join-Path $here "build.ps1") -Widgets $w -Out $dir -Scenarios $Scenarios -Log $Log *> $runLog
    $ErrorActionPreference = "Stop"
    $results = Select-String -Path $runLog -Pattern "^RESULT " | ForEach-Object { $_.Line }
    Write-Host "== $c ($sha)"
    $results | ForEach-Object { Write-Host $_ }
    $rows += [pscustomobject]@{ Commit = $c; Sha = $sha; Results = ($results -join " | ") }
}
