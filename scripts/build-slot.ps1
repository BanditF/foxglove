# Build caps (#347): at most 2 cargo builds at once across every session, each on about half
# the cores. A slot is one of two named mutexes; Windows hands an abandoned one to the next
# waiter if a build is killed, so a crash never leaks a slot. Waiting blocks on the handles,
# with no polling.
#   powershell -File scripts\build-slot.ps1 cargo build --release     # one command in a slot
#   . "$PSScriptRoot\build-slot.ps1"; $slot = Enter-BuildSlot; try { ... } finally { Exit-BuildSlot $slot }
# CARGO_BUILD_JOBS already set (by .claude/settings.json or the caller) is kept.
# Builds run at BelowNormal priority so the desktop stays smooth during a gate: the slot's
# process drops to it, and cargo, rustc and the tests inherit it (Windows passes the priority
# class to child processes).
# No param block: every argument, dashes included, belongs to the command.
$Command = @($args)

# Lowers this process (and so everything it starts) to BelowNormal; returns the class it had.
function Set-BuildPriority {
    try {
        $me = Get-Process -Id $PID
        $was = $me.PriorityClass
        if ($was -ne 'Idle' -and $was -ne 'BelowNormal') { $me.PriorityClass = 'BelowNormal' }
        $was
    } catch { Write-Warning "build priority: $_"; $null }
}

function Restore-BuildPriority($was) {
    if ($was) { try { (Get-Process -Id $PID).PriorityClass = $was } catch { Write-Warning "build priority: $_" } }
}

function Enter-BuildSlot {
    $priority = Set-BuildPriority    if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = [math]::Max(1, [int][math]::Floor([Environment]::ProcessorCount / 2)) }
    # A nested call (check.ps1 running build-slot.ps1) already holds a slot.
    if ($env:FOXGLOVE_BUILD_SLOT) { return $null }
    $slots = [Threading.Mutex[]](0..1 | ForEach-Object { [Threading.Mutex]::new($false, "Local\Foxglove.CargoBuild.$_") })
    $index = Wait-Slot $slots 0
    if ($index -eq [Threading.WaitHandle]::WaitTimeout) {
        Write-Host 'Waiting for a cargo build slot: 2 builds are already running in other sessions.' -ForegroundColor Yellow
        $index = Wait-Slot $slots -1
    }
    $env:FOXGLOVE_BUILD_SLOT = "$index"
    @{ Slots = $slots; Index = $index; Priority = $priority }
}

function Wait-Slot($slots, [int]$timeout) {
    try { [Threading.WaitHandle]::WaitAny($slots, $timeout) }
    catch {
        # The previous holder died without releasing it: the slot is ours now.
        $e = $_.Exception
        while ($e -and $e -isnot [Threading.AbandonedMutexException]) { $e = $e.InnerException }
        if (-not $e) { throw }
        $e.MutexIndex
    }
}

function Exit-BuildSlot($slot) {
    if (-not $slot) { return }
    $slot.Slots[$slot.Index].ReleaseMutex()
    $slot.Slots | ForEach-Object { $_.Dispose() }
    Restore-BuildPriority $slot.Priority
    Remove-Item Env:FOXGLOVE_BUILD_SLOT -ErrorAction SilentlyContinue
}

if ($Command) {
    $slot = Enter-BuildSlot
    try {
        # An array even for one argument: splatting a lone string passes its characters.
        $rest = @($Command | Select-Object -Skip 1)
        # A missing command is a failure, not a pass. Checked up front so the call itself runs
        # with the caller's error setting: under 'Stop', cargo's stderr lines would throw.
        $null = Get-Command $Command[0] -ErrorAction Stop
        $global:LASTEXITCODE = 0
        & $Command[0] @rest
        $code = $LASTEXITCODE
    } catch {
        Write-Error $_ -ErrorAction Continue
        $code = 1
    } finally { Exit-BuildSlot $slot }
    exit $code
}
