<#
  bundle_tools.ps1 - builds dist\r5rust-tools-<n>.zip: everything tools\install.ps1 needs on a PC with no dev tools.
  The launcher extracts it into the game folder (next to play.ps1), so every path below is tools\bin\...:

    tools\bin\pwsh\        portable PowerShell 7 (the launcher runs tools\bin\pwsh\pwsh.exe -File tools\install.ps1)
    tools\bin\python\      Python 3.11 embeddable + numpy + Pillow (install.ps1 puts it first on PATH)
    tools\bin\rsx\         rsx.exe (R5Flowstate's RSX fork, our local build) + the VC++ runtime DLLs it imports + libzstd.dll
    tools\bin\dxc\         dxc.exe + dxcompiler.dll + dxil.dll (DirectXShaderCompiler v1.8.2502 = the SDK's 1.8.2502.11)
    tools\bin\extractor\   r5rust.py (paths come from the config install.ps1 writes, never its config.retail.json)
    tools\bin\export-map\  vcruntime140.dll only: the game zip brings export-map.exe itself, which imports it
    tools\bin\BUNDLE.txt   what's in here and where it came from

  Downloads are cached in -Cache (rerun = no network) and checked against the sha256 pins below. Nothing is
  installed anywhere; pip only downloads wheels, which are unzipped into the embeddable Python.

    pwsh tools\bundle_tools.ps1 [-Version 1] [-Cache <dir>] [-Rsx <rsx.exe>] [-Extractor <dir>]
#>
param(
  [int]$Version = 1,
  [string]$Cache = (Join-Path ([IO.Path]::GetTempPath()) 'r5rust-bundle'),
  [string]$Rsx = 'D:\R5Rust\tools\rsx-fs\bin\Release\rsx.exe',
  [string]$Extractor = 'D:\R5Rust\extractor',
  # a Python with pip, only to download the wheels (any 3.x; the wheels are picked for 3.11 / win_amd64)
  [string]$HostPython = 'python'
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot

# url, sha256 (pinned 2026-11-21)
$Downloads = [ordered]@{
  pwsh   = @{ url = 'https://github.com/PowerShell/PowerShell/releases/download/v7.6.10/PowerShell-7.6.10-win-x64.zip'
              sha = '7d871531806eb1646264e4bbabd9f0c30c23355591bc756bb9909448c29ae166' }
  python = @{ url = 'https://www.python.org/ftp/python/3.11.9/python-3.11.9-embed-amd64.zip'
              sha = '009d6bf7e3b2ddca3d784fa09f90fe54336d5b60f0e0f305c37f400bf83cfd3b' }
  dxc    = @{ url = 'https://github.com/microsoft/DirectXShaderCompiler/releases/download/v1.8.2502/dxc_2025_02_20.zip'
              sha = '70b1913a1bfce4a3e1a5311d16246f4ecdf3a3e613abec8aa529e57668426f85' }
  # RSX loads a zstd decoder at runtime (zstd_loader.cpp: libzstd.dll next to rsx.exe, else on PATH): retail models'
  # vertex data (.vg) is zstd. Dev PCs found Git's mingw64\bin\libzstd.dll on PATH; without one the .vg exports stay
  # compressed and export-map drops ~70 prop models.
  zstd   = @{ url = 'https://github.com/facebook/zstd/releases/download/v1.5.7/zstd-v1.5.7-win64.zip'
              sha = 'acb4e8111511749dc7a3ebedca9b04190e37a17afeb73f55d4425dbf0b90fad9' }
}
# the same versions the dev extractor venv runs (3.11.9)
$Wheels = @('numpy==2.4.6', 'pillow==13.0.0')

function Fetch([string]$name) {
  $d = $Downloads[$name]
  $file = Join-Path $Cache ([IO.Path]::GetFileName($d.url))
  if (-not (Test-Path $file)) {
    Write-Host "  downloading $($d.url)"
    $tmp = "$file.part"
    Invoke-WebRequest -Uri $d.url -OutFile $tmp -UseBasicParsing
    Move-Item $tmp $file -Force
  }
  $sha = (Get-FileHash $file -Algorithm SHA256).Hash.ToLower()
  if ($d.sha -and $sha -ne $d.sha) { throw "$file has sha256 $sha, expected $($d.sha): delete it and run again" }
  if (-not $d.sha) { Write-Host "  $name sha256 $sha (not pinned)" -ForegroundColor Yellow }
  $file
}

function Expand-To([string]$zip, [string]$dir) {
  New-Item -ItemType Directory -Force $dir | Out-Null
  [IO.Compression.ZipFile]::ExtractToDirectory($zip, $dir, $true)
}

# app-local VC++ runtime: the newest Visual Studio redist on this machine (RSX was built with it)
function Find-Redist {
  $r = Get-ChildItem 'C:\Program Files\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT' -Directory -ErrorAction SilentlyContinue |
    Sort-Object { [version]$_.Parent.Parent.Name } -Descending | Select-Object -First 1
  if (-not $r) { throw 'no Visual Studio VC++ redist found (VC\Redist\MSVC\<ver>\x64\Microsoft.VC*.CRT)' }
  $r
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
New-Item -ItemType Directory -Force $Cache | Out-Null
$stage = Join-Path $Cache "stage-$Version"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
$bin = Join-Path $stage 'tools\bin'
$sw = [Diagnostics.Stopwatch]::StartNew()

Write-Host '== PowerShell'
Expand-To (Fetch 'pwsh') (Join-Path $bin 'pwsh')

Write-Host '== Python 3.11 embeddable + wheels'
$pyDir = Join-Path $bin 'python'
Expand-To (Fetch 'python') $pyDir
$wheelDir = Join-Path $Cache 'wheels'
New-Item -ItemType Directory -Force $wheelDir | Out-Null
& $HostPython -m pip download --disable-pip-version-check --only-binary=:all: --platform win_amd64 --python-version 3.11 `
  --implementation cp --abi cp311 -d $wheelDir @Wheels | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'pip download failed' }
$site = Join-Path $pyDir 'Lib\site-packages'
foreach ($w in $Wheels) {
  $n, $v = $w -split '=='
  $whl = Get-ChildItem $wheelDir -Filter "$n-$v-cp311-cp311-win_amd64.whl" | Select-Object -First 1
  if (-not $whl) { throw "wheel $w (cp311 win_amd64) wasn't downloaded" }
  Expand-To $whl.FullName $site
}
# the ._pth file turns the embeddable's path lookup off (no PYTHONPATH, no registry): list the stdlib zip,
# site-packages, and `import site` so .pth files there work
$pth = Get-ChildItem $pyDir -Filter 'python3*._pth' | Select-Object -First 1
Set-Content $pth.FullName @('python311.zip', '.', 'Lib\site-packages', 'import site') -Encoding ascii
# an embeddable Python with a ._pth leaves the script's folder off sys.path: put it back (tools\*.py import siblings)
Set-Content (Join-Path $site 'sitecustomize.py') @(
  '# r5rust bundle: the embeddable Python (._pth mode) does not put the script''s folder on sys.path; do it here',
  'import os, sys',
  'if sys.argv and sys.argv[0] and sys.argv[0] not in ("-c", "-m") and os.path.isfile(sys.argv[0]):',
  '    sys.path.insert(0, os.path.dirname(os.path.abspath(sys.argv[0])))'
) -Encoding ascii

Write-Host '== RSX (R5Flowstate fork) + VC++ runtime'
if (-not (Test-Path $Rsx)) { throw "RSX not found: $Rsx" }
$rsxDir = Join-Path $bin 'rsx'
New-Item -ItemType Directory -Force $rsxDir | Out-Null
Copy-Item $Rsx $rsxDir
$crt = Find-Redist
$omp = Join-Path $crt.Parent.FullName ($crt.Name -replace '\.CRT$', '.OpenMP')
# rsx.exe imports MSVCP140, VCRUNTIME140, VCRUNTIME140_1 and VCOMP140 (dumpbin /dependents); the api-ms-win-crt-* are
# the Universal CRT, part of Windows 10/11
foreach ($f in 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll') { Copy-Item (Join-Path $crt.FullName $f) $rsxDir }
Copy-Item (Join-Path $omp 'vcomp140.dll') $rsxDir
# the zstd decoder RSX loads by name (imports only KERNEL32 + msvcrt)
$zstdZip = Join-Path $Cache 'zstd'
Expand-To (Fetch 'zstd') $zstdZip
$zdll = Get-ChildItem $zstdZip -Recurse -Filter 'libzstd.dll' | Where-Object { $_.Directory.Name -eq 'dll' } | Select-Object -First 1
if (-not $zdll) { throw 'libzstd.dll not in the zstd release zip' }
Copy-Item $zdll.FullName $rsxDir
# export-map.exe (Rust, MSVC) imports VCRUNTIME140 only
$emDir = Join-Path $bin 'export-map'
New-Item -ItemType Directory -Force $emDir | Out-Null
Copy-Item (Join-Path $crt.FullName 'vcruntime140.dll') $emDir

Write-Host '== dxc'
$dxcZip = Join-Path $Cache 'dxc'
Expand-To (Fetch 'dxc') $dxcZip
$dxcDir = Join-Path $bin 'dxc'
New-Item -ItemType Directory -Force $dxcDir | Out-Null
foreach ($f in 'dxc.exe', 'dxcompiler.dll', 'dxil.dll') { Copy-Item (Join-Path $dxcZip "bin\x64\$f") $dxcDir }
Get-ChildItem $dxcZip -Filter 'LICENSE*' | Copy-Item -Destination $dxcDir

Write-Host '== extractor'
$exDir = Join-Path $bin 'extractor'
New-Item -ItemType Directory -Force $exDir | Out-Null
Copy-Item (Join-Path $Extractor 'r5rust.py') $exDir

$pwshVer = (Get-Item (Join-Path $bin 'pwsh\pwsh.exe')).VersionInfo.ProductVersion
$dxcVer = (Get-Item (Join-Path $dxcDir 'dxc.exe')).VersionInfo.FileVersion
@(
  "r5rust tools bundle $Version, built $((Get-Date).ToString('yyyy-MM-dd HH:mm'))",
  '',
  "pwsh\        PowerShell $pwshVer  ($($Downloads.pwsh.url))",
  "python\      Python 3.11.9 embeddable ($($Downloads.python.url)) + $($Wheels -join ', ')",
  "rsx\         R5Flowstate's RSX fork (github.com/R5Flowstate/rsx, AGPL-3.0), local build; rsx.exe sha256 $((Get-FileHash (Join-Path $rsxDir 'rsx.exe')).Hash.ToLower())",
  "             + Microsoft VC++ runtime $($crt.Parent.Parent.Name) (app-local redist) + libzstd.dll (zstd 1.5.7, $($Downloads.zstd.url))",
  "dxc\         DirectXShaderCompiler $dxcVer ($($Downloads.dxc.url))",
  "extractor\   r5rust.py (R5Rust's pak extractor)",
  "export-map\  vcruntime140.dll for export-map.exe (the exe comes with the game)"
) | Set-Content (Join-Path $bin 'BUNDLE.txt') -Encoding utf8

$dist = Join-Path $repo 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null
$zip = Join-Path $dist "r5rust-tools-$Version.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
[IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip, [IO.Compression.CompressionLevel]::Optimal, $false)
$files = Get-ChildItem $stage -Recurse -File
Write-Host ("`n{0}: {1:n1} MB ({2} files, {3:n1} MB unpacked) in {4:n0} s" -f $zip, ((Get-Item $zip).Length / 1MB), $files.Count,
  (($files | Measure-Object Length -Sum).Sum / 1MB), $sw.Elapsed.TotalSeconds) -ForegroundColor Green
