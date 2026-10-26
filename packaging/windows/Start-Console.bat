@echo off
setlocal EnableExtensions
title Office Recorder Console
cd /d "%~dp0"
if exist "bin\hidden-screen-recorder.exe" (
  start "" "bin\hidden-screen-recorder.exe"
  exit /b 0
)
if exist "hidden-screen-recorder.exe" (
  start "" "hidden-screen-recorder.exe"
  exit /b 0
)
echo [ERROR] hidden-screen-recorder.exe not found.
echo Expected next to this file or in bin\
pause
exit /b 1
