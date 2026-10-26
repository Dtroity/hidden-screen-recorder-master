@echo off
REM Watchdog: restart recorder-agent.exe unless admin stopped it via Stop-Agent.bat
setlocal EnableExtensions
cd /d "%~dp0"

set "BIN=%CD%\bin"
if not exist "%BIN%\recorder-agent.exe" if exist "%CD%\recorder-agent.exe" set "BIN=%CD%"
if not exist "%BIN%\recorder-agent.exe" exit /b 0

REM Legitimate admin stop — do not revive until Start-Agent.bat
if exist "%BIN%\agent.admin-stopped" exit /b 0

tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I "recorder-agent.exe" >nul
if not errorlevel 1 exit /b 0

REM Clear stale graceful-stop flag from a crash path
if exist "%BIN%\agent.stop" del "%BIN%\agent.stop" >nul 2>&1

start "" /MIN "%BIN%\recorder-agent.exe"
exit /b 0
