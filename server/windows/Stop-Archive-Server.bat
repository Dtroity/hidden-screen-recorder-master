@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Stop Archive Server
cd /d "%~dp0"

set "CFGDIR=%CD%\"
if exist "%CD%\data\server.toml" set "CFGDIR=%CD%\data\"
if exist "%CD%\server.toml" set "CFGDIR=%CD%\"
set "PIDFILE=!CFGDIR!archive-server.pid"

if exist "!PIDFILE!" (
  set /p PID=<"!PIDFILE!"
  echo Stopping PID=!PID! ...
  taskkill /PID !PID! /F >nul 2>&1
  del "!PIDFILE!" >nul 2>&1
  echo Stopped.
) else (
  echo No PID file. Trying by process name...
  taskkill /IM archive-server.exe /F >nul 2>&1
  if errorlevel 1 (
    echo No running archive-server.exe found.
  ) else (
    echo Stopped archive-server.exe
  )
)
echo.
pause
