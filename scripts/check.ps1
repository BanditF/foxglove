# The gate for the desktop side: lint and tests for crates/ and app/. Run it before pushing:
#   powershell -File scripts\check.ps1
# GitHub's Rust and app workflows only run by hand (Actions tab) for a Linux check or an exe.
$ErrorActionPreference = 'Stop'

# Seconds per stage, printed at the end (also when a stage fails) so the slow ones show in every gate log.
$script:Timings = [System.Collections.Generic.List[object]]::new()
function Step($name, [scriptblock]$command) {
    Write-Host "== $name" -ForegroundColor Cyan
    $clock = [Diagnostics.Stopwatch]::StartNew()
    try {
        & $command
        if ($LASTEXITCODE) { throw "$name failed" }
    } finally {
        $script:Timings.Add([pscustomobject]@{ Stage = $name; Seconds = [math]::Round($clock.Elapsed.TotalSeconds, 1) })
    }
}

. "$PSScriptRoot\build-slot.ps1"
Push-Location (Split-Path -Parent $PSScriptRoot)
# One of the two cargo build slots for the whole gate, at about half the cores (#347).
$slot = $null
# The cargo slot is taken at the first cargo step, not at the start: the python, ops, relay-sim
# and node steps before it need no build capacity, and holding a slot through them queues
# other gates for nothing.
function Get-CargoSlot {
    if ($script:slot) { return }
    $queue = [Diagnostics.Stopwatch]::StartNew()
    $script:slot = Enter-BuildSlot
    $script:Timings.Add([pscustomobject]@{ Stage = '(waited for a cargo slot)'; Seconds = [math]::Round($queue.Elapsed.TotalSeconds, 1) })
}
# A change of only Markdown files and scripts/**/*.py cannot move the Rust or firmware results (the
# Rust sources include docs/ and scripts/ files only as .json/.jsonl fixtures), so those steps are
# skipped; the python and ops tests still run. FOXGLOVE_FULL_GATE=1 forces every step.
function Test-RustRelevant {
    if ($env:FOXGLOVE_FULL_GATE) { return $true }
    $base = (git merge-base HEAD origin/main 2>$null)
    if (-not $base) { return $true }
    $changed = @(git diff --name-only "$base" HEAD) + @(git diff --name-only HEAD) + @(git ls-files --others --exclude-standard)
    $changed = @($changed | Where-Object { $_ })
    if (-not $changed.Count) { return $true }
    $other = @($changed | Where-Object { $_ -notmatch '\.md$' -and $_ -notmatch '^scripts/.*\.py$' })
    return ($other.Count -gt 0)
}
# cargo-nextest (OPTIONAL: when absent, e.g. Defender quarantined it, check.ps1 falls back to plain cargo test), or $null: on PATH, else _tools\perf-bin\bin (or worktrees\_tools\perf-bin\bin) in this
# checkout's parent folders, so every worktree finds the one copy the machine has.
function Find-Nextest {
    $onPath = Get-Command cargo-nextest -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $dir = Split-Path -Parent $PSScriptRoot
    for ($i = 0; $i -lt 4 -and $dir; $i++) {
        $dir = Split-Path -Parent $dir
        foreach ($sub in '_tools\perf-bin\bin', 'worktrees\_tools\perf-bin\bin') {
            $exe = if ($dir) { Join-Path $dir "$sub\cargo-nextest.exe" }
            if ($exe -and (Test-Path $exe)) { return $exe }
        }
    }
    $null
}
try {
    $start = Get-Date
    $rustRelevant = Test-RustRelevant
    if (-not $rustRelevant) { Write-Host 'Docs/python-only change: Rust, firmware and node steps skipped (FOXGLOVE_FULL_GATE=1 runs them).' -ForegroundColor Yellow }
    # Safety (2026-11-20): the gate never opens a window. Tests that would are #[ignore]d and only run on
    # purpose (cargo test -- --ignored, FOXGLOVE_QUIET_DESKTOP=1, quiet desktop); this gate never passes
    # --ignored / --include-ignored, and clears the opt-in variable so a stray shell setting cannot arm them.
    Remove-Item Env:FOXGLOVE_QUIET_DESKTOP -ErrorAction SilentlyContinue
    # Source lint, runs first and needs no build: no test may create a window, take focus or start a visible
    # console unless it is #[ignore]d or vetted in scripts/window_tests_allowlist.txt.
    Step 'lint: no window-creating tests' { python scripts/check_window_tests.py --self-test; if (!$LASTEXITCODE) { python scripts/check_window_tests.py } }
    Step 'scripts: python tests' { python -m unittest discover -s scripts -p "test_*.py" }
    # The recurring ops scripts (#949) on fake inputs; window-free, no network.
    Step 'scripts: ops tests' { powershell -NoProfile -ExecutionPolicy Bypass -File scripts/ops/test-ops.ps1 }
    # The UIA tree dump's pure parts (#1376): JSON shape, expectation matching; never reads a window.
    Step 'scripts: uia_tree self-test' { powershell -NoProfile -ExecutionPolicy Bypass -File scripts/lab/uia_tree.ps1 -SelfTest }
    Step 'scripts: demo-up tests' { powershell -NoProfile -ExecutionPolicy Bypass -File scripts/ops/test-demo-up.ps1 }
    Step 'scripts: dev copy-in tests' { powershell -NoProfile -ExecutionPolicy Bypass -File scripts/ops/test-dev-copy-in.ps1 }
    # Functions over 120 lines and files over 1,500 lines, listed as warnings; never fails.
    Step 'size budget' { python scripts/size_budget.py }
    if ($rustRelevant) {
        # The left's art relay against a fake split link (tests/relay-sim, #618); CI runs it with gcc.
        if (Test-Path "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe") {
            Step 'firmware: relay-sim' { powershell -ExecutionPolicy Bypass -File tests/relay-sim/build.ps1 -Log 1 }
        } else { Write-Warning 'firmware: relay-sim: no MSVC (vswhere) found, skipped; CI runs it' }
        # The SignalRGB plugin's packets, and the receiver test's recording of them (#331).
        if (Get-Command node -ErrorAction SilentlyContinue) {
            Step 'signalrgb: packets' { node integrations/signalrgb/test.mjs; if (!$LASTEXITCODE) { node integrations/signalrgb/record-packets.mjs --check } }
        } else { Write-Warning 'signalrgb: node not found, skipped' }
        # rustfmt (default style) on the crates and Foxglove's app code. KeyPeek's upstream files in
        # app/ are left as upstream has them, so subtree syncs stay clean. Fix with the same commands
        # without --check.
        Step 'format' {
            Get-CargoSlot
            cargo fmt --all --check
            if (!$LASTEXITCODE) { rustfmt --check --edition 2021 (Get-ChildItem -Recurse app/src/foxglove -Filter *.rs).FullName }
        }
        # Licences, advisories, banned sources (deny.toml for the MIT crates, app/deny.toml for the
        # GPL app). Needs cargo-deny: cargo install --locked cargo-deny
        Step 'licences and advisories' {
            if (-not (Get-Command cargo-deny -ErrorAction SilentlyContinue)) { throw 'cargo-deny is not installed: cargo install --locked cargo-deny' }
            cargo deny --log-level error check
            if (!$LASTEXITCODE) { cargo deny --log-level error --manifest-path app/Cargo.toml check }
        }
        # The Hideout hub (hideout-shell, hideout; #932) is MIT and clean-room (contract §11): every
        # package in its graph has a permissive licence, and no path dependency points under app/.
        Step 'hub: dependency licences' { python scripts/check_hub_deps.py }
        Step 'crates: clippy' { cargo clippy --workspace --all-targets -- -D warnings }
        # cargo-nextest runs each test in its own process (about 90 s faster than cargo test on the
        # crates); without it, plain cargo test. nextest skips doc tests, so those run on their own.
        Step 'crates: tests' {
            $nextest = Find-Nextest
            if ($nextest) {
                Write-Host "crates: tests run with cargo nextest ($nextest)"
                $path = $env:PATH
                $env:PATH = "$(Split-Path -Parent $nextest);$path"
                try {
                    cargo nextest run --workspace
                    if (!$LASTEXITCODE) { cargo test --workspace --doc }
                } finally { $env:PATH = $path }
            } else {
                Write-Host 'crates: tests run with cargo test (cargo-nextest is optional and was not found; cargo install cargo-nextest --locked)'
                cargo test --workspace
            }
        }
        # The catalog signer (#785) is behind a feature the app never turns on.
        Step 'plugin-trust signer' { cargo clippy -p foxglove-plugin-trust --features sign --all-targets -- -D warnings; if (!$LASTEXITCODE) { cargo test -p foxglove-plugin-trust --features sign } }
        Step 'foxglove-link without hidapi' { cargo build -p foxglove-link --no-default-features }
        Step 'foxglove-link Devices device' { cargo clippy -p foxglove-link --features devices --all-targets -- -D warnings; if (!$LASTEXITCODE) { cargo test -p foxglove-link --features devices } }
        # The runtime accesskit_windows and eframe must be the vendored, patched copies (#1376): a
        # dependency bump to another accesskit_windows would silently drop the [patch] and bring the
        # atom-table crash back.
        Step 'app: vendored patches are the ones built' {
            foreach ($c in 'accesskit_windows', 'eframe') {
                $t = @(cargo tree --manifest-path app/Cargo.toml --locked --offline -e normal --target x86_64-pc-windows-msvc -i $c)
                if ($LASTEXITCODE) { throw "cargo tree -i $c failed" }
                $heads = @($t | Where-Object { $_ -match '^[a-z_0-9-]+ v[0-9]' })
                if ($heads.Count -ne 1 -or $heads[0] -notmatch ('app.vendor.' + $c)) {
                    Write-Host "$c is not the single vendored copy: $($heads -join ' | ')"
                    throw "$c is not the vendored copy"
                }
            }
        }
        # The same for the hub's copies (#1381): root [patch.crates-io], vendor\ at the repo root.
        Step 'hub: vendored patches are the ones built' {
            foreach ($c in 'accesskit_windows', 'eframe') {
                $t = @(cargo tree --locked --offline -e normal --target x86_64-pc-windows-msvc -p hideout-shell -i $c)
                if ($LASTEXITCODE) { throw "cargo tree -i $c failed" }
                $heads = @($t | Where-Object { $_ -match '^[a-z_0-9-]+ v[0-9]' })
                if ($heads.Count -ne 1 -or $heads[0] -notmatch ('(?<!app.)vendor.' + $c)) {
                    Write-Host "$c is not the single vendored copy: $($heads -join ' | ')"
                    throw "$c is not the vendored copy"
                }
            }
        }
        Step 'app: clippy' { cargo clippy --manifest-path app/Cargo.toml --all-targets -- -D warnings }
        Step 'app: tests' { cargo test --manifest-path app/Cargo.toml }
    }
    Write-Host ("All checks passed in {0:N0} s." -f ((Get-Date) - $start).TotalSeconds) -ForegroundColor Green
    # Disk hygiene on this event, not a timer: free other worktrees' builds that are done with.
    # Best effort; it never fails the gate. FOXGLOVE_NO_CLEAN=1 skips it.
    if (-not $env:FOXGLOVE_NO_CLEAN) {
        try { & "$PSScriptRoot\clean-worktrees.ps1" -Apply }
        catch { Write-Warning "clean-worktrees: $_" }
    }
} finally {
    if ($script:Timings.Count) {
        Write-Host '== stage timings (s)' -ForegroundColor Cyan
        foreach ($t in $script:Timings) { Write-Host ('stage-time {0,7:N1}  {1}' -f $t.Seconds, $t.Stage) }
    }
    Exit-BuildSlot $slot
    Pop-Location
}
