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
Copy-Item (Join-Path $PSScriptRoot "settings.example.json") (Join-Path $Dist "settings.example.json")
Copy-Item (Join-Path $PSScriptRoot "Test-Archive-Upload.bat") $Dist
# Starter settings.json (user must paste agent token before Start-Agent).
if (-not (Test-Path (Join-Path $Dist "settings.json"))) {
  Copy-Item (Join-Path $PSScriptRoot "settings.example.json") (Join-Path $Dist "settings.json")
}

@"
Office Recorder — автономный комплект для офисного ПК
====================================================
Копируйте эту папку целиком. Настройки только здесь: settings.json (не AppData).

  Start-Console.bat        — панель (Архив: URL + токен)
  Start-Agent.bat          — агент + автозапуск после reboot
  Stop-Agent.bat           — стоп админом
  Test-Archive-Upload.bat  — показать журнал/очередь отправки
  settings.json            — ОБЯЗАТЕЛЬНО вставьте archive_token
  archive_upload.log       — появится после старта агента (диагностика)
  bin\ffmpeg.exe           — не удалять

Первый запуск:
  1) Откройте settings.json → archive_token = токен агента с VPS
     (sudo -u hsr /opt/hsr/archive-server --config /etc/hsr/server.toml show-agent-token)
  2) Start-Console.bat → Архив → «Проверить подключение» (должно быть OK)
  3) Start-Agent.bat
  4) Подождите ~15 мин (нарезка сегментов) ИЛИ Stop/Start запись —
     в archive_upload.log должны появиться строки «upload ok»
  5) На сервере в панели — новые файлы

Важно: запись режется каждые 15 минут, иначе один бесконечный файл
уходит на сервер только после стопа/сна.
"@ | Set-Content -Path (Join-Path $Dist "README-RU.txt") -Encoding UTF8

$size = (Get-ChildItem $Dist -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ""
Write-Host "Office PC package ready:"
Write-Host "  $Dist"
Write-Host ("  Size: {0:N1} MB" -f ($size / 1MB))
Get-ChildItem $Dist -Recurse -File | Select-Object FullName, Length | Format-Table -AutoSize
