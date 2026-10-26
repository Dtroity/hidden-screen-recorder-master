@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Stop Office Recorder Agent
cd /d "%~dp0"

set "BIN=%CD%\bin"
if not exist "%BIN%\recorder-agent.exe" if exist "%CD%\recorder-agent.exe" set "BIN=%CD%"

REM Mark intentional admin stop — watchdog must NOT restart the agent.
echo. > "%BIN%\agent.admin-stopped"

tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I "recorder-agent.exe" >nul
if errorlevel 1 (
  echo recorder-agent.exe is not running.
  if exist "%BIN%\agent.stop" del "%BIN%\agent.stop" >nul 2>&1
  echo Marked as admin-stopped (watchdog will not revive it).
  echo Use Start-Agent.bat to run again.
  pause
  exit /b 0
)

echo Requesting graceful stop...
echo. > "%BIN%\agent.stop"

set /a WAIT=0
:wait_loop
tasklist /FI "IMAGENAME eq recorder-agent.exe" 2>nul | findstr /I "recorder-agent.exe" >nul
if errorlevel 1 goto done
set /a WAIT+=1
if !WAIT! GEQ 30 goto force
timeout /t 1 /nobreak >nul
goto wait_loop

:force
echo Graceful stop timed out; forcing...
taskkill /IM recorder-agent.exe /F >nul 2>&1

:done
if exist "%BIN%\agent.stop" del "%BIN%\agent.stop" >nul 2>&1
echo Agent stopped by admin. Watchdog will NOT restart it until Start-Agent.bat.
echo.
pause
