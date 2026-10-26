@echo off
setlocal EnableExtensions
title Office Recorder Agent
cd /d "%~dp0"

set "BIN=%CD%\bin"
if not exist "%BIN%\recorder-agent.exe" if exist "%CD%\recorder-agent.exe" set "BIN=%CD%"
if not exist "%BIN%\recorder-agent.exe" (
  echo [ERROR] recorder-agent.exe not found.
  echo Expected next to this file or in bin\
  pause
  exit /b 1
)

REM Clear admin-stop so watchdog may restart after crashes again.
if exist "%BIN%\agent.admin-stopped" del "%BIN%\agent.admin-stopped" >nul 2>&1
if exist "%BIN%\agent.stop" del "%BIN%\agent.stop" >nul 2>&1

tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I "recorder-agent.exe" >nul
if not errorlevel 1 (
  echo Agent already running.
  goto install_watchdog
)

start "" "%BIN%\recorder-agent.exe"
echo Agent started.

:install_watchdog
REM Crash auto-restart every 2 minutes (skipped when agent.admin-stopped exists).
set "WATCH=%CD%\Watch-Agent.bat"
if not exist "%WATCH%" set "WATCH=%CD%\packaging\windows\Watch-Agent.bat"
if exist "%WATCH%" (
  schtasks /Create /TN "OfficePC-RecorderAgentWatchdog" /SC MINUTE /MO 2 /TR "\"%WATCH%\"" /RU "%USERNAME%" /F >nul 2>&1
  if errorlevel 1 (
    echo [WARN] Could not register watchdog scheduled task. Agent still started.
  ) else (
    echo Watchdog task installed: restarts agent after crashes (not after Stop-Agent.bat).
  )
)
echo.
pause
