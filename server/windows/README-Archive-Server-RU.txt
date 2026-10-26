Сервер видеоархива (Windows)
============================

Файлы:
  Setup-Archive-Server.bat
  Start-Archive-Server.bat
  Start-Archive-Server-Background.bat   - скрытый запуск без окна
  Stop-Archive-Server.bat               - остановить фоновый сервер
  Show-Agent-Token.bat
  README-Archive-Server-RU.txt
  archive-server.exe  (или bin\archive-server.exe)

BAT-файлы на английском специально (кодировка CMD).
Русский текст показывает archive-server.exe.


1. Администратор
----------------
  1) Setup-Archive-Server.bat — папка архива, пароль, порт
     В конце Setup печатаются готовые URL с IP этого ПК.
  2) Запуск:
       Start-Archive-Server.bat              — окно на экране
       Start-Archive-Server-Background.bat   — без окна (фон)
       Stop-Archive-Server.bat               — остановить фон
  3) Браузер: http://127.0.0.1:8787/  (пароль администратора)
  4) Токен агента: Show-Agent-Token.bat / AGENT-TOKEN.txt


2. Рабочий ПК
-------------
  Консоль -> "Сервер архива (HTTP)":
    Адрес = один из URL, показанных в Setup (например http://192.168.x.x:8787)
    Токен = AGENT-TOKEN.txt  (НЕ пароль администратора)
  "Проверить подключение".


3. Где видео
------------
  Каталог из Setup (archive_dir в server.toml):
    <архив>\<ID_ПК>\<год>\<месяц>\<день>\*.mp4


4. Вход и просмотр по IP (в т.ч. VPS)
------------------------------------
  Браузер: http://IP_СЕРВЕРА:8787/
  Стриминг с того же адреса — IP не мешает воспроизведению.
  Нужен кодек HEVC в браузере (Edge + пакет из Microsoft Store)
  или скачивание файла в VLC. Firewall должен пропускать порт.


5. Ubuntu VPS
-------------
  См. папку UbuntuVPS\ или server\linux\README-Ubuntu-RU.txt


6. Если сервер не открывается
-----------------------------
  - Background: archive-server.out.log / .err.log рядом с server.toml
  - Порт свободен; брандмауэр разрешает входящие
