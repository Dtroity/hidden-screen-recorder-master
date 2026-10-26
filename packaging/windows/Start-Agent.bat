@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Office Recorder Agent
cd /d "%~dp0"

set "BIN="
if exist "%CD%\bin\recorder-agent.exe" set "BIN=%CD%\bin"
if "!BIN!"=="" if exist "%CD%\recorder-agent.exe" set "BIN=%CD%"
if "!BIN!"=="" (
  echo [ERROR] recorder-agent.exe not found.
  echo Expected in bin\ or next to this file.
  pause
  exit /b 1
)

REM Clear admin-stop so the agent and watchdog may run again.
if exist "!BIN!\agent.admin-stopped" del "!BIN!\agent.admin-stopped" >nul 2>&1
if exist "!BIN!\agent.stop" del "!BIN!\agent.stop" >nul 2>&1

tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I /C:"recorder-agent.exe" >nul
if not errorlevel 1 (
  echo Agent already running.
  goto install_watchdog
)

start "" "!BIN!\recorder-agent.exe"
timeout /t 1 /nobreak >nul
tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I /C:"recorder-agent.exe" >nul
if errorlevel 1 (
  echo [ERROR] Agent exited immediately.
  echo If agent.admin-stopped exists in bin\, delete it and retry.
  pause
  exit /b 1
)
echo Agent started.

:install_watchdog
set "WATCH=%CD%\Watch-Agent.bat"
if not exist "!WATCH!" set "WATCH=%CD%\packaging\windows\Watch-Agent.bat"
if exist "!WATCH!" (
  schtasks /Create /TN "OfficePC-RecorderAgentWatchdog" /SC MINUTE /MO 2 /TR "\"!WATCH!\"" /RU "%USERNAME%" /F >nul 2>&1
  if errorlevel 1 (
    echo [WARN] Could not register watchdog task. Agent still started.
  ) else (
    echo Watchdog installed: restarts after crashes ^(not after Stop-Agent^).
  )
)
echo.
pause
