<#
  push.ps1 - sends players the build you're playing (the launcher's "Push build").

    pwsh tools\push.ps1 [-NotesFile notes.txt] [-Next]

  Takes play\current\r5rust.exe (the build play.ps1 runs; -Next: the staged one) and releases exactly that exe on
  GitHub, with the scripts and content from the same commit (from its version.txt). Players' launchers download it by
  themselves and R5Rust Bot posts it in #patch-notes. NotesFile: one short player-facing line per line (empty lines
  and lines starting with # are skipped); without notes the patch notes are the commit subjects, cut short.
#>
param([string]$NotesFile = '', [switch]$Next)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$slot = Join-Path $root $(if ($Next) { 'play\next' } else { 'play\current' })
$exe = Join-Path $slot 'r5rust.exe'
$ver = Join-Path $slot 'version.txt'
if (-not (Test-Path $exe) -or -not (Test-Path $ver)) { throw "no build in $slot (play it once, or push the staged one with -Next)" }
$line = (Get-Content $ver -Raw).Trim()
$hash = ($line -split '\s+')[2]
if (-not $hash -or -not (git -C $root cat-file -t $hash 2>$null)) { throw "can't tell which commit $exe was built from ($line)" }
$notes = @()
if ($NotesFile -and (Test-Path $NotesFile)) {
  $notes = @(Get-Content $NotesFile | ForEach-Object { $_.Trim() } | Where-Object { $_ -and -not $_.StartsWith('#') })
}
Write-Host "pushing $line"
& (Join-Path $PSScriptRoot 'package.ps1') -Publish -Exe $exe -Commit $hash -Notes $notes
