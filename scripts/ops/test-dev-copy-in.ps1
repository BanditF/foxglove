# MIT. Window-free tests for scripts/ops/dev-copy-in.ps1: temp folders and fake exes (text files) only. The real
# -Apply is never run here; the dry run and the refusals are. Exit 0 if all pass.
#   powershell -File scripts\ops\test-dev-copy-in.ps1
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$fails = 0
function Check([string]$name, $cond) { if ($cond) { Write-Host "ok   $name" } else { Write-Host "FAIL $name"; $script:fails++ } }
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("dev-copy-in-test-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $tmp | Out-Null
$ps = (Get-Command powershell).Source
$s = Join-Path $here 'dev-copy-in.ps1'
function Run { & $ps -NoProfile -File $s @args 2>&1 | Out-String }
try {
    $dev = Join-Path $tmp 'dev'; New-Item -ItemType Directory $dev | Out-Null
    $cur = Join-Path $dev 'foxglove.exe'; Set-Content $cur 'current'
    Set-Content "$cur.old" 'older'
    $new = Join-Path $tmp 'new.exe'; Set-Content $new 'new'

    $o = Run -DevFolder $dev -NewExe $new
    Check 'dry run: exit 0, says DRY RUN' ($LASTEXITCODE -eq 0 -and $o -match 'DRY RUN')
    $iRm = $o.IndexOf('remove older'); $iRn = $o.IndexOf('rename aside'); $iCp = $o.IndexOf('copy in')
    Check 'dry run: plans remove older, rename aside, copy in, in that order' ($iRm -ge 0 -and $iRm -lt $iRn -and $iRn -lt $iCp -and $o -match 'foxglove\.exe -> [^\r\n]*foxglove\.exe\.old')
    Check 'dry run: says it touches no process' ($o -match 'no process is stopped')
    Check 'dry run: files unchanged' ((Get-Content $cur) -eq 'current' -and (Get-Content "$cur.old") -eq 'older' -and @(Get-ChildItem $dev).Count -eq 2)

    $o = Run -DevFolder $dev -NewExe $new -Target (Join-Path $dev 'sub\..\foxglove.exe')
    Check 'target with .. that stays inside is fine' ($LASTEXITCODE -eq 0)

    $out = Join-Path $tmp 'elsewhere.exe'; Set-Content $out 'x'
    $o = Run -DevFolder $dev -NewExe $new -Target $out
    Check 'target outside the enrolled folder refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*outside')
    $o = Run -DevFolder $dev -NewExe $new -Target (Join-Path $dev '..\elsewhere.exe')
    Check '.. escape refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*outside')
    $o = Run -DevFolder $dev -NewExe $new -Target ($dev + '-sibling\foxglove.exe')
    Check 'sibling folder with the same prefix refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*outside')
    $o = Run -DevFolder $dev -NewExe $new -Target $dev
    Check 'the folder itself is not a target' ($LASTEXITCODE -eq 1)

    $o = Run -DevFolder '\\server\share\dev' -NewExe $new
    Check 'UNC dev folder refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*UNC')
    $o = Run -DevFolder $dev -NewExe '\\?\C:\x\new.exe'
    Check 'device-path new exe refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*UNC')
    $o = Run -DevFolder 'relative\dev' -NewExe $new
    Check 'relative folder refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*absolute')

    $unused = [char[]](68..90) | Where-Object { -not (Test-Path ([string]$_ + ':\')) } | Select-Object -First 1
    if ($unused) {
        $o = Run -DevFolder ([string]$unused + ':\dev') -NewExe $new
        Check 'a drive that is not a local fixed disk refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*fixed')
    }

    $o = Run -DevFolder $dev -NewExe (Join-Path $tmp 'nope.exe')
    Check 'missing new exe refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*not found')

    # links: a junction as the dev folder, as a parent of the target, as the new exe
    $real = Join-Path $tmp 'real'; New-Item -ItemType Directory $real | Out-Null
    $jun = Join-Path $tmp 'jun'; New-Item -ItemType Junction -Path $jun -Target $real | Out-Null
    $o = Run -DevFolder $jun -NewExe $new
    Check 'dev folder that is a junction refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*link')
    $inner = Join-Path $dev 'inner'; New-Item -ItemType Junction -Path $inner -Target $real | Out-Null
    $o = Run -DevFolder $dev -NewExe $new -Target (Join-Path $inner 'foxglove.exe')
    Check 'a junction parent inside the folder refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*link')
    $ljn = Join-Path $tmp 'newlink'; New-Item -ItemType Junction -Path $ljn -Target $real | Out-Null
    $o = Run -DevFolder $dev -NewExe $ljn
    Check 'a link as the new exe refused' ($LASTEXITCODE -eq 1 -and $o -match 'REFUSED.*link')
    Check 'nothing written through any link' (@(Get-ChildItem $real -Force).Count -eq 0)
    (Get-Item $inner).Delete(); (Get-Item $jun).Delete(); (Get-Item $ljn).Delete()

    Check 'no refusal changed the dev folder' ((Get-Content $cur) -eq 'current' -and (Get-Content "$cur.old") -eq 'older')
    Check 'script never stops or inspects a process' (-not ((Get-Content -Raw $s) -match 'Stop-Process|Get-Process|taskkill|Get-CimInstance'))
}
finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
if ($fails) { Write-Host "$fails FAILED"; exit 1 }
Write-Host 'all passed'
