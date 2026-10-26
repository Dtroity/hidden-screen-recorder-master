@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Show server URLs
cd /d "%~dp0"

set "EXE="
if exist "archive-server.exe" set "EXE=%CD%\archive-server.exe"
if exist "bin\archive-server.exe" set "EXE=%CD%\bin\archive-server.exe"
if "!EXE!"=="" (
  echo [ERROR] archive-server.exe not found.
  pause
  exit /b 1
)

set "CFG="
if exist "%CD%\data\server.toml" set "CFG=%CD%\data\server.toml"
if exist "%CD%\server.toml" set "CFG=%CD%\server.toml"
if "!CFG!"=="" (
  echo [ERROR] Config not found. Run Setup-Archive-Server.bat first.
  pause
  exit /b 1
)

echo URLs for browser / agent settings:
echo.
"!EXE!" --config "!CFG!" show-urls
echo.
pause
