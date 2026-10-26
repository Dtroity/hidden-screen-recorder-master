@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Archive Server Setup
cd /d "%~dp0"

set "EXE="
if exist "archive-server.exe" set "EXE=%CD%\archive-server.exe"
if exist "bin\archive-server.exe" set "EXE=%CD%\bin\archive-server.exe"
if "!EXE!"=="" (
  echo [ERROR] archive-server.exe not found.
  echo Put it next to this BAT or in bin\
  pause
  exit /b 1
)

set "CFG=%CD%\server.toml"
if exist "bin\archive-server.exe" (
  if not exist "%CD%\data" mkdir "%CD%\data"
  set "CFG=%CD%\data\server.toml"
)

echo Starting interactive setup...
echo Config will be: !CFG!
echo.
"!EXE!" --config "!CFG!" setup
set "ERR=!ERRORLEVEL!"
echo.
if not "!ERR!"=="0" echo [ERROR] Setup failed with code !ERR!.
pause
exit /b !ERR!
