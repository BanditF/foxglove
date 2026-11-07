# Foxglove's dev package identity (#179): a self-signed dev certificate, trusted for this user
# only, signs the sparse identity package, which is then registered with a Foxglove build's
# folder as its external location. Then Windows lists Foxglove under Settings > Personalization >
# Dynamic Lighting > Background light control.
#
#   powershell -File scripts\dev-identity.ps1              # plan (the default): prints every step, changes nothing
#   powershell -File scripts\dev-identity.ps1 apply        # does the steps; asks first unless -Yes
#   powershell -File scripts\dev-identity.ps1 undo         # reverses every step it did, newest first
#
# Run apply only on the user's own go, by the user: it writes the user's certificate stores and
# registers a package. Agents run plan, or the tests with fake operations, never apply.
#
# What apply does, in order (each one recorded in the state file as it completes, so undo
# reverses exactly what was done, even after a failure part way):
#   1. cert     New-SelfSignedCertificate in Cert:\CurrentUser\My, subject -Publisher, code
#               signing only, key NOT exportable (it never leaves this user's store), 1 year.
#   2. trust    its public part (no key) into Cert:\CurrentUser\TrustedPeople: this user only,
#               no admin. Windows checks the package's signature against it.
#   3. sign     scripts\pack-identity.ps1 -Publisher -Thumbprint -Stage: packs the package with
#               that publisher, signs it, and puts it and Assets\ next to foxglove.exe.
#   4. register Add-AppxPackage -Path <msix> -ExternalLocation <exe folder> (per user).
#   5. verify   Get-AppxPackage BanditF.Foxglove: registered, the publisher and the folder match.
# Undo: Remove-AppxPackage, the staged msix and Assets\, the TrustedPeople copy, the My cert,
# then the state file.
#
# The exe must have been built with the same publisher:
#   $env:FOXGLOVE_SPARSE_IDENTITY = '1'; $env:FOXGLOVE_IDENTITY_PUBLISHER = '<-Publisher>'
#   cargo build --release --manifest-path app/Cargo.toml
# and every later rebuild of that exe needs both again, or it runs without identity: on a PC
# with the identity registered, [identity] sparse = true and publisher = '<-Publisher>' in
# tools\local.toml give them to every build (docs/packaging.md, Dev builds).
param(
    [ValidateSet('plan', 'apply', 'undo')][string]$Mode = 'plan',
    [string]$ExeDir,
    [string]$Publisher = 'CN=Foxglove Dev, O=BanditF',
    [string]$StateFile,
    [switch]$Yes,
    # Tests only: a script that replaces the operations (it defines $Ops entries), so plan,
    # apply and undo run without touching a certificate store or a package.
    [string]$OpsScript
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $ExeDir) { $ExeDir = Join-Path $root 'app\target\release' }
if (-not $StateFile) { $StateFile = Join-Path $env:LOCALAPPDATA 'Foxglove\dev-identity.json' }
$Name = 'BanditF.Foxglove'
$Friendly = 'Foxglove dev identity'
$Msix = Join-Path $ExeDir 'foxglove-identity.msix'
$Assets = Join-Path $ExeDir 'Assets'

# The operations: the only code that changes anything. Each takes the state (a hashtable).
$Ops = @{
    CheckExe     = {
        $exe = Join-Path $ExeDir 'foxglove.exe'
        if (-not (Test-Path $exe)) { throw "no foxglove.exe in $ExeDir" }
        $text = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($exe))
        if (-not $text.Contains('urn:schemas-microsoft-com:msix.v1')) {
            throw "$exe has no <msix> element: build it with FOXGLOVE_SPARSE_IDENTITY=1"
        }
        if (-not $text.Contains("publisher=`"$Publisher`"")) {
            throw "$exe names another publisher: build it with FOXGLOVE_IDENTITY_PUBLISHER='$Publisher'"
        }
    }
    CreateCert   = {
        param($s)
        $c = New-SelfSignedCertificate -Type Custom -Subject $Publisher -FriendlyName $Friendly `
            -KeyUsage DigitalSignature -KeyExportPolicy NonExportable `
            -CertStoreLocation 'Cert:\CurrentUser\My' -NotAfter (Get-Date).AddYears(1) `
            -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
        $s.thumbprint = $c.Thumbprint
    }
    TrustCert    = {
        param($s)
        $cer = Join-Path ([IO.Path]::GetTempPath()) "foxglove-dev-$($s.thumbprint).cer"
        try {
            Export-Certificate -Cert "Cert:\CurrentUser\My\$($s.thumbprint)" -FilePath $cer | Out-Null
            Import-Certificate -FilePath $cer -CertStoreLocation 'Cert:\CurrentUser\TrustedPeople' | Out-Null
        } finally { Remove-Item $cer -ErrorAction SilentlyContinue }
    }
    SignPackage  = {
        param($s)
        & (Join-Path $PSScriptRoot 'pack-identity.ps1') -Publisher $Publisher -Thumbprint $s.thumbprint `
            -NoTimestamp -Stage $ExeDir
    }
    Register     = { Add-AppxPackage -Path $Msix -ExternalLocation $ExeDir }
    Verify       = {
        $p = Get-AppxPackage -Name $Name | Where-Object { $_.Publisher -eq $Publisher }
        if (-not $p) { throw "$Name isn't registered for this user" }
        Write-Host "  registered: $($p.PackageFullName), $($p.SignatureKind)"
        Write-Host '  next: quit Foxglove and start it again from that folder; it runs with the identity'
    }
    Unregister   = {
        Get-AppxPackage -Name $Name | Where-Object { $_.Publisher -eq $Publisher } |
            ForEach-Object { Remove-AppxPackage -Package $_.PackageFullName }
    }
    Unstage      = {
        Remove-Item $Msix -ErrorAction SilentlyContinue
        Remove-Item $Assets -Recurse -ErrorAction SilentlyContinue
    }
    Untrust      = { param($s) Remove-Item "Cert:\CurrentUser\TrustedPeople\$($s.thumbprint)" -ErrorAction SilentlyContinue }
    RemoveCert   = { param($s) Remove-Item "Cert:\CurrentUser\My\$($s.thumbprint)" -DeleteKey -ErrorAction SilentlyContinue }
    # Undo without a state file: the dev certificates this script makes, by name and subject.
    FindCerts    = {
        @(Get-ChildItem 'Cert:\CurrentUser\My', 'Cert:\CurrentUser\TrustedPeople' |
            Where-Object { $_.Subject -eq $Publisher -and $_.FriendlyName -eq $Friendly } |
            ForEach-Object Thumbprint | Sort-Object -Unique)
    }
    Confirm      = { (Read-Host "Type APPLY to change this user's certificate stores and packages") -ceq 'APPLY' }
}
if ($OpsScript) { . $OpsScript }

# Each step: its name, what it does in words, how undo reverses it, and its operation.
$Steps = @(
    @{ id = 'cert'; does = "create a self-signed code-signing certificate '$Publisher' in Cert:\CurrentUser\My (key not exportable, 1 year)"; undo = 'remove it from Cert:\CurrentUser\My'; op = 'CreateCert' }
    @{ id = 'trust'; does = 'copy its public part to Cert:\CurrentUser\TrustedPeople (this user only)'; undo = 'remove it from Cert:\CurrentUser\TrustedPeople'; op = 'TrustCert' }
    @{ id = 'sign'; does = "pack and sign foxglove-identity.msix with it, put it and Assets\ in $ExeDir"; undo = "delete $Msix and $Assets"; op = 'SignPackage' }
    @{ id = 'register'; does = "Add-AppxPackage -Path $Msix -ExternalLocation $ExeDir (this user)"; undo = "Remove-AppxPackage $Name"; op = 'Register' }
    @{ id = 'verify'; does = "Get-AppxPackage $($Name): registered with publisher '$Publisher'"; undo = '(nothing to undo)'; op = 'Verify' }
)
$UndoOps = @{ cert = 'RemoveCert'; trust = 'Untrust'; sign = 'Unstage'; register = 'Unregister' }

function Read-State {
    if (Test-Path $StateFile) {
        $j = Get-Content $StateFile -Raw | ConvertFrom-Json
        @{ thumbprint = $j.thumbprint; publisher = $j.publisher; done = @($j.done) }
    } else { @{ thumbprint = $null; publisher = $Publisher; done = @() } }
}
function Write-State($s) {
    New-Item -ItemType Directory -Force (Split-Path -Parent $StateFile) | Out-Null
    [pscustomobject]@{ thumbprint = $s.thumbprint; publisher = $s.publisher; done = @($s.done) } | ConvertTo-Json | Set-Content $StateFile -Encoding utf8
}

function Show-Plan {
    Write-Host "Foxglove dev identity: the plan (nothing changes). Exe folder: $ExeDir"
    Write-Host "  0. check  foxglove.exe there was built with FOXGLOVE_SPARSE_IDENTITY=1 and publisher '$Publisher'"
    $i = 1
    foreach ($st in $Steps) {
        Write-Host ("  {0}. {1,-8} {2}" -f $i, $st.id, $st.does)
        Write-Host ("     undo:    {0}" -f $st.undo)
        $i++
    }
    Write-Host "  State file: $StateFile (what was done, for undo)"
    Write-Host '  Then: Settings > Personalization > Dynamic Lighting > Background light control: drag Foxglove to the top.'
    Write-Host '  Apply: scripts\dev-identity.ps1 apply    Undo: scripts\dev-identity.ps1 undo'
}

switch ($Mode) {
    'plan' { Show-Plan }
    'apply' {
        Show-Plan
        if (-not $Yes -and -not (& $Ops.Confirm)) { Write-Host 'Not applied.'; exit 1 }
        $s = Read-State
        if ($s.done.Count) { throw "a dev identity is already applied ($($s.done -join ', ')): run undo first" }
        & $Ops.CheckExe
        foreach ($st in $Steps) {
            Write-Host "== $($st.id): $($st.does)"
            try { & $Ops[$st.op] $s }
            catch {
                Write-State $s
                Write-Host "Failed at $($st.id): $_"
                Write-Host 'Undo what was done: scripts\dev-identity.ps1 undo'
                exit 1
            }
            if ($UndoOps.ContainsKey($st.id)) { $s.done += $st.id }
            Write-State $s
        }
        Write-Host 'Applied.'
    }
    'undo' {
        $s = Read-State
        if (-not (Test-Path $StateFile)) {
            # No record: undo everything this script could have made.
            Write-Host 'No state file: removing any dev identity by name and subject.'
            & $Ops.Unregister
            & $Ops.Unstage
            foreach ($t in (& $Ops.FindCerts)) {
                & $Ops.Untrust @{ thumbprint = $t }
                & $Ops.RemoveCert @{ thumbprint = $t }
            }
        } else {
            # The publisher it was applied with, so the right package is removed.
            if ($s.publisher) { $script:Publisher = $s.publisher }
            $done = @($s.done)
            [array]::Reverse($done)
            foreach ($id in $done) {
                Write-Host "== undo $id"
                & $Ops[$UndoOps[$id]] $s
                $s.done = @($s.done | Where-Object { $_ -ne $id })
                Write-State $s
            }
            Remove-Item $StateFile
        }
        Write-Host 'Undone. Foxglove runs without identity from its next start.'
    }
}
