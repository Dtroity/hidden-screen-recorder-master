@echo off
setlocal EnableExtensions EnableDelayedExpansion
title Archive Server (background)
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

for %%I in ("!CFG!") do set "CFGDIR=%%~dpI"
set "PIDFILE=!CFGDIR!archive-server.pid"
set "LOGOUT=!CFGDIR!archive-server.out.log"
set "LOGERR=!CFGDIR!archive-server.err.log"

if exist "!PIDFILE!" (
  set /p OLDPID=<"!PIDFILE!"
  tasklist /FI "PID eq !OLDPID!" 2>nul | findstr /I "archive-server.exe" >nul
  if not errorlevel 1 (
    echo Server already running PID=!OLDPID!
    echo Browser: http://127.0.0.1:8787/
    echo Stop: Stop-Archive-Server.bat
    pause
    exit /b 0
  )
)

echo Starting archive-server in background (hidden window)...
set "HSR_EXE=!EXE!"
set "HSR_CFG=!CFG!"
set "HSR_PID=!PIDFILE!"
set "HSR_OUT=!LOGOUT!"
set "HSR_ERR=!LOGERR!"
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$exe=$env:HSR_EXE; $cfg=$env:HSR_CFG; $pidFile=$env:HSR_PID; $out=$env:HSR_OUT; $err=$env:HSR_ERR;" ^
  "$p=Start-Process -FilePath $exe -ArgumentList @('--config',$cfg,'run') -WindowStyle Hidden -RedirectStandardOutput $out -RedirectStandardError $err -PassThru;" ^
  "if($null -eq $p){ exit 1 }; Set-Content -LiteralPath $pidFile -Value $p.Id -Encoding ASCII"
if errorlevel 1 (
  echo [ERROR] Failed to start.
  pause
  exit /b 1
)

timeout /t 1 /nobreak >nul
if exist "!PIDFILE!" (
  set /p NEWPID=<"!PIDFILE!"
  tasklist /FI "PID eq !NEWPID!" 2>nul | findstr /I "archive-server.exe" >nul
  if errorlevel 1 (
    echo [ERROR] Process exited immediately. See:
    echo   !LOGERR!
    echo   !LOGOUT!
    pause
    exit /b 1
  )
  echo Started PID=!NEWPID!
) else (
  echo [ERROR] PID file was not created.
  pause
  exit /b 1
)
echo Logs: !LOGOUT!  /  !LOGERR!
echo Browser: http://127.0.0.1:8787/
echo Stop with: Stop-Archive-Server.bat
echo.
pause
