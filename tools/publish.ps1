# Builds the client and stages it for play.ps1 (the launcher's "Build latest"). Safe while the game is running:
# it never touches the copy you're playing; play.ps1 swaps the new build in on next launch.
# Local only: players get a build when you push the one you played (tools\push.ps1, the launcher's "Push build").
# It's compiled the way player builds are (static C runtime, this PC's paths remapped out), so the exe you test is
# byte for byte the one a push sends.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$target = 'D:\R5Rust\_build\game'
Push-Location $root
try {
    $env:CARGO_TARGET_DIR = $target
    $env:RUSTFLAGS = "-C target-feature=+crt-static --remap-path-prefix=$env:USERPROFILE=~ --remap-path-prefix=C:\Code=src"
    try {
        cargo build --release --bin r5rust
        if ($LASTEXITCODE -ne 0) { throw "build failed" }
    } finally { Remove-Item Env:RUSTFLAGS, Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue }
    $next = Join-Path $root 'play\next'
    New-Item -ItemType Directory -Force $next | Out-Null
    Copy-Item (Join-Path $target 'release\r5rust.exe') (Join-Path $next 'r5rust.exe') -Force
    $stamp = "{0}  {1}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm'), (git log -1 --format='%h %s')
    Set-Content (Join-Path $next 'version.txt') $stamp
    Write-Host "staged: $stamp"
} finally { Pop-Location }
