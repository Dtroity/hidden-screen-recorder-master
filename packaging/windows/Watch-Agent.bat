@echo off
REM Legacy entry — prefer Watch-Agent.vbs / Watch-Agent.ps1 from Task Scheduler.
REM If somehow still invoked as .bat, re-launch PowerShell hidden and exit.
setlocal EnableExtensions
cd /d "%~dp0"
if exist "%~dp0Watch-Agent.ps1" (
  start "" /B powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "%~dp0Watch-Agent.ps1"
  exit /b 0
)
REM Fallback inline (still may flash if Task Scheduler runs this .bat directly).
set "BIN=%CD%\bin"
if not exist "%BIN%\recorder-agent.exe" if exist "%CD%\recorder-agent.exe" set "BIN=%CD%"
if not exist "%BIN%\recorder-agent.exe" exit /b 0
if exist "%BIN%\agent.admin-stopped" exit /b 0
tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I "recorder-agent.exe" >nul
if not errorlevel 1 exit /b 0
if exist "%BIN%\agent.stop" del "%BIN%\agent.stop" >nul 2>&1
start "" /MIN "%BIN%\recorder-agent.exe"
exit /b 0
