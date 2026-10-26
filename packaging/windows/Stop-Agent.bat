@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Stop Office Recorder Agent
cd /d "%~dp0"

set "BIN="
if exist "%CD%\bin\recorder-agent.exe" set "BIN=%CD%\bin"
if "!BIN!"=="" if exist "%CD%\recorder-agent.exe" set "BIN=%CD%"
if "!BIN!"=="" (
  echo [WARN] recorder-agent.exe not found next to this script.
  echo Still force-killing any running recorder-agent.exe ...
  goto force
)

REM Legitimate admin stop — watchdog must not revive.
echo 1>"!BIN!\agent.admin-stopped"
echo 1>"!BIN!\agent.stop"

tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I /C:"recorder-agent.exe" >nul
if errorlevel 1 (
  echo recorder-agent.exe is not running.
  if exist "!BIN!\agent.stop" del "!BIN!\agent.stop" >nul 2>&1
  echo Marked admin-stopped. Use Start-Agent.bat or the console to run again.
  pause
  exit /b 0
)

echo Stopping agent gracefully...
set /a WAIT=0
:wait_loop
tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I /C:"recorder-agent.exe" >nul
if errorlevel 1 goto done
set /a WAIT+=1
if !WAIT! GEQ 6 goto force
timeout /t 1 /nobreak >nul
goto wait_loop

:force
echo Force-stopping recorder-agent.exe ...
taskkill /IM recorder-agent.exe /F /T >nul 2>&1
timeout /t 1 /nobreak >nul

:done
if defined BIN if exist "!BIN!\agent.stop" del "!BIN!\agent.stop" >nul 2>&1
tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I /C:"recorder-agent.exe" >nul
if errorlevel 1 (
  echo Agent stopped. Watchdog will NOT restart it until Start-Agent.bat / console Start.
) else (
  echo [ERROR] Process still present. Close recorder-agent.exe in Task Manager.
)
echo.
pause
