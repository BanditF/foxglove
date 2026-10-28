# The gate for the desktop side: lint and tests for crates/ and app/. Run it before pushing:
#   powershell -File scripts\check.ps1
# GitHub's Rust and app workflows only run by hand (Actions tab) for a Linux check or an exe.
$ErrorActionPreference = 'Stop'

function Step($name, [scriptblock]$command) {
    Write-Host "== $name" -ForegroundColor Cyan
    & $command
    if ($LASTEXITCODE) { throw "$name failed" }
}

. "$PSScriptRoot\build-slot.ps1"
Push-Location (Split-Path -Parent $PSScriptRoot)
# One of the two cargo build slots for the whole gate, at about half the cores (#347).
$slot = $null
try {
    $slot = Enter-BuildSlot
    $start = Get-Date
    Step 'scripts: python tests' { python -m unittest discover -s scripts -p "test_*.py" }
    # Functions over 120 lines and files over 1,500 lines, listed as warnings; never fails.
    Step 'size budget' { python scripts/size_budget.py }
    # The SignalRGB plugin's packets, and the receiver test's recording of them (#331).
    if (Get-Command node -ErrorAction SilentlyContinue) {
        Step 'signalrgb: packets' { node integrations/signalrgb/test.mjs; if (!$LASTEXITCODE) { node integrations/signalrgb/record-packets.mjs --check } }
    } else { Write-Warning 'signalrgb: node not found, skipped' }
    # rustfmt (default style) on the crates and Foxglove's app code. KeyPeek's upstream files in
    # app/ are left as upstream has them, so subtree syncs stay clean. Fix with the same commands
    # without --check.
    Step 'format' {
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
    Step 'crates: clippy' { cargo clippy --workspace --all-targets -- -D warnings }
    Step 'crates: tests' { cargo test --workspace }
    Step 'foxglove-link without hidapi' { cargo build -p foxglove-link --no-default-features }
    Step 'foxglove-link Devices device' { cargo clippy -p foxglove-link --features devices --all-targets -- -D warnings; if (!$LASTEXITCODE) { cargo test -p foxglove-link --features devices } }
    Step 'app: clippy' { cargo clippy --manifest-path app/Cargo.toml --all-targets -- -D warnings }
    Step 'app: tests' { cargo test --manifest-path app/Cargo.toml }
    Write-Host ("All checks passed in {0:N0} s." -f ((Get-Date) - $start).TotalSeconds) -ForegroundColor Green
    # Disk hygiene on this event, not a timer: free other worktrees' builds that are done with.
    # Best effort; it never fails the gate. FOXGLOVE_NO_CLEAN=1 skips it.
    if (-not $env:FOXGLOVE_NO_CLEAN) {
        try { & "$PSScriptRoot\clean-worktrees.ps1" -Apply }
        catch { Write-Warning "clean-worktrees: $_" }
    }
} finally {
    Exit-BuildSlot $slot
    Pop-Location
}
