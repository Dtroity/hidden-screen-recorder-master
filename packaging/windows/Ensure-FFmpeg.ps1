# Download BtbN FFmpeg GPL (libx265) into third_party/ffmpeg/bin/ffmpeg.exe if missing.
$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Vendor = Join-Path $Root "third_party\ffmpeg"
$Bin = Join-Path $Vendor "bin"
$Exe = Join-Path $Bin "ffmpeg.exe"
$Marker = Join-Path $Vendor "SOURCE.txt"

function Test-Libx265([string]$Path) {
  $out = & $Path -hide_banner -encoders 2>&1 | Out-String
  return ($out -match "libx265")
}

if ((Test-Path $Exe) -and (Test-Libx265 $Exe)) {
  Write-Host "FFmpeg OK (libx265): $Exe"
  Write-Output $Exe
  exit 0
}

if (Test-Path $Exe) {
  Write-Host "Existing ffmpeg.exe lacks libx265 - re-downloading..."
  Remove-Item -Force $Exe
}

New-Item -ItemType Directory -Force -Path $Bin | Out-Null
$ZipName = "ffmpeg-n9.0-latest-win64-gpl-9.0.zip"
$Url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$ZipName"
$ZipPath = Join-Path $env:TEMP $ZipName
$Extract = Join-Path $env:TEMP ("ffmpeg-extract-" + [guid]::NewGuid().ToString("n"))

Write-Host "Downloading $Url ..."
Write-Host "(~185 MB - GPL static build with libx265)"
Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing

Write-Host "Extracting..."
New-Item -ItemType Directory -Force -Path $Extract | Out-Null
Expand-Archive -LiteralPath $ZipPath -DestinationPath $Extract -Force

$found = Get-ChildItem -Path $Extract -Recurse -Filter "ffmpeg.exe" |
  Where-Object { $_.DirectoryName -match '[\\/]bin$' } |
  Select-Object -First 1
if (-not $found) {
  $found = Get-ChildItem -Path $Extract -Recurse -Filter "ffmpeg.exe" | Select-Object -First 1
}
if (-not $found) { throw "ffmpeg.exe not found inside archive" }

Copy-Item -Force $found.FullName $Exe

Get-ChildItem -Path $Extract -Recurse -Include "LICENSE","COPYING*","LICENSE.txt" -File -ErrorAction SilentlyContinue |
  Select-Object -First 5 |
  ForEach-Object { Copy-Item -Force $_.FullName (Join-Path $Vendor $_.Name) -ErrorAction SilentlyContinue }

$stamp = Get-Date -Format o
@(
  "Source: $Url"
  "Fetched: $stamp"
  "File: bin\ffmpeg.exe (GPL static, includes libx265)"
) | Set-Content -Path $Marker -Encoding ASCII

try { if (Test-Path -LiteralPath $Extract) { Remove-Item -LiteralPath $Extract -Recurse -Force } } catch { }
try { if (Test-Path -LiteralPath $ZipPath) { Remove-Item -LiteralPath $ZipPath -Force } } catch { }

if (-not (Test-Libx265 $Exe)) {
  throw "Downloaded ffmpeg.exe does not report libx265"
}
Write-Host "Installed: $Exe"
Write-Output $Exe
