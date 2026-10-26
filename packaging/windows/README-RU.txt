Office Recorder — комплект для Windows
======================================

Источники в репозитории
-----------------------
  packaging\windows\   — агент (Start/Stop/Watch), Start-Console, pack-*.ps1
  server\windows\      — BAT сервера архива (копируются в ArchiveServer\)
  server\linux\        — Ubuntu VPS (systemd)
  docs\STRUCTURE.ru.md — карта репозитория


Структура папки (после pack-office-recorder.ps1)
------------------------------------------------
  Start-Console.bat              Консоль настроек / ручная запись
  Start-Agent.bat                Агент без трея + watchdog
  Stop-Agent.bat                 Остановка админом (без автоперезапуска)
  Watch-Agent.bat                Служебный watchdog для планировщика
  README-RU.txt                  Этот файл
  LICENSE / THIRD_PARTY_NOTICES.txt
  bin\
    hidden-screen-recorder.exe
    recorder-agent.exe
    archive-server.exe           (копия также в ArchiveServer\)
    ffmpeg.exe                   (необязательно)
  ArchiveServer\                 Сервер архива (Setup/Start/Stop + README)
  UbuntuVPS\                     Установка на Ubuntu


Администратор (офисный ПК / VPS)
--------------------------------
  1. Откройте папку ArchiveServer\
  2. Setup-Archive-Server.bat — каталог архива, пароль, порт
  3. Start-Archive-Server.bat или Start-Archive-Server-Background.bat
  4. Браузер: http://127.0.0.1:8787/ или http://IP:8787/
     Просмотр по IP поддерживается; для HEVC нужен кодек в браузере
     или скачивание в VLC.
  5. Show-Agent-Token.bat — токен для рабочих ПК
  6. Ubuntu: папка UbuntuVPS\


Рабочий ПК
----------
  1. Start-Console.bat — папка записи, URL архива + токен, «Проверить подключение»
  2. Start-Agent.bat — фоновый агент + watchdog
  3. Stop-Agent.bat — легитимная остановка (watchdog не поднимает снова)
  4. FFmpeg — только если рядом есть ffmpeg.exe


Если видео не открывается
-------------------------
  Дождитесь полной остановки записи. HEVC: Edge + пакет кодека из Store, либо VLC.


Если сервер не открывается
--------------------------
  Background: archive-server.out.log / .err.log рядом с server.toml в ArchiveServer\
  Порт свободен; брандмауэр разрешает входящие.
