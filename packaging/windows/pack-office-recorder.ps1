# Build Release binaries and assemble one install folder (no secrets).
# Layout:
#   OfficeRecorder\                 — agent + console (bin\ + agent BATs)
#   OfficeRecorder\ArchiveServer\   — archive server only (exe + server BATs)
#   OfficeRecorder\UbuntuVPS\       — Linux install helpers
$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $Root

Write-Host "Building release binaries..."
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
cargo build -p archive-server --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$Dist = Join-Path $Root "target\OfficeRecorder"
if (Test-Path $Dist) { Remove-Item -Recurse -Force $Dist }
$Bin = Join-Path $Dist "bin"
New-Item -ItemType Directory -Path $Bin | Out-Null

Copy-Item (Join-Path $Root "target\release\hidden-screen-recorder.exe") $Bin
Copy-Item (Join-Path $Root "target\release\recorder-agent.exe") $Bin
Copy-Item (Join-Path $Root "target\release\archive-server.exe") $Bin
Copy-Item (Join-Path $Root "LICENSE") $Dist
Copy-Item (Join-Path $Root "THIRD_PARTY_NOTICES.txt") $Dist -ErrorAction SilentlyContinue

# FFmpeg HEVC (libx265) next to agent/console
& (Join-Path $PSScriptRoot "Ensure-FFmpeg.ps1") | Out-Null
$Ffmpeg = Join-Path $Root "third_party\ffmpeg\bin\ffmpeg.exe"
if (-not (Test-Path $Ffmpeg)) { throw "ffmpeg.exe missing after Ensure-FFmpeg.ps1" }
Copy-Item -Force $Ffmpeg (Join-Path $Bin "ffmpeg.exe")
Get-ChildItem (Join-Path $Root "third_party\ffmpeg") -File -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -match '^(LICENSE|COPYING)' } |
  ForEach-Object { Copy-Item -Force $_.FullName $Bin }

# Agent / console at package root (no duplicate server BATs here).
Copy-Item (Join-Path $PSScriptRoot "Start-Console.bat") $Dist
Copy-Item (Join-Path $PSScriptRoot "Start-Agent.bat") $Dist
Copy-Item (Join-Path $PSScriptRoot "Stop-Agent.bat") $Dist
Copy-Item (Join-Path $PSScriptRoot "Watch-Agent.bat") $Dist
Copy-Item (Join-Path $PSScriptRoot "README-RU.txt") $Dist

# Server-only subfolder (single copy of server scripts).
$WinScripts = @(
  "Setup-Archive-Server.bat",
  "Start-Archive-Server.bat",
  "Start-Archive-Server-Background.bat",
  "Stop-Archive-Server.bat",
  "Show-Agent-Token.bat",
  "Show-Server-URLs.bat",
  "README-Archive-Server-RU.txt"
)
$Srv = Join-Path $Dist "ArchiveServer"
New-Item -ItemType Directory -Path $Srv | Out-Null
Copy-Item (Join-Path $Bin "archive-server.exe") $Srv
foreach ($f in $WinScripts) {
  Copy-Item (Join-Path $Root "server\windows\$f") $Srv
}

# Ubuntu VPS helpers (binary built on Linux).
$Lin = Join-Path $Dist "UbuntuVPS"
New-Item -ItemType Directory -Path $Lin | Out-Null
Copy-Item (Join-Path $Root "server\linux\README-Ubuntu-RU.txt") $Lin
Copy-Item (Join-Path $Root "server\linux\install-ubuntu.sh") $Lin
Copy-Item (Join-Path $Root "server\linux\hsr-archive.service") $Lin

Write-Host ""
Write-Host "Package ready (no secrets):"
Write-Host "  $Dist"
Write-Host "  Agent:   Start/Stop/Watch-Agent.bat, Start-Console.bat, bin\"
Write-Host "  Server:  ArchiveServer\"
Write-Host "  Ubuntu:  UbuntuVPS\"
Get-ChildItem $Dist -Recurse -File | Select-Object FullName, Length | Format-Table -AutoSize
