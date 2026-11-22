<#
  package.ps1 - builds the player zip the launcher downloads, and (with -Publish) releases it on GitHub.

    pwsh tools\package.ps1 [-Publish] [-Tools 1] [-Notes 'Short line one','Short line two'] [-Exe <r5rust.exe> -Commit <hash>]

  -Exe / -Commit (tools\push.ps1): ship that exact exe (no compile) with the files from that commit.

  -Notes: the player-facing lines the Discord bot posts in #patch-notes (R5Flowstate style: short, plain, no
  commit-speak). Without them the bot cuts the commit subjects down instead.

  The zip is laid out like the game folder, so the launcher just unpacks it over a player's game folder:
    play.ps1, content\, cfg\, mods\README.md, tools\*.ps1 + *.py (the install pipeline), tools\bin\export-map\export-map.exe,
    play\next\r5rust.exe + version.txt (play.ps1 swaps it in on the next launch), notes.json (patch notes, newest first),
    tools.txt (the install-tools release it needs: R5Rust/R5Rust release tools-<n>, made by tools\bundle_tools.ps1).
  Only committed files go in (git archive HEAD), never game assets. Binaries are built with the user folder and C:\Code
  remapped out of their embedded paths. Release tag: build-<yyyy.MM.dd>-<local short hash>.
  -Publish needs gh logged in with push rights on R5Rust/R5Rust; the tag goes on the public repo's main.
#>
param(
  [switch]$Publish,
  [int]$Tools = 1,
  [string[]]$Notes = @(),
  [string]$Repo = 'R5Rust/R5Rust',
  [string]$TargetDir = 'D:\R5Rust\_build\game',
  [string]$Exe = '',
  [string]$Commit = 'HEAD'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
  if (-not $Exe -and (git status --porcelain --untracked-files=no -- crates tools play.ps1 content cfg)) { Write-Warning 'uncommitted changes: the zip has committed files only (the exes are built from the working tree)' }
  $hash = (git rev-parse --short $Commit).Trim()
  # unique per build (a rebuild of the same commit is still new for players): the launcher compares release.txt
  $tag = 'build-{0}-{1}' -f (Get-Date -Format 'yyyy.MM.dd.HHmm'), $hash
  $dist = Join-Path $root 'dist'
  $stage = Join-Path $dist "stage-$tag"
  Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $stage | Out-Null

  # 1. binaries, without this PC's paths baked in
  $env:CARGO_TARGET_DIR = $TargetDir
  # crt-static: no VCRUNTIME140.dll needed on the player's PC (a clean Windows may not have the VC++ redist)
  $env:RUSTFLAGS = "-C target-feature=+crt-static --remap-path-prefix=$env:USERPROFILE=~ --remap-path-prefix=C:\Code=src"
  if (-not $Exe) {
    cargo build --release --bin r5rust
    if ($LASTEXITCODE) { throw 'r5rust build failed' }
    $Exe = Join-Path $TargetDir 'release\r5rust.exe'
  }
  Push-Location tools\export-map
  try { cargo build --release; if ($LASTEXITCODE) { throw 'export-map build failed' } } finally { Pop-Location }
  Remove-Item Env:RUSTFLAGS, Env:CARGO_TARGET_DIR

  # 2. committed files the player needs
  $paths = @('play.ps1', 'content', 'cfg', 'mods/README.md', 'tools')
  $tar = Join-Path $dist "files-$hash.tar"
  git archive --format=tar -o $tar $Commit -- @paths
  & "$env:SystemRoot\System32\tar.exe" -xf $tar -C $stage
  Remove-Item $tar
  # dev-only tool sources stay out (export-map ships as an exe, the probe is for R5Flowstate recordings)
  foreach ($d in 'tools\export-map', 'tools\r5f-probe') { Remove-Item -Recurse -Force (Join-Path $stage $d) -ErrorAction SilentlyContinue }

  New-Item -ItemType Directory -Force (Join-Path $stage 'play\next'), (Join-Path $stage 'tools\bin\export-map') | Out-Null
  Copy-Item $Exe (Join-Path $stage 'play\next\r5rust.exe')
  Copy-Item (Join-Path $TargetDir 'release\export-map.exe') (Join-Path $stage 'tools\bin\export-map\export-map.exe')
  # what the launcher shows next to the build: the first player-facing note, not commit-speak
  $subject = if ($Notes.Count) { $Notes[0] } else { (git log -1 --format=%s $Commit).Trim() }
  [IO.File]::WriteAllText((Join-Path $stage 'play\next\version.txt'), ('{0}  {1} {2}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm'), $hash, $subject))
  [IO.File]::WriteAllText((Join-Path $stage 'tools.txt'), "$Tools")
  [IO.File]::WriteAllText((Join-Path $stage 'release.txt'), $tag)

  # 3. patch notes: the last 100 commits ($history, not $notes: PowerShell names ignore case and -Notes is a parameter) (fields split by 0x1f, records by 0x1e)
  $raw = git log -n100 $Commit --date=format:'%Y-%m-%d %H:%M' --format='%h%x1f%ad%x1f%s%x1f%b%x1e'
  $history = foreach ($rec in (($raw -join "`n") -split [char]0x1e)) {
    $f = $rec.TrimStart("`r", "`n") -split [char]0x1f
    if ($f.Count -ge 3 -and $f[0]) { [ordered]@{ hash = $f[0].Trim(); date = $f[1]; subject = $f[2]; body = $(if ($f.Count -gt 3) { $f[3].Trim() } else { '' }) } }
  }
  [IO.File]::WriteAllText((Join-Path $stage 'notes.json'), (ConvertTo-Json -InputObject @($history) -Depth 3 -AsArray))

  # 4. nothing personal may ship
  $leak = Get-ChildItem $stage -Recurse -File | Where-Object { $_.Extension -in '.ps1', '.py', '.json', '.txt', '.md', '.toml', '.cfg', '.mjs', '.js' } |
    Select-String -Pattern ([regex]::Escape($env:USERPROFILE)), 'example\.invalid' -SimpleMatch:$false -List
  if ($leak) { throw "personal paths in the package: $($leak.Path -join ', ')" }
  foreach ($bin in Get-ChildItem $stage -Recurse -Filter *.exe) {
    if (Select-String -LiteralPath $bin.FullName -Pattern ([regex]::Escape($env:USERPROFILE)) -Encoding ascii -List) { throw "$($bin.Name) has the user folder baked in" }
  }

  $zip = Join-Path $dist "r5rust-$tag.zip"
  Remove-Item $zip -ErrorAction SilentlyContinue
  Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
  Remove-Item -Recurse -Force $stage
  '{0}  {1:N1} MB' -f $zip, ((Get-Item $zip).Length / 1MB)

  if ($Publish) {
    # release notes: commits since the newest published build (its tag ends in the local hash)
    $prev = gh release list -R $Repo --limit 50 --json tagName -q '.[].tagName' | Where-Object { $_ -like 'build-*' } | Select-Object -First 1
    $range = if ($prev -and (git cat-file -t ($prev -split '-')[-1] 2>$null) -eq 'commit') { "$(($prev -split '-')[-1])..$Commit" } else { '-n20' }
    $lines = @(git log $range --format='- %s' $(if ($range -eq '-n20') { $Commit }))
    $patch = if ($Notes.Count) { "### Patch notes`n" + (($Notes | ForEach-Object { "- $_" }) -join "`n") + "`n`n" } else { '' }
    $body = "Game build $hash. Get it with the **R5Rust Launcher** (Play page > Download / Update); you don't need to download this zip by hand.`n`n" + $patch +
      "### Changes`n" + ($lines -join "`n") + "`n`nNeeds install tools ``tools-$Tools``. No game files are included: the launcher's Install builds them from your own Apex install."
    $notesFile = Join-Path $dist "notes-$tag.md"
    [IO.File]::WriteAllText($notesFile, $body)
    gh release create $tag $zip -R $Repo --target main --title "Build $((Get-Date -Format 'yyyy-MM-dd')) ($hash)" --notes-file $notesFile
    if ($LASTEXITCODE) { throw 'gh release create failed' }
    Remove-Item $notesFile
  }
} finally { Pop-Location }
