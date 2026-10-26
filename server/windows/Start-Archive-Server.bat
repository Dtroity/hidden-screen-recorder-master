@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Archive Server
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
  echo [ERROR] Config not found: server.toml / data\server.toml
  echo Run Setup-Archive-Server.bat once first.
  pause
  exit /b 1
)

echo Starting archive-server...
echo EXE:    !EXE!
echo Config: !CFG!
echo Browser: http://127.0.0.1:8787/
echo Keep this window open while the server runs.
echo ============================================================
echo.

"!EXE!" --config "!CFG!" run
set "ERR=!ERRORLEVEL!"

echo.
echo ============================================================
if "!ERR!"=="0" (
  echo Server stopped normally.
) else (
  echo [ERROR] Server exited with code !ERR!.
  echo Common causes: port in use, bad archive path, broken config.
)
echo.
pause
exit /b !ERR!
