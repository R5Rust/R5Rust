<#
  extract_weapon.ps1 - pulls the R-99 (mp_weapon_r97) out of retail Apex for the first-person weapon, then bakes it
  (tools/build_weapon.py -> D:\R5Rust\assets-retail\weapons\mp_weapon_r97).

  Uses the official RSX 2.4.1 only (our patched build trips Windows Defender, HANDOFF.md). Everything but the textures
  reads export-map's pak mirror; the textures need the install itself (their top mips live in pc_all.opt.starpak,
  which the mirror doesn't carry), so close Apex and pass -ApexIsClosed. Never touches the game process.

  Retail paks have no asset names; RSX resolves most from its own tables, and the rest are found by StringToGuid
  (`export-map --guid <path>`). What each step found is in HANDOFF.md "R-99".

    pwsh tools\extract_weapon.ps1 -ApexIsClosed
#>
param(
  [switch]$ApexIsClosed,
  [string]$Game = 'C:\Program Files\EA Games\Apex',
  [string]$Work = 'D:\R5Rust\_work_retail\weapon',
  [string]$Mirror = 'D:\R5Rust\_work_retail\export-map\mirror\paks\Win64',
  [string]$Rsx = 'D:\R5Rust\tools\rsx-2.4.1\bin\rsx.exe'
)
$ErrorActionPreference = 'Stop'
if (-not $ApexIsClosed) { throw 'The textures are read from the retail install: close Apex, then run again with -ApexIsClosed.' }
$common = Join-Path $Mirror 'common.rpak'
New-Item -ItemType Directory -Force $Work | Out-Null
function Rsx([string]$what, [string[]]$argv) {
  Write-Host "== $what" -ForegroundColor Cyan
  $p = Start-Process -FilePath $Rsx -WorkingDirectory $Work -PassThru -Wait -NoNewWindow -RedirectStandardOutput (Join-Path $Work 'rsx.log') -ArgumentList $argv
  if ($p.ExitCode -ne 0) { Write-Warning "${what}: RSX exit $($p.ExitCode) (see $Work\rsx.log)" }
}
function Q([string]$p) { "`"$p`"" }

# weapon settings (all 324 wepn assets are tiny): mp_weapon_r97.txt + viewkick_patterns.txt
Rsx 'weapon settings' @('-nogui', '-export', '--loadwhitelist', 'wepn', '--exporttypes', 'wepn', '--exportdir', (Q "$Work\out"), (Q $common))
# the viewmodel: mdl/techart/mshop/weapons/class/smg/r99/r99_base_v.rmdl
Rsx 'viewmodel' @('-nogui', '-export', '-exportfullpaths', '--loadwhitelist', 'mdl_,arig,aseq,asqd,anir,matl,txtr,shds,shdr,msnp', '--exporttypes', 'mdl_', '--exportfilter', '0x8415FDF21FC5DE64', '--exportdir', (Q "$Work\mdl3"), (Q $common))
# the arms: mdl/Weapons/arms/pov_pilot_light_wraith_cyber_ninja.rmdl (the only first-person arms in common.rpak; the
# legends' default arms load on demand, odl_asset)
Rsx 'arms' @('-nogui', '-export', '-exportfullpaths', '--loadwhitelist', 'mdl_,arig,aseq,asqd,anir,matl,txtr,shds,shdr,msnp', '--exporttypes', 'mdl_', '--exportfilter', '0xF17804FB849712D4', '--exportdir', (Q "$Work\arms"), (Q $common))
# the anim rig's sequences: animseq/weapons/r97/ptpov_r97/* + animseq_derived/.../r99_base_v_animRig/*
foreach ($f in 'ptpov_r97', 'r99_base_v_animRig') {
  Rsx "sequences $f" @('-nogui', '-export', '-exportfullpaths', '--loadwhitelist', 'arig,aseq,asqd,anir,mdl_', '--exporttypes', 'aseq', '--exportfilter', $f, '--exportdir', (Q "$Work\seq"), (Q $common))
}
# material -> texture lists (slot order), then the textures from the install
Rsx 'material dependencies' @('-nogui', '--loadwhitelist', 'matl,txtr,shds,shdr', '--depfilepath', (Q "$Work\deps_mat.txt"), (Q $common))
$mats = @('material\art\mshop\weapons\class\smg\r99\base\r99_base_main_sknp.rpak',
  'material\art\mshop\characters\legends\wraith\legendary\v19_voidwalker\wraith_lgnd_v19_voidwalker_body_sknp.rpak',
  'material\art\mshop\characters\legends\wraith\legendary\v19_voidwalker\wraith_lgnd_v19_voidwalker_v_arms_sknp.rpak')
$tex = [ordered]@{}
foreach ($m in $mats) {
  $line = (Select-String -Path "$Work\deps_mat.txt" -SimpleMatch "$m,").Line
  if (-not $line) { throw "no dependency line for $m (a retail update renamed it?)" }
  $tex[$m] = @($line.Split(',') | Where-Object { $_ -like 'texture\0x*' } | ForEach-Object { $_.Substring(8) })
}
$tex | ConvertTo-Json | Set-Content "$Work\mat_textures.json"
$filter = ($tex.Values | ForEach-Object { $_ }) -join ','
Rsx 'textures (install)' @('-nogui', '-export', '--loadwhitelist', 'txtr', '--exporttypes', 'txtr', '--exportfilter', $filter, '--exportdir', (Q "$Work\tex"), (Q (Join-Path $Game 'paks\Win64\common.rpak')))

Write-Host '== bake' -ForegroundColor Cyan
& python (Join-Path $PSScriptRoot 'build_weapon.py')
