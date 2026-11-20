# MIT. Foxglove dev copy-in (rebuild plan R7 / S9): put a fresh foxglove.exe into the ENROLLED dev folder while the
# old one may still be running. Windows will not overwrite a running exe but will rename it, so: rename the current
# exe aside to foxglove.exe.old (removing older .old files first), then copy the new exe in.
#
#   powershell -File scripts\ops\dev-copy-in.ps1 -DevFolder <enrolled folder> -NewExe <built foxglove.exe>
#       DRY RUN (default): prints each step it would do and any refusal; changes nothing.
#   ... -Apply   does it. Running it on the user's hub is an On-call step; tests and agents never pass -Apply.
#   ... -Target <path>   the exe to replace; default <DevFolder>\foxglove.exe. Must be inside -DevFolder.
#
# It only renames, deletes and copies FILES. It never stops, signals or inspects a process. It refuses (exit 1):
# a path outside the enrolled folder (checked after full-path normalisation, so .. cannot escape), UNC and device
# paths, a drive that is not a local fixed disk (removable, network, optical), and any link (symlink or junction)
# on the way: the folder, each parent up to the target, the target, the .old files, or the new exe. Links are
# never followed.
param(
    [Parameter(Mandatory)][string]$DevFolder,   # the folder enrolled in Hideout's dev folders
    [Parameter(Mandatory)][string]$NewExe,      # the freshly built exe
    [string]$Target,
    [switch]$Apply
)
$ErrorActionPreference = 'Stop'
$problems = New-Object System.Collections.Generic.List[string]
function Refuse([string]$m) { $script:problems.Add($m) }

function Get-FullPathChecked([string]$p, [string]$what) {
    if ([string]::IsNullOrWhiteSpace($p)) { Refuse "$what is empty"; return $null }
    if ($p.StartsWith('\\') -or $p.StartsWith('//')) { Refuse "$what is a UNC or device path: $p"; return $null }
    if ($p -notmatch '^[A-Za-z]:[\\/]') { Refuse "$what is not an absolute local path: $p"; return $null }
    [IO.Path]::GetFullPath($p).TrimEnd('\')
}
function Test-Link([string]$p) {
    $i = Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue
    [bool]($i -and ($i.Attributes -band [IO.FileAttributes]::ReparsePoint))
}
function Test-FixedDrive([string]$full) {
    try { [IO.DriveInfo]::new($full.Substring(0, 1)).DriveType -eq [IO.DriveType]::Fixed } catch { $false }
}

$dev = Get-FullPathChecked $DevFolder '-DevFolder'
$new = Get-FullPathChecked $NewExe '-NewExe'
$tgt = $null
if ($dev) {
    $want = if ($Target) { $Target } else { Join-Path $dev 'foxglove.exe' }
    $tgt = Get-FullPathChecked $want '-Target'
}

if ($dev -and -not (Test-FixedDrive $dev)) { Refuse "-DevFolder is not on a local fixed drive: $dev" }
if ($tgt -and -not (Test-FixedDrive $tgt)) { Refuse "-Target is not on a local fixed drive: $tgt" }
if ($new -and -not (Test-FixedDrive $new)) { Refuse "-NewExe is not on a local fixed drive: $new" }
if ($dev -and $tgt -and -not $tgt.StartsWith($dev + '\', [StringComparison]::OrdinalIgnoreCase)) {
    Refuse "-Target is outside the enrolled dev folder: $tgt (folder $dev)"
}
if ($dev -and -not (Test-Path -LiteralPath $dev -PathType Container)) { Refuse "-DevFolder does not exist: $dev" }
if ($new -and -not (Test-Path -LiteralPath $new -PathType Leaf)) { Refuse "-NewExe not found: $new" }

$old = $null
$leftovers = @()
if (-not $problems.Count) {
    $old = $tgt + '.old'
    # links: the folder, every part from it down to the target, the .old, the new exe
    $chain = New-Object System.Collections.Generic.List[string]
    $chain.Add($dev)
    $cur = $dev
    foreach ($part in $tgt.Substring($dev.Length).TrimStart('\').Split('\')) { $cur = Join-Path $cur $part; $chain.Add($cur) }
    $chain.Add($old); $chain.Add($new)
    foreach ($c in $chain) { if (Test-Link $c) { Refuse "refusing a link (never followed): $c" } }
    $tdir = Split-Path -Parent $tgt
    if (Test-Path -LiteralPath $tdir -PathType Container) {
        $leftovers = @(Get-ChildItem -LiteralPath $tdir -Force -Filter ((Split-Path -Leaf $tgt) + '.old*') -ErrorAction SilentlyContinue)
        foreach ($l in $leftovers) { if (Test-Link $l.FullName) { Refuse "refusing a link (never followed): $($l.FullName)" } }
    }
}

if ($Apply) { Write-Host 'APPLY' } else { Write-Host 'DRY RUN (no -Apply: nothing is changed)' }
Write-Host "dev folder : $dev"
Write-Host "new exe    : $new"
Write-Host "target     : $tgt"
if ($problems.Count) {
    foreach ($p in $problems) { Write-Host "REFUSED    : $p" }
    exit 1
}
$hasTarget = Test-Path -LiteralPath $tgt -PathType Leaf
foreach ($l in $leftovers) { Write-Host "plan       : remove older  $($l.FullName)" }
if ($hasTarget) { Write-Host "plan       : rename aside  $tgt -> $old" } else { Write-Host 'plan       : no current exe at the target, nothing to rename aside' }
Write-Host "plan       : copy in       $new -> $tgt"
Write-Host 'no process is stopped or touched; a running copy keeps running from the .old file until it exits.'
if (-not $Apply) { Write-Host 'dry run done. Re-run with -Apply to do the steps above (On-call only).'; exit 0 }

foreach ($l in $leftovers) { Remove-Item -LiteralPath $l.FullName -Force }
if ($hasTarget) { Move-Item -LiteralPath $tgt -Destination $old }
try { Copy-Item -LiteralPath $new -Destination $tgt }
catch {
    if ($hasTarget -and -not (Test-Path -LiteralPath $tgt)) { Move-Item -LiteralPath $old -Destination $tgt }
    throw
}
Write-Host 'done.'
