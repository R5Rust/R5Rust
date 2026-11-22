<#
  retail_setup.ps1 - rebuild R5Rust's retail Apex assets (D:\R5Rust\assets-retail) from the retail install, in one go.

  Retail Apex is read from FILES ONLY. Close Apex first: this script never looks at or touches the game process
  (anti-cheat), so it can't check for you; pass -ApexIsClosed to confirm. Nothing is written inside the game folder:
  map paks are mirrored to the work folder first, RSX writes only to the work/output folders. Steps 1 (pak
  mirror), 3 (pak scan), 4 (textures' top mips), 8 (grade file), 9 and 10 read the install; the rest work on the
  mirror, so `-Only 2,5,6,7` redoes the conversion without touching the game folder.

  Steps (each documented where it's implemented; HANDOFF.md "switching to retail" has the background):
    1  tools\export-map            world.glb, props.json, collision.bin, meta.json   (BSP v54, mdl v21 via Cast, lightmaps
                                   type 11; props as a model-space LOD library + instances for runtime LOD)
    2  BSP lumps -> <map>\bsp\<id>.bin                                   (lighting data the client reads)
    3  extractor scan + shaders    _work_retail\catalog.csv, assets-retail\shaders   (skipped when present)
    4  materials + textures         RSX by guid (tools\retail_matguids.py), then extractor sort
    5  material constants           RSX raw (matl=1) + tools\retail_uber.py, then tools\build_materials.py
    6  sky cubes                    the map pak's two 2048^2 BC6H cubes -> <map>\skies\
    7  reflection cubemaps          RSX cubemap export (DDS + index json) + tools\retail_cubemaps.py -> <map>\cubemaps.dds
    8  colour grade                 <install>\<color_correction filename> -> <map>\
    9  loading screen art           <map>_loadscreen.rpak -> <out>\images\ui\maps_<map>_loadscreen\

  RSX = R5Flowstate's fork (github.com/R5Flowstate/rsx, built in D:\R5Rust\tools\rsx-fs): it reads retail models
  (mdl v20/v21), exports cubemaps, takes --exportguids, and sets formats with --exportsetting <type>=<n>. Our old
  patched build (tools\rsx-src) was deleted by Defender on 2026-11-18 and isn't needed any more.
  tools\install.ps1 (the launcher's Install button) runs these steps one at a time with its own paths.

    pwsh tools\retail_setup.ps1 -ApexIsClosed [-Map mp_rr_canyonlands_staging_mu1] [-Only 1,2,...] [-Rescan]
#>
param(
  [switch]$ApexIsClosed,
  [string]$Map = 'mp_rr_canyonlands_staging_mu1',
  [string]$Game = 'C:\Program Files\EA Games\Apex',
  [string]$Out = 'D:\R5Rust\assets-retail',
  [string]$Work = 'D:\R5Rust\_work_retail',
  # raw material exports (step 4) land here, then the extractor sorts them into $Out
  [string]$Raw = '',
  [string]$Rsx = 'D:\R5Rust\tools\rsx-fs\bin\Release\rsx.exe',
  [string]$Extractor = 'D:\R5Rust\extractor',
  # the extractor's config (game / out / work / raw / groups); install.ps1 writes one per install
  [string]$ExtractorConfig = '',
  [string]$Only = '',
  [switch]$Rescan
)
$ErrorActionPreference = 'Stop'
# -Only 2,4,5 arrives as one string under pwsh -File
$Steps = @($Only -split '[,\s]+' | Where-Object { $_ } | ForEach-Object { [int]$_ })
$readsInstall = $Steps.Count -eq 0 -or ($Steps | Where-Object { $_ -in 1, 3, 4, 8, 9, 10 })
if ($readsInstall -and -not $ApexIsClosed) { throw 'Steps 1, 3, 4, 8, 9 and 10 read the retail install: close Apex, then run again with -ApexIsClosed (this script never checks the game process).' }
$repo = Split-Path -Parent $PSScriptRoot
$mapDir = Join-Path $Out "maps\$Map"
$emWork = Join-Path $Work 'export-map'
$mirror = Join-Path $emWork 'mirror\paks\Win64'
$extractor = $Extractor
if (-not $Raw) { $Raw = Join-Path (Split-Path -Parent $Work) '_raw_retail' }
$py = if (Test-Path "$extractor\.venv\Scripts\python.exe") { "$extractor\.venv\Scripts\python.exe" } else { 'python' }
$env:R5RUST_CONFIG = if ($ExtractorConfig) { $ExtractorConfig } else { Join-Path $extractor 'config.retail.json' }
if (-not (Test-Path $Rsx)) { throw "RSX not found at $Rsx (build R5Flowstate's fork: D:\R5Rust\tools\rsx-fs, rsx.sln, x64 Release)" }
function Want($n) { $Steps.Count -eq 0 -or $Steps -contains $n }
function Step($n, $what) { Write-Host "`n== $n. $what" -ForegroundColor Cyan }
# RSX's per-type format: one `--exportsetting <type>=<n>` each (fork numbering: txtr 0 PNG top mip, 4 DDS mipmapped;
# matl 1 uber raw, 2 uber struct; uiia 0 PNG; cube 0 DDS, 1 raw + index json)
function Fmt([string[]]$settings) { $settings | ForEach-Object { '--exportsetting'; $_ } }
function Run-Rsx([string[]]$argv, [string]$cwd, [string]$what) {
  New-Item -ItemType Directory -Force $cwd | Out-Null
  $p = Start-Process -FilePath $Rsx -WorkingDirectory $cwd -PassThru -Wait -NoNewWindow -RedirectStandardOutput (Join-Path $cwd 'rsx.log') -ArgumentList $argv
  # the fork now and then dies at startup (access violation before it loads the guid list; the same run passes
  # again): one retry
  if ($p.ExitCode -ne 0) {
    Write-Host "  RSX exit $($p.ExitCode) ($what), retrying once" -ForegroundColor Yellow
    $p = Start-Process -FilePath $Rsx -WorkingDirectory $cwd -PassThru -Wait -NoNewWindow -RedirectStandardOutput (Join-Path $cwd 'rsx.log') -ArgumentList $argv
  }
  if ($p.ExitCode -ne 0) { throw "RSX failed ($what), exit $($p.ExitCode): see $cwd\rsx.log" }
}
function Q([string]$p) { "`"$p`"" }
$sw = [Diagnostics.Stopwatch]::StartNew()

if (Want 1) {
  Step 1 'map export (export-map)'
  $env:R5_GAME_ROOT = $Game; $env:R5_EXPORT_WORK = $emWork; $env:R5_RSX = $Rsx; $env:R5_RSX_MODELS = $Rsx
  # the loot's ground models go in the prop library too (step 10 writes the list: install.ps1 runs it first)
  $lootModels = Join-Path $Out 'data\loot\models.txt'
  $env:R5_EXTRA_MODELS = if (Test-Path $lootModels) { $lootModels } else { '' }
  Push-Location (Join-Path $repo 'tools\export-map')
  try {
    # rebuild when cargo is there (dev machines); a prebuilt export-map.exe is used as is otherwise
    if (Get-Command cargo -ErrorAction SilentlyContinue) {
      cargo build --release 2>&1 | Select-String '^error' | ForEach-Object { throw "export-map build: $_" }
    } elseif (-not (Test-Path .\target\release\export-map.exe)) { throw 'export-map.exe is missing and cargo is not installed' }
    & .\target\release\export-map.exe $Map --out $mapDir 2>&1 | Tee-Object -FilePath (Join-Path $Work 'export-map.log') | Select-String '^==|placed|done in|lightmap|error'
    if ($LASTEXITCODE -ne 0) { throw "export-map failed: $Work\export-map.log" }
  } finally { Pop-Location }
}

if (Want 2) {
  Step 2 'BSP lumps -> bsp\'
  $wrap = Join-Path $emWork "bsp\$Map\wrap\maps"
  $bsp = Join-Path $mapDir 'bsp'
  New-Item -ItemType Directory -Force $bsp | Out-Null
  Get-ChildItem $wrap -Filter '*.bsp_lump*' | ForEach-Object {
    if ($_.Name -match '\.bsp\.([0-9A-F]{4})\.bsp_lump') { Copy-Item $_.FullName (Join-Path $bsp "$($Matches[1]).bin") -Force }
  }
  Get-ChildItem $wrap -Filter '*.ent' | Copy-Item -Destination $bsp -Force
  "  $((Get-ChildItem $bsp -Filter *.bin).Count) lumps"
}

$catalog = Join-Path $Work 'catalog.csv'
if (Want 3) {
  Step 3 'pak scan + shaders (extractor)'
  if ($Rescan -or -not (Test-Path $catalog)) { & $py "$extractor\r5rust.py" scan }
  if (-not (Test-Path (Join-Path $Out 'shaders\sets'))) { & $py "$extractor\r5rust.py" export shaders; & $py "$extractor\r5rust.py" sort }
  else { '  shaders present' }
}

# steps 4-7 read export-map's mirrored copies (step 1), never the install
$base = @('common.rpak', 'common_mp.rpak', 'common_early.rpak', "$Map.rpak") | ForEach-Object { Q (Join-Path $mirror $_) }
$guids = Join-Path $Work 'matl_guids.txt'
if (Want 4) {
  Step 4 'materials + textures by guid'
  & python (Join-Path $repo 'tools\retail_matguids.py') (Join-Path $mapDir 'world.glb') $catalog $guids
  $dest = [IO.Path]::GetFullPath((Join-Path $Raw "materials\maps\$Map"))
  # the top mips stream from pc_all.opt.starpak (20 GB), which the mirror leaves out: RSX (patched fork) falls back to
  # the install's paks folder for starpaks it can't find next to the pak. Without it textures cap at the mandatory mip.
  $env:RSX_STARPAK_DIRS = Join-Path $Game 'paks\Win64'
  Run-Rsx (@('-nogui', '-export', '-exportfullpaths', '--loadwhitelist', 'matl,txtr,shds,shdr,msnp', '--exporttypes', 'matl', '--exportthreads', '10',
      '--exportdir', (Q $dest), '-matltextures', '--texturenames', 'semantic') + (Fmt 'matl=2', 'txtr=0') + @('--exportguids', (Q $guids)) + $base) $dest 'materials'
  & $py "$extractor\r5rust.py" sort
}

if (Want 5) {
  Step 5 'material constants + materials.json'
  $raw = Join-Path $Out '_uber_raw'
  Run-Rsx (@('-nogui', '-export', '-exportfullpaths', '--loadwhitelist', 'matl,txtr,shds,shdr,msnp', '--exporttypes', 'matl') +
      (Fmt 'matl=1') + @('--exportguids', (Q $guids), '--exportdir', (Q $raw)) + $base) $raw 'raw material constants'
  & python (Join-Path $repo 'tools\retail_uber.py') $Out
  & python (Join-Path $repo 'tools\build_materials.py') $Out $Map
}

if (Want 6) {
  Step 6 'sky cubes'
  # The map pak holds two 2048^2 6-face BC6H textures, the dome skins' cubes (retail names neither by a hash we can
  # reproduce): matched by eye on 2026-11-18, firing range = canyon + river, skulltown = desert with tyre tracks.
  $sky = @{ 'a309d30f0572218f' = 'freedm_cubemap_firingrange'; 'd2afc2386013ac7b' = 'freedm_cubemap_skulltown' }
  $tmp = Join-Path $Work 'skytex'
  Set-Content (Join-Path $Work 'sky_guids.txt') ($sky.Keys -join "`n")
  Run-Rsx (@('-nogui', '-export', '--exporttypes', 'txtr') + (Fmt 'txtr=4') + @('--exportguids', (Q (Join-Path $Work 'sky_guids.txt')),
    '--exportdir', (Q $tmp), (Q (Join-Path $mirror "$Map.rpak")))) $tmp 'sky cubes'
  New-Item -ItemType Directory -Force (Join-Path $mapDir 'skies') | Out-Null
  foreach ($g in $sky.Keys) {
    $f = Get-ChildItem $tmp -Recurse -Filter "0x$g.dds" | Select-Object -First 1
    if (-not $f) { throw "sky cube 0x$g wasn't exported (a retail update may have changed it: check the map pak's 2048^2 cube textures)" }
    Copy-Item $f.FullName (Join-Path $mapDir "skies\$($sky[$g]).dds") -Force
  }
  "  $($sky.Count) sky cubes"
}

if (Want 7) {
  Step 7 'reflection cubemaps'
  $tmp = Join-Path $Work 'cubes'
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
  # the fork's cubemap type: once as DDS (decompressed, face-major), once raw for the json that carries each cube's
  # index (= the BSP cubemap lump order the client reads them in)
  foreach ($s in 'cube=0', 'cube=1') {
    Run-Rsx (@('-nogui', '-export', '--exporttypes', 'cube') + (Fmt $s) + @('--exportdir', (Q $tmp), (Q (Join-Path $mirror "$Map.rpak")))) $tmp "cubemaps ($s)"
  }
  & python (Join-Path $repo 'tools\retail_cubemaps.py') $tmp (Join-Path $mapDir 'cubemaps.dds')
  if ($LASTEXITCODE -ne 0) { throw 'retail_cubemaps.py failed' }
}

if (Want 8) {
  Step 8 'colour grade'
  $ents = (Get-ChildItem (Join-Path $mapDir 'bsp') -Filter '*.ent' | ForEach-Object { [IO.File]::ReadAllText($_.FullName) }) -join "`n"
  $cc = [regex]::Match($ents, '\{[^{}]*"classname" "color_correction"[^{}]*\}').Value
  $file = [regex]::Match($cc, '"filename" "([^"]+)"').Groups[1].Value
  if (-not $file) { throw 'no color_correction entity in the map' }
  Copy-Item (Join-Path $Game ($file -replace '/', '\')) $mapDir -Force
  "  $file"
}

if (Want 9) {
  Step 9 'loading screen art'
  # The launcher's map art and the game's loading screen read <out>\images\ui\maps_<map>_loadscreen\*.png
  # (launcher src\game.rs GameFiles, client Config::loadscreen). Retail ships it in <map>_loadscreen.rpak.
  $pak = Join-Path $Game "paks\Win64\$($Map)_loadscreen.rpak"
  $tmp = Join-Path $Work 'loadscreen'
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
  Run-Rsx (@('-nogui', '-export', '--exporttypes', 'uiia,txtr') + (Fmt 'uiia=0', 'txtr=0') + @('--exportdir', (Q $tmp), (Q $pak))) $tmp 'loadscreen'
  $dst = Join-Path $Out "images\ui\maps_$($Map)_loadscreen"
  New-Item -ItemType Directory -Force $dst | Out-Null
  $pngs = Get-ChildItem $tmp -Recurse -Filter *.png | Sort-Object Length -Descending
  if (-not $pngs) { throw "no images in $pak" }
  $pngs | ForEach-Object { Copy-Item $_.FullName $dst -Force }
  "  $($pngs.Count) images -> $dst (largest: $($pngs[0].Name))"
}

if (Want 10) {
  Step 10 'loot + weapon data (inventory)'
  # every datatable + weapon settings file of common.rpak (tiny), then tools\build_loot.py picks the four loot tables
  # by their columns and writes <out>\data\loot\ (docs/inventory.md)
  $tmp = Join-Path $Work 'loot'
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
  # before the first map export there's no pak mirror yet: read the install's common paks (Apex closed)
  $paks = if (Test-Path (Join-Path $mirror 'common.rpak')) { $mirror } else { Join-Path $Game 'paks\Win64' }
  # wepn=1: the KeyValues text (the fork's default is JSON); the loot tables are spread over the three common paks
  Run-Rsx (@('-nogui', '-export', '--loadwhitelist', 'dtbl,wepn', '--exporttypes', 'dtbl,wepn') + (Fmt 'wepn=1') + @('--exportdir', (Q $tmp)) +
    @('common.rpak', 'common_mp.rpak', 'common_early.rpak' | ForEach-Object { Q (Join-Path $paks $_) })) $tmp 'datatables + weapon settings'
  $wepn = Get-ChildItem $tmp -Recurse -Filter 'mp_weapon_r97.txt' | Select-Object -First 1
  if (-not $wepn) { throw "no weapon settings exported to $tmp" }
  # icons: apex-content-kit's extraction of retail's ui paks, when it's on this PC
  $icons = 'C:\Code\apex\apex-content-kit\public\game\icons'
  & python (Join-Path $repo 'tools\build_loot.py') $tmp $wepn.DirectoryName $Out $icons
  if ($LASTEXITCODE -ne 0) { throw 'build_loot.py failed' }
}

Write-Host ("`nretail assets ready in {0} ({1:n0} s). Set assets_root = '{0}' in r5rust.toml." -f $Out, $sw.Elapsed.TotalSeconds) -ForegroundColor Green
