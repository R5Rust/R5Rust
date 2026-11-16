# Launches your copy of the game (play\current). If a newer build was staged by tools\publish.ps1,
# it's swapped in first. Building in target\ never touches this copy, so you can play while code changes.
$root = $PSScriptRoot
$cur = Join-Path $root 'play\current'
$next = Join-Path $root 'play\next'
New-Item -ItemType Directory -Force $cur | Out-Null
if (Test-Path (Join-Path $next 'r5rust.exe')) {
    try {
        Move-Item (Join-Path $next '*') $cur -Force -ErrorAction Stop
        Write-Host "updated to: $(Get-Content (Join-Path $cur 'version.txt'))"
    } catch { Write-Host "update waiting (is the game still open?): $_" }
}
if (-not (Test-Path (Join-Path $cur 'r5rust.exe'))) { Write-Host "no build yet: run tools\publish.ps1"; exit 1 }
Push-Location $root
try { & (Join-Path $cur 'r5rust.exe') @args } finally { Pop-Location }
