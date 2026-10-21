# The gate for the desktop side: lint and tests for crates/ and app/. Run it before pushing:
#   powershell -File scripts\check.ps1
# GitHub's Rust and app workflows only run by hand (Actions tab) for a Linux check or an exe.
$ErrorActionPreference = 'Stop'

function Step($name, [scriptblock]$command) {
    Write-Host "== $name" -ForegroundColor Cyan
    & $command
    if ($LASTEXITCODE) { throw "$name failed" }
}

Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $start = Get-Date
    Step 'crates: clippy' { cargo clippy --workspace --all-targets -- -D warnings }
    Step 'crates: tests' { cargo test --workspace }
    Step 'digitalis without hidapi' { cargo build -p digitalis --no-default-features }
    Step 'app: clippy' { cargo clippy --manifest-path app/Cargo.toml --all-targets -- -D warnings }
    Step 'app: tests' { cargo test --manifest-path app/Cargo.toml }
    Write-Host ("All checks passed in {0:N0} s." -f ((Get-Date) - $start).TotalSeconds) -ForegroundColor Green
} finally {
    Pop-Location
}
