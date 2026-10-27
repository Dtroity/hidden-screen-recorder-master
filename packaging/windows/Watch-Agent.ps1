# Silent watchdog for OfficePC agent (no console window).
# Invoked by Task Scheduler via: powershell -WindowStyle Hidden -File Watch-Agent.ps1
$ErrorActionPreference = 'SilentlyContinue'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$bin = Join-Path $root 'bin'
if (-not (Test-Path -LiteralPath (Join-Path $bin 'recorder-agent.exe'))) {
    $bin = $root
}
$exe = Join-Path $bin 'recorder-agent.exe'
if (-not (Test-Path -LiteralPath $exe)) { exit 0 }

# Admin stop — do not revive until Start-Agent / console Start.
if (Test-Path -LiteralPath (Join-Path $bin 'agent.admin-stopped')) { exit 0 }

if (Get-Process -Name 'recorder-agent' -ErrorAction SilentlyContinue) { exit 0 }

Remove-Item -LiteralPath (Join-Path $bin 'agent.stop') -Force -ErrorAction SilentlyContinue
Start-Process -FilePath $exe -WindowStyle Hidden | Out-Null
exit 0
