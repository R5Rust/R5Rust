<#
  install.ps1 - makes everything R5Rust needs from the player's own Apex install, in one go. The launcher's INSTALL
  button runs this; it also works by hand. R5Rust ships no game files: they're built here, from your copy of Apex.

  Apex is read from FILES ONLY and must be closed: nothing here ever looks at or touches the game process (anti-cheat),
  so it can't check for you; pass -ApexIsClosed to confirm. Nothing is written inside the Apex folder.

  Steps (tools\retail_setup.ps1 does the work, one step per call; see its header for what each one makes):
    map          map geometry, props, collision      (export-map)          ~10 min, reads Apex
    lumps        lighting data                       (BSP lumps)
    catalog      asset catalog + shaders             (extractor)           reads Apex
    materials    materials + textures                (RSX by guid)         reads Apex (top mips: pc_all.opt.starpak)
    constants    material settings                   (RSX raw + dxc)
    sky          sky cubes
    reflections  reflection cubemaps                 optional: reflections off without it
    grade        colour grade                        reads Apex
    loadscreen   loading screen art                  optional, reads Apex
    loot         loot tables + weapon settings       (inventory, docs/inventory.md)
  The first-person items (R-99, arms, heirlooms, sounds, decals, effects) aren't part of the install yet: they still
  come from one-off exports (HANDOFF.md). The game runs without them (no weapon in hand).

  What got made is stamped in <assets>\install.json: the pipeline version, the Apex files it came from (size + date)
  and every finished step. A later run skips steps that are still current, so a failed install resumes, and an Apex
  patch or a new pipeline version redoes everything. When the map is complete, r5rust.toml's assets_root points here.

  Machine-readable lines for the launcher (everything else is the steps' own log):
    @@apex <path>                      -Detect: an Apex install that has the map (newest first)
    @@tool <name> ok|missing <detail>  tool check
    @@plan <id>:<weight>:<title>|...   the steps this run will do (weights ~ minutes)
    @@step <id> / @@ok <id> <seconds> / @@skip <id> <why> / @@fail <id> <why>
    @@status <installed|partial|stale|missing> <detail>    -Check
    @@done ok|failed <message>

    pwsh tools\install.ps1 -ApexIsClosed [-Apex <dir>] [-Assets <dir>] [-Only map,sky] [-Force]
    pwsh tools\install.ps1 -Detect | -Check [-Assets <dir>]
#>
param(
  [string]$Apex = '',
  [string]$Assets = '',
  [string]$Work = '',
  [string]$Map = 'mp_rr_canyonlands_staging_mu1',
  [switch]$ApexIsClosed,
  [string]$Only = '',
  [switch]$Force,
  [switch]$Detect,
  [switch]$Check,
  [string]$Rsx = '',
  [string]$Extractor = '',
  [string]$Dxc = '',
  # prints made-up progress (no files touched) so the launcher's install view can be checked: launcher --install-demo
  [switch]$Demo
)
$ErrorActionPreference = 'Stop'
# Bump when a step's output changes in a way old installs should be redone for (the launcher shows "update needed").
$PipelineVersion = 3
$repo = Split-Path -Parent $PSScriptRoot
$setup = Join-Path $PSScriptRoot 'retail_setup.ps1'

$Steps = @(
  # loot first: the map export puts the loot's ground models in the prop library (models.txt)
  @{ id = 'loot';        n = 10; w = 1; title = 'Loot and weapon data'; reads = $true }
  @{ id = 'map';         n = 1; w = 10; title = 'Map geometry, props and collision'; reads = $true }
  @{ id = 'lumps';       n = 2; w = 1;  title = 'Lighting data' }
  @{ id = 'catalog';     n = 3; w = 3;  title = 'Asset catalog and shaders'; reads = $true }
  @{ id = 'materials';   n = 4; w = 4;  title = 'Materials and textures'; reads = $true }
  @{ id = 'constants';   n = 5; w = 2;  title = 'Material settings' }
  @{ id = 'sky';         n = 6; w = 2;  title = 'Sky' }
  @{ id = 'reflections'; n = 7; w = 1;  title = 'Reflections'; optional = $true }
  @{ id = 'grade';       n = 8; w = 1;  title = 'Colour grade'; reads = $true }
  @{ id = 'loadscreen';  n = 9; w = 1;  title = 'Loading screen'; reads = $true; optional = $true }
)

function Say([string]$line) { [Console]::Out.WriteLine($line); [Console]::Out.Flush() }

# ---- where things are --------------------------------------------------------------------------------------------

function Read-TomlString([string]$path, [string]$key) {
  if (-not (Test-Path $path)) { return $null }
  foreach ($l in Get-Content $path) {
    if ($l -match "^\s*$key\s*=\s*['""](.*)['""]\s*$") { return $Matches[1] }
  }
  $null
}

function Find-Apex {
  $c = [System.Collections.Generic.List[string]]::new()
  foreach ($k in 'HKLM:\SOFTWARE\Respawn\Apex', 'HKLM:\SOFTWARE\WOW6432Node\Respawn\Apex') {
    try { $v = (Get-ItemProperty $k -ErrorAction Stop).'Install Dir'; if ($v) { $c.Add($v.TrimEnd('\')) } } catch {}
  }
  $c.Add('C:\Program Files\EA Games\Apex')
  $steam = $null
  try { $steam = (Get-ItemProperty 'HKCU:\Software\Valve\Steam' -ErrorAction Stop).SteamPath } catch {}
  if ($steam) {
    $lib = Join-Path $steam 'steamapps\libraryfolders.vdf'
    if (Test-Path $lib) {
      foreach ($m in [regex]::Matches((Get-Content $lib -Raw), '"path"\s+"([^"]+)"')) {
        $c.Add((Join-Path ($m.Groups[1].Value -replace '\\\\', '\') 'steamapps\common\Apex Legends'))
      }
    }
  }
  # file metadata only: an install counts when it has this map's pak
  @($c | Select-Object -Unique | Where-Object { Test-Path -LiteralPath (Join-Path $_ "paks\Win64\$Map.rpak") } |
    Sort-Object { (Get-Item -LiteralPath (Join-Path $_ 'paks\Win64\common.rpak') -ErrorAction SilentlyContinue).LastWriteTimeUtc } -Descending)
}

function First-Existing([string[]]$paths) { foreach ($p in $paths) { if ($p -and (Test-Path -LiteralPath $p)) { return $p } }; $null }

function Find-Dxc {
  $kits = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin\*\x64\dxc.exe' -ErrorAction SilentlyContinue |
    Sort-Object { [version]($_.Directory.Parent.Name) } -Descending | Select-Object -ExpandProperty FullName
  First-Existing (@($Dxc, $env:R5_DXC, (Join-Path $repo 'tools\bin\dxc\dxc.exe')) + @($kits))
}

# The files a stamp remembers: what the steps read from the install. Size + last write only (never opened here).
function Apex-Files([string]$dir) {
  $o = [ordered]@{}
  foreach ($rel in @('paks\Win64\common.rpak', 'paks\Win64\common_mp.rpak', 'paks\Win64\common_early.rpak', 'paks\Win64\patch_master.rpak',
      "paks\Win64\$Map.rpak", "paks\Win64\$($Map)_loadscreen.rpak")) {
    $f = Get-Item -LiteralPath (Join-Path $dir $rel) -ErrorAction SilentlyContinue
    if ($f) { $o[$rel] = "$($f.Length)@$($f.LastWriteTimeUtc.ToString('o'))" }
  }
  $o
}

if (-not $Assets) {
  $Assets = Read-TomlString (Join-Path $repo 'r5rust.toml') 'assets_root'
  if (-not $Assets) { $Assets = Join-Path $env:LOCALAPPDATA 'R5Rust\assets' }
}
$Assets = [IO.Path]::GetFullPath($Assets)
if (-not $Work) { $Work = Join-Path (Split-Path -Parent $Assets) '_work_retail' }
$stampPath = Join-Path $Assets 'install.json'

function Load-Stamp {
  if (Test-Path $stampPath) { try { return Get-Content $stampPath -Raw | ConvertFrom-Json -AsHashtable } catch {} }
  $null
}

function Same-Files($a, $b) {
  if (-not $a -or -not $b -or $a.Count -ne $b.Count) { return $false }
  foreach ($k in $a.Keys) { if ($a[$k] -ne $b[$k]) { return $false } }
  $true
}

# ---- -Detect / -Check --------------------------------------------------------------------------------------------

if ($Detect) {
  foreach ($a in Find-Apex) { Say "@@apex $a" }
  exit 0
}

if ($Demo) {
  Say '@@apex C:\Program Files\EA Games\Apex'
  Say ('@@plan ' + (($Steps | ForEach-Object { "$($_.id):$($_.w):$($_.title)" }) -join '|'))
  foreach ($s in $Steps[0..3]) { Say "@@step $($s.id)"; Say "  == $($s.n). $($s.title)"; Start-Sleep -Milliseconds 300; Say "@@ok $($s.id) $(17 * $s.w)" }
  Say "@@step $($Steps[4].id)"; Say '  RSX: exporting 574 materials (raw constants)'
  Start-Sleep -Seconds 30
  exit 0
}

if ($Check) {
  $s = Load-Stamp
  if (-not $s) {
    # files made by hand before install.json existed (retail_setup.ps1 runs) still play: call them installed
    $mapDir = Join-Path $Assets "maps\$Map"
    if (@('world.glb', 'collision.bin', 'materials.json', 'meta.json') | Where-Object { -not (Test-Path (Join-Path $mapDir $_)) }) {
      Say "@@status missing nothing installed in $Assets"
    } else { Say '@@status installed made by hand (no install.json): Reinstall to track Apex updates' }
    exit 0
  }
  $done = @($Steps | Where-Object { $s.steps -and $s.steps[$_.id] } | ForEach-Object { $_.id })
  $need = @($Steps | Where-Object { -not $_.optional -and $done -notcontains $_.id } | ForEach-Object { $_.id })
  if ($need.Count) { Say "@@status partial not finished: $($need -join ', ')"; exit 0 }
  if ([int]$s.pipeline -lt $PipelineVersion) { Say "@@status stale R5Rust's installer changed (v$($s.pipeline) -> v$PipelineVersion)"; exit 0 }
  if ($s.apex -and (Test-Path -LiteralPath $s.apex) -and -not (Same-Files $s.apexFiles (Apex-Files $s.apex))) {
    Say "@@status stale Apex was updated since the install"; exit 0
  }
  Say "@@status installed $($s.finished)"
  exit 0
}

# ---- install -----------------------------------------------------------------------------------------------------

try {
  if (-not $ApexIsClosed) { throw 'Close Apex first, then confirm with -ApexIsClosed (R5Rust never looks at the running game).' }
  if (-not $Apex) { $Apex = @(Find-Apex)[0] }
  if (-not $Apex -or -not (Test-Path -LiteralPath (Join-Path $Apex "paks\Win64\$Map.rpak"))) {
    throw "Apex wasn't found (or it has no $Map). Choose the Apex folder: the one with paks\Win64 in it."
  }
  if ($Assets.StartsWith($Apex.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'The files folder is inside the Apex folder; pick another one.'
  }
  Say "@@apex $Apex"

  # tools: the bundled copy (tools\bin) first, then this machine's dev copies
  $Rsx = First-Existing @($Rsx, (Join-Path $repo 'tools\bin\rsx\rsx.exe'), 'D:\R5Rust\tools\rsx-fs\bin\Release\rsx.exe')
  $Extractor = First-Existing @($Extractor, (Join-Path $repo 'tools\bin\extractor'), 'D:\R5Rust\extractor')
  $dxcExe = Find-Dxc
  $exportMap = Join-Path $repo 'tools\export-map\target\release\export-map.exe'
  $cargo = [bool](Get-Command cargo -ErrorAction SilentlyContinue)
  $python = [bool](Get-Command python -ErrorAction SilentlyContinue)
  # objects, not nested @(): pwsh flattens an @() of one-per-line @() rows into a single list
  function Tool($name, $ok, $detail) { [pscustomobject]@{ Name = $name; Ok = [bool]$ok; Detail = $detail } }
  $tools = @(
    (Tool 'RSX (R5Flowstate fork)' $Rsx $(if ($Rsx) { $Rsx } else { 'build github.com/R5Flowstate/rsx (rsx.sln, x64 Release)' }))
    (Tool 'extractor' $Extractor $(if ($Extractor) { $Extractor } else { 'D:\R5Rust\extractor is missing' }))
    (Tool 'dxc (shader disassembler)' $dxcExe $(if ($dxcExe) { $dxcExe } else { 'install the Windows SDK, or put dxc.exe in tools\bin\dxc' }))
    (Tool 'export-map' ((Test-Path $exportMap) -or $cargo) $(if (Test-Path $exportMap) { $exportMap } elseif ($cargo) { 'built on first use' } else { 'not built and no cargo' }))
    (Tool 'python' $python $(if ($python) { (Get-Command python).Source } else { 'install Python 3.11+' }))
  )
  $missing = @()
  foreach ($t in $tools) {
    Say ("@@tool {0} {1} {2}" -f ($t.Name -replace ' ', '_'), $(if ($t.Ok) { 'ok' } else { 'missing' }), $t.Detail)
    if (-not $t.Ok) { $missing += $t.Name }
  }
  if ($missing.Count) { throw "Missing tools: $($missing -join ', ')" }
  $env:R5_DXC = $dxcExe

  # the extractor's config for this install (its game / out / work / raw / RSX)
  New-Item -ItemType Directory -Force $Assets, $Work | Out-Null
  $raw = Join-Path (Split-Path -Parent $Work) '_raw_retail'
  $cfgPath = Join-Path $Work 'extractor.json'
  [ordered]@{ game = $Apex; out = $Assets; work = $Work; raw = $raw; rsx = $Rsx; rsx_list = $Rsx
    groups = @('core', "maps/$Map") } | ConvertTo-Json | Set-Content $cfgPath -Encoding utf8

  # what's already current: same pipeline + same Apex files -> keep the step
  $apexFiles = Apex-Files $Apex
  $stamp = Load-Stamp
  $fresh = $stamp -and [int]$stamp.pipeline -eq $PipelineVersion -and $stamp.apex -eq $Apex -and $stamp.map -eq $Map -and
    (Same-Files $stamp.apexFiles $apexFiles) -and -not $Force
  $done = if ($fresh -and $stamp.steps) { $stamp.steps } else { @{} }
  $want = @($Only -split '[,\s]+' | Where-Object { $_ })
  $plan = @($Steps | Where-Object { if ($want.Count) { $want -contains $_.id } else { -not $done[$_.id] } })
  Say ('@@plan ' + (($plan | ForEach-Object { "$($_.id):$($_.w):$($_.title)" }) -join '|'))

  $save = {
    [ordered]@{ pipeline = $PipelineVersion; map = $Map; apex = $Apex; apexFiles = $apexFiles; steps = $done
      complete = -not @($Steps | Where-Object { -not $_.optional -and -not $done[$_.id] }).Count
      finished = (Get-Date).ToString('yyyy-MM-dd HH:mm') } | ConvertTo-Json -Depth 5 | Set-Content $stampPath -Encoding utf8
  }

  $failed = $null
  foreach ($s in $plan) {
    Say "@@step $($s.id)"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & pwsh -NoProfile -NonInteractive -File $setup -Only $s.n -ApexIsClosed -Map $Map -Game $Apex -Out $Assets -Work $Work `
      -Raw $raw -Rsx $Rsx -Extractor $Extractor -ExtractorConfig $cfgPath 2>&1 | ForEach-Object { Say ("  $_" -replace "`e\[[0-9;]*m", '') }
    $code = $LASTEXITCODE
    if ($code -eq 0) {
      $done[$s.id] = @{ at = (Get-Date).ToString('yyyy-MM-dd HH:mm'); secs = [int]$sw.Elapsed.TotalSeconds }
      & $save
      Say "@@ok $($s.id) $([int]$sw.Elapsed.TotalSeconds)"
    } elseif ($s.optional) {
      Say "@@skip $($s.id) didn't work (exit $code); the game runs without it"
    } else {
      $failed = "$($s.title) failed (exit $code)"
      Say "@@fail $($s.id) $failed"
      break
    }
  }
  & $save
  if ($failed) { throw $failed }

  # point the game at the files (r5rust.toml is per machine and gitignored)
  $toml = Join-Path $repo 'r5rust.toml'
  $lines = @(if (Test-Path $toml) { Get-Content $toml } else { @() })
  $entry = "assets_root = '$Assets'"
  if ($lines -match '^\s*assets_root\s*=') { $lines = $lines -replace '^\s*assets_root\s*=.*$', $entry } else { $lines = @($entry) + $lines }
  Set-Content $toml $lines -Encoding utf8
  Say "@@done ok Installed to $Assets"
  exit 0
} catch {
  Say "@@done failed $($_.Exception.Message)"
  exit 1
}
