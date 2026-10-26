@echo off
setlocal EnableExtensions
title Office Recorder — тест отправки
cd /d "%~dp0"

echo === Настройки ===
if exist "%CD%\settings.json" (
  echo Файл: %CD%\settings.json
) else (
  echo [!] Нет settings.json
  echo Скопируйте settings.example.json в settings.json и вставьте токен агента.
  pause
  exit /b 1
)

echo.
echo === Журнал отправки (хвост) ===
if exist "%CD%\archive_upload.log" (
  powershell -NoProfile -Command "Get-Content -LiteralPath '%CD%\archive_upload.log' -Tail 30"
) else (
  echo Пока нет archive_upload.log — запустите агент хотя бы раз после настройки.
)

echo.
echo === Очередь ===
if exist "%CD%\archive_upload_queue.json" (
  type "%CD%\archive_upload_queue.json"
) else (
  echo Очередь пуста / файла нет.
)

echo.
echo Откройте Start-Console.bat → Архив → Проверить подключение.
echo После записи смотрите archive_upload.log в этой папке.
pause
