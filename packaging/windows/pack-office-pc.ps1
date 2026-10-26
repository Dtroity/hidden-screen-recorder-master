# Slim package for office PCs: console + agent only (no archive-server, no Ubuntu docs).
$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $Root

Write-Host "Building release binaries (agent + console)..."
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Dist = Join-Path $Root "target\OfficePC"
if (Test-Path $Dist) { Remove-Item -Recurse -Force $Dist }
$Bin = Join-Path $Dist "bin"
New-Item -ItemType Directory -Path $Bin | Out-Null

Copy-Item (Join-Path $Root "target\release\hidden-screen-recorder.exe") $Bin
Copy-Item (Join-Path $Root "target\release\recorder-agent.exe") $Bin
Copy-Item (Join-Path $Root "LICENSE") $Dist -ErrorAction SilentlyContinue

# FFmpeg HEVC (libx265) - required for default encode backend
& (Join-Path $PSScriptRoot "Ensure-FFmpeg.ps1") | Out-Null
$Ffmpeg = Join-Path $Root "third_party\ffmpeg\bin\ffmpeg.exe"
if (-not (Test-Path $Ffmpeg)) { throw "ffmpeg.exe missing after Ensure-FFmpeg.ps1" }
Copy-Item -Force $Ffmpeg (Join-Path $Bin "ffmpeg.exe")
Get-ChildItem (Join-Path $Root "third_party\ffmpeg") -File -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -match '^(LICENSE|COPYING)' } |
  ForEach-Object { Copy-Item -Force $_.FullName $Bin }

Copy-Item (Join-Path $PSScriptRoot "Start-Console.bat") $Dist
Copy-Item (Join-Path $PSScriptRoot "Start-Agent.bat") $Dist
Copy-Item (Join-Path $PSScriptRoot "Stop-Agent.bat") $Dist
Copy-Item (Join-Path $PSScriptRoot "Watch-Agent.bat") $Dist

@"
Office Recorder — комплект для офисного ПК
==========================================
Только консоль и агент (без сервера архива и вьюера).

  Start-Console.bat   — панель управления (настройки + старт/стоп агента)
  Start-Agent.bat     — запуск агента + watchdog
  Stop-Agent.bat      — остановка админом
  Watch-Agent.bat     — служебный (планировщик)
  bin\
    hidden-screen-recorder.exe
    recorder-agent.exe
    ffmpeg.exe          — HEVC (libx265), обязателен

Настройка архива: вкладка «Архив» в консоли (URL VPS + токен).
Кодирование: FFmpeg HEVC (по умолчанию). Не удаляйте ffmpeg.exe из bin\.
"@ | Set-Content -Path (Join-Path $Dist "README-RU.txt") -Encoding UTF8

$size = (Get-ChildItem $Dist -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ""
Write-Host "Office PC package ready:"
Write-Host "  $Dist"
Write-Host ("  Size: {0:N1} MB" -f ($size / 1MB))
Get-ChildItem $Dist -Recurse -File | Select-Object FullName, Length | Format-Table -AutoSize
