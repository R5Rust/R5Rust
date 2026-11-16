# Builds the client and stages it for play.ps1. Safe while the game is running:
# it never touches the copy you're playing; play.ps1 swaps the new build in on next launch.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    cargo build --release --bin r5rust
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
    $next = Join-Path $root 'play\next'
    New-Item -ItemType Directory -Force $next | Out-Null
    Copy-Item target\release\r5rust.exe (Join-Path $next 'r5rust.exe') -Force
    $stamp = "{0}  {1}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm'), (git log -1 --format='%h %s')
    Set-Content (Join-Path $next 'version.txt') $stamp
    Write-Host "staged: $stamp"
} finally { Pop-Location }
