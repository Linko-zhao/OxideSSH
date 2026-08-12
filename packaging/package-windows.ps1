$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $RepoRoot
try {
    # No certificate, thumbprint, timestamp server, or signing command is configured.
    # The MSI is intentionally unsigned.
    $Target = "x86_64-pc-windows-msvc"
    cargo build -p oxide-ssh-desktop --release --target $Target --locked
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    cargo packager --release --target $Target --formats wix
    if ($LASTEXITCODE -ne 0) { throw "cargo packager failed" }
}
finally {
    Pop-Location
}
