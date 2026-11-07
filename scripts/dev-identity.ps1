# Foxglove's dev package identity (#179): a self-signed dev certificate, trusted on this PC,
# signs the sparse identity package, which is then registered with a Foxglove build's folder as
# its external location. Then Windows lists Foxglove under Settings > Personalization > Dynamic
# Lighting > Background light control.
#
#   powershell -File scripts\dev-identity.ps1                     # plan (the default): each step and whether it is done; changes nothing
#   powershell -File scripts\dev-identity.ps1 apply               # does the missing steps; asks first unless -Yes
#   powershell -File scripts\dev-identity.ps1 apply -MachineTrust # also trusts the cert machine-wide (needs an elevated PowerShell)
#   powershell -File scripts\dev-identity.ps1 undo                # removes every dev identity piece it finds
#
# Run apply and undo only on the user's own go, by the user: they write the certificate stores
# and register a package. Agents run plan, or the tests with fake operations, never apply/undo.
#
# Steps (apply looks at each one first and does only the missing ones, so a rerun after a failure
# or after the same steps done by hand resumes and is safe; when nothing is missing it is a no-op):
#   1. cert     a self-signed code-signing cert with subject -Publisher in Cert:\CurrentUser\My, key
#               NOT exportable, 1 year. An existing one (same subject, self-signed, with its key,
#               code signing, not expired) is reused, never duplicated.
#   2. trust    its public part (no key) in Cert:\CurrentUser\TrustedPeople: this user, no admin.
#   3. machine  (-MachineTrust) its public part in Cert:\LocalMachine\TrustedPeople: all users,
#               needs admin. Some Windows builds refuse a package trusted for the user only
#               (Add-AppxPackage 0x800B0109, root not trusted); then this is needed.
#   4. sign     scripts\pack-identity.ps1 -Publisher -Thumbprint -Stage: packs the package with
#               that publisher, signs it, and puts it and Assets\ next to foxglove.exe. Done when
#               the staged msix is signed by that cert and Assets\ is there.
#   5. register Add-AppxPackage -Path <msix> -ExternalLocation <exe folder> (per user). Done when
#               Get-AppxPackage BanditF.Foxglove* lists one with that publisher, however it got there.
#   6. verify   Get-AppxPackage BanditF.Foxglove*: registered, the publisher (and the folder) match.
# Undo, newest first: every BanditF.Foxglove* package (also one registered by hand), the staged
# msix and Assets\, the TrustedPeople copies (this user's and the machine's; the machine's needs
# admin), the My cert and its key, then the state file. It finds them in the stores, so it is
# complete with or without the state file.
#
# Admin: only the machine store needs it. When a run needs it and the PowerShell isn't elevated,
# the script prints the exact command for an elevated one and stops before changing anything.
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
    [switch]$MachineTrust,
    [switch]$Yes,
    # Tests only: a script that replaces the operations (it defines $Ops entries), so plan,
    # apply and undo run without touching a certificate store or a package.
    [string]$OpsScript
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$DefaultExeDir = Join-Path $root 'app\target\release'
$DefaultPublisher = 'CN=Foxglove Dev, O=BanditF'
if (-not $ExeDir) { $ExeDir = $DefaultExeDir }
$DefaultStateFile = Join-Path $env:LOCALAPPDATA 'Foxglove\dev-identity.json'
if (-not $StateFile) { $StateFile = $DefaultStateFile }
$Name = 'BanditF.Foxglove'
$Friendly = 'Foxglove dev identity'
$Msix = Join-Path $ExeDir 'foxglove-identity.msix'
$Assets = Join-Path $ExeDir 'Assets'
$CodeSigning = '1.3.6.1.5.5.7.3.3'
$UserTrust = 'CurrentUser\TrustedPeople'
$MachineStore = 'LocalMachine\TrustedPeople'
$Stores = @('CurrentUser\My', $UserTrust, $MachineStore)

# The operations: the only code that reads or changes the PC. Probes (IsElevated, FindCert,
# InStore, SignedBy, FindPackages, FindCerts) only read; the rest change something.
$Ops = @{
    IsElevated   = {
        ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
            [Security.Principal.WindowsBuiltInRole]::Administrator)
    }
    # A usable dev cert in this user's My store: the hinted thumbprint first, else the newest.
    FindCert     = {
        param($hint)
        $now = Get-Date
        $c = @(Get-ChildItem 'Cert:\CurrentUser\My' | Where-Object {
                $_.Subject -eq $Publisher -and $_.Issuer -eq $Publisher -and $_.HasPrivateKey -and
                $_.NotAfter -gt $now -and (@($_.EnhancedKeyUsageList | ForEach-Object ObjectId) -contains $CodeSigning)
            } | Sort-Object @{ e = { $_.Thumbprint -eq $hint }; Descending = $true }, @{ e = 'NotAfter'; Descending = $true })
        if ($c.Count) { $c[0].Thumbprint }
    }
    InStore      = { param($t, $store) [bool]$t -and (Test-Path "Cert:\$store\$t") }
    SignedBy     = {
        if (-not (Test-Path $Msix) -or -not (Test-Path $Assets)) { return $null }
        $sig = Get-AuthenticodeSignature -FilePath $Msix
        if ($sig.SignerCertificate) { $sig.SignerCertificate.Thumbprint }
    }
    FindPackages = {
        @(Get-AppxPackage -Name "$Name*" | ForEach-Object {
                [pscustomobject]@{ PackageFullName = $_.PackageFullName; Publisher = $_.Publisher; InstallLocation = $_.InstallLocation }
            })
    }
    # Every dev cert this script could have made or trusted, in all three stores, by subject
    # (a TrustedPeople copy has no friendly name: that lives in the store, not in the cert).
    FindCerts    = {
        @(foreach ($store in $Stores) {
                Get-ChildItem "Cert:\$store" | Where-Object { $_.Subject -eq $Publisher -and $_.Issuer -eq $Publisher } |
                    ForEach-Object Thumbprint
            }) | Sort-Object -Unique
    }
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
        $c = New-SelfSignedCertificate -Type Custom -Subject $Publisher -FriendlyName $Friendly `
            -KeyUsage DigitalSignature -KeyExportPolicy NonExportable `
            -CertStoreLocation 'Cert:\CurrentUser\My' -NotAfter (Get-Date).AddYears(1) `
            -TextExtension @("2.5.29.37={text}$CodeSigning", '2.5.29.19={text}')
        $c.Thumbprint
    }
    TrustCert    = {
        param($t, $store)
        $cer = Join-Path ([IO.Path]::GetTempPath()) "foxglove-dev-$t.cer"
        try {
            Export-Certificate -Cert "Cert:\CurrentUser\My\$t" -FilePath $cer | Out-Null
            Import-Certificate -FilePath $cer -CertStoreLocation "Cert:\$store" | Out-Null
        } finally { Remove-Item $cer -ErrorAction SilentlyContinue }
    }
    SignPackage  = {
        param($t)
        & (Join-Path $PSScriptRoot 'pack-identity.ps1') -Publisher $Publisher -Thumbprint $t -NoTimestamp -Stage $ExeDir
    }
    Register     = { Add-AppxPackage -Path $Msix -ExternalLocation $ExeDir }
    Unregister   = { param($full) Remove-AppxPackage -Package $full }
    Unstage      = {
        Remove-Item $Msix -ErrorAction SilentlyContinue
        Remove-Item $Assets -Recurse -ErrorAction SilentlyContinue
    }
    Untrust      = { param($t, $store) Remove-Item "Cert:\$store\$t" }
    RemoveCert   = { param($t) Remove-Item "Cert:\CurrentUser\My\$t" -DeleteKey }
    Confirm      = { (Read-Host 'Type APPLY to change the certificate stores and packages') -ceq 'APPLY' }
}
if ($OpsScript) { . $OpsScript }

function Read-State {
    if (Test-Path $StateFile) {
        $j = Get-Content $StateFile -Raw | ConvertFrom-Json
        @{ thumbprint = $j.thumbprint; publisher = $j.publisher; done = @($j.done | Where-Object { $_ }) }
    } else { @{ thumbprint = $null; publisher = $null; done = @() } }
}
# The state file is a record, not the truth: apply and undo look at the stores and packages.
# Apply writes what it found done (also steps done by hand, e.g. a manual Add-AppxPackage), so
# an older record (done = cert, trust, sign) is brought up to date by a rerun.
function Write-State($t, $done) {
    $old = Read-State
    if ((Test-Path $StateFile) -and $old.thumbprint -eq $t -and $old.publisher -eq $Publisher -and
        (@($old.done) -join ',') -eq (@($done) -join ',')) { return }
    New-Item -ItemType Directory -Force (Split-Path -Parent $StateFile) | Out-Null
    [pscustomobject]@{ thumbprint = $t; publisher = $Publisher; done = @($done) } | ConvertTo-Json | Set-Content $StateFile -Encoding utf8
}
# Every step found done, the machine trust too when it was added by hand without -MachineTrust.
function Get-DoneIds($st) { @('cert', 'trust', 'machine', 'sign', 'register' | Where-Object { $st.Done[$_] }) }

# The exact command for an elevated PowerShell: this script, this mode, the same options.
function Get-ElevatedCommand($m) {
    $cmd = "powershell -ExecutionPolicy Bypass -File `"$PSCommandPath`" $m"
    if ($m -eq 'apply') { $cmd += ' -MachineTrust' }
    if ($ExeDir -ne $DefaultExeDir) { $cmd += " -ExeDir `"$ExeDir`"" }
    if ($Publisher -ne $DefaultPublisher) { $cmd += " -Publisher '$Publisher'" }
    if ($StateFile -ne $DefaultStateFile) { $cmd += " -StateFile `"$StateFile`"" }
    $cmd
}
function Show-NeedsAdmin($m, $why) {
    Write-Host ''
    Write-Host "Needs administrator: $why. Nothing was changed."
    Write-Host 'Open Start, type PowerShell, right-click Windows PowerShell, click Run as administrator'
    Write-Host '(from your own account: the dev cert is in your user store), then run:'
    Write-Host "  $(Get-ElevatedCommand $m)"
}

# What is there now. Probes only: nothing changes.
function Get-Status {
    $s = Read-State
    $t = & $Ops.FindCert $s.thumbprint
    $pkgs = @(& $Ops.FindPackages)
    $mine = @($pkgs | Where-Object { $_.Publisher -eq $Publisher })
    $signer = & $Ops.SignedBy
    [pscustomobject]@{
        Thumbprint = $t
        Packages   = $pkgs
        Mine       = $mine
        Others     = @($pkgs | Where-Object { $_.Publisher -ne $Publisher })
        Done       = @{
            cert     = [bool]$t
            trust    = [bool]$t -and (& $Ops.InStore $t $UserTrust)
            machine  = [bool]$t -and (& $Ops.InStore $t $MachineStore)
            sign     = [bool]$t -and $signer -eq $t
            register = $mine.Count -gt 0
        }
    }
}

function Get-Steps {
    $steps = @(
        @{ id = 'cert'; does = "a self-signed code-signing certificate '$Publisher' in Cert:\CurrentUser\My (key not exportable, 1 year; an existing one is reused)"; undo = 'remove it and its key from Cert:\CurrentUser\My' }
        @{ id = 'trust'; does = "its public part in Cert:\$UserTrust (this user only, no admin)"; undo = "remove it from Cert:\$UserTrust" }
    )
    if ($MachineTrust) {
        $steps += @{ id = 'machine'; does = "its public part in Cert:\$MachineStore (all users; needs an elevated PowerShell)"; undo = "remove it from Cert:\$MachineStore (needs an elevated PowerShell)" }
    }
    $steps + @(
        @{ id = 'sign'; does = "foxglove-identity.msix packed and signed with it, it and Assets\ in $ExeDir"; undo = "delete $Msix and $Assets" }
        @{ id = 'register'; does = "Add-AppxPackage -Path $Msix -ExternalLocation $ExeDir (this user)"; undo = "Remove-AppxPackage every $Name* package (also one registered by hand)" }
    )
}

function Show-Plan($st) {
    Write-Host "Foxglove dev identity: the plan (nothing changes here). Exe folder: $ExeDir"
    if ($MachineTrust) {
        Write-Host "  Trust store: Cert:\$UserTrust and Cert:\$MachineStore (-MachineTrust; the machine store needs administrator)"
    } else {
        Write-Host "  Trust store: Cert:\$UserTrust only (if registration fails with 0x800B0109, add -MachineTrust for Cert:\$MachineStore)"
    }
    Write-Host "  0. check    foxglove.exe there was built with FOXGLOVE_SPARSE_IDENTITY=1 and publisher '$Publisher'"
    $i = 1
    foreach ($step in (Get-Steps)) {
        $mark = if ($st.Done[$step.id]) { '[done] ' } else { '[to do]' }
        Write-Host ("  {0}. {1,-8} {2} {3}" -f $i, $step.id, $mark, $step.does)
        Write-Host ("     undo:    {0}" -f $step.undo)
        $i++
    }
    Write-Host ("  {0}. verify   Get-AppxPackage {1}*: registered with publisher '{2}'" -f $i, $Name, $Publisher)
    if ($st.Thumbprint) { Write-Host "  Certificate: $($st.Thumbprint)" }
    foreach ($p in $st.Packages) { Write-Host "  Registered: $($p.PackageFullName) ($($p.Publisher))" }
    Write-Host "  Apply does only the [to do] steps; a rerun is safe. State file: $StateFile"
    Write-Host '  Then: Settings > Personalization > Dynamic Lighting > Background light control: drag Foxglove to the top.'
    Write-Host '  Apply: scripts\dev-identity.ps1 apply [-MachineTrust]    Undo: scripts\dev-identity.ps1 undo'
}

function Invoke-Verify {
    $p = @(& $Ops.FindPackages | Where-Object { $_.Publisher -eq $Publisher })
    if (-not $p.Count) { throw "$Name isn't registered for this user with publisher '$Publisher'" }
    Write-Host "  registered: $($p[0].PackageFullName)"
    if ($p[0].InstallLocation -and $p[0].InstallLocation.TrimEnd('\') -ne $ExeDir.TrimEnd('\')) {
        Write-Host "  note: it points at $($p[0].InstallLocation), not $ExeDir; undo, then apply, to move it"
    }
    Write-Host '  next: quit Foxglove and start it again from that folder; it runs with the identity'
}

switch ($Mode) {
    'plan' { Show-Plan (Get-Status) }
    'apply' {
        $st = Get-Status
        Show-Plan $st
        $todo = @(Get-Steps | Where-Object { -not $st.Done[$_.id] } | ForEach-Object { $_.id })
        if (-not $todo.Count) {
            Write-Host 'Nothing to do: every step is done already.'
            Write-State $st.Thumbprint (Get-DoneIds $st)
            Invoke-Verify
            Write-Host 'Applied.'
            exit 0
        }
        if ($todo -contains 'machine' -and -not (& $Ops.IsElevated)) {
            Show-NeedsAdmin 'apply' "the certificate goes into Cert:\$MachineStore"
            exit 1
        }
        if ($todo -contains 'register' -and $st.Others.Count) {
            Write-Host "Another $Name package is registered ($(($st.Others | ForEach-Object { "$($_.PackageFullName), $($_.Publisher)" }) -join '; ')). Nothing was changed."
            Write-Host 'Remove it first: scripts\dev-identity.ps1 undo'
            exit 1
        }
        if (-not $Yes -and -not (& $Ops.Confirm)) { Write-Host 'Not applied.'; exit 1 }
        if ($todo -contains 'sign' -or $todo -contains 'register') { & $Ops.CheckExe }
        $t = $st.Thumbprint
        foreach ($id in $todo) {
            $step = Get-Steps | Where-Object { $_.id -eq $id }
            Write-Host "== $($id): $($step.does)"
            try {
                switch ($id) {
                    'cert' { $t = & $Ops.CreateCert; Write-State $t @() }
                    'trust' { & $Ops.TrustCert $t $UserTrust }
                    'machine' { & $Ops.TrustCert $t $MachineStore }
                    'sign' { & $Ops.SignPackage $t }
                    'register' { & $Ops.Register }
                }
            } catch {
                $err = $_
                Write-State $t (Get-DoneIds (Get-Status))
                Write-Host "Failed at $($id): $err"
                if ("$err" -match '0x800B0109' -or $err.Exception.HResult -eq -2146762487) {
                    Write-Host ''
                    if ($MachineTrust) {
                        Write-Host "Windows doesn't trust the certificate's root (0x800B0109) even with machine trust: check that $t is in Cert:\$MachineStore."
                    } else {
                        Write-Host "Windows doesn't trust the certificate's root (0x800B0109): this Windows build needs the certificate trusted machine-wide, in Cert:\$MachineStore."
                        Write-Host 'Run apply again with -MachineTrust from an elevated PowerShell (it resumes; the steps done stay):'
                        Write-Host "  $(Get-ElevatedCommand 'apply')"
                    }
                } else {
                    Write-Host 'Fix the cause and run apply again (it resumes), or remove everything: scripts\dev-identity.ps1 undo'
                }
                exit 1
            }
        }
        Write-State $t (Get-DoneIds (Get-Status))
        Write-Host '== verify'
        Invoke-Verify
        Write-Host 'Applied.'
    }
    'undo' {
        $s = Read-State
        if ($s.publisher) { $script:Publisher = $s.publisher }
        $certs = @(@(& $Ops.FindCerts) + @($s.thumbprint) | Where-Object { $_ } | Sort-Object -Unique)
        $where = @{}
        foreach ($t in $certs) { $where[$t] = @($Stores | Where-Object { & $Ops.InStore $t $_ }) }
        $pkgs = @(& $Ops.FindPackages)
        $machine = @($certs | Where-Object { $where[$_] -contains $MachineStore })
        if ($machine.Count -and -not (& $Ops.IsElevated)) {
            Show-NeedsAdmin 'undo' "a dev certificate ($($machine -join ', ')) is in Cert:\$MachineStore"
            exit 1
        }
        foreach ($p in $pkgs) {
            Write-Host "== undo register: Remove-AppxPackage $($p.PackageFullName) ($($p.Publisher))"
            & $Ops.Unregister $p.PackageFullName
        }
        Write-Host "== undo sign: delete $Msix and $Assets"
        & $Ops.Unstage
        foreach ($t in $certs) {
            foreach ($store in @($MachineStore, $UserTrust)) {
                if ($where[$t] -contains $store) { Write-Host "== undo trust: $t from Cert:\$store"; & $Ops.Untrust $t $store }
            }
            if ($where[$t] -contains 'CurrentUser\My') { Write-Host "== undo cert: $t and its key from Cert:\CurrentUser\My"; & $Ops.RemoveCert $t }
        }
        Remove-Item $StateFile -ErrorAction SilentlyContinue
        Write-Host 'Undone. Foxglove runs without identity from its next start.'
    }
}
