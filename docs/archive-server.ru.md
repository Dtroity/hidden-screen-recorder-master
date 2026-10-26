# Сервер видеоархива

Лёгкий HTTP-сервер для приёма MP4 с рабочих ПК и просмотра в браузере. Не выполняет захват экрана и не перекодирует видео.

## Готовый комплект (Windows, без Rust)

Соберите папку поставки:

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\pack-office-recorder.ps1
```

Результат: `target\OfficeRecorder\` — консоль, агент и сервер в одной структуре.

Админ (сервер) — папка `ArchiveServer\` в комплекте:

1. **Setup-Archive-Server.bat** (один раз) — интерактивная настройка; в конце печатаются URL с IP этого ПК.
2. Запуск:
   - **Start-Archive-Server.bat** — окно на экране;
   - **Start-Archive-Server-Background.bat** — скрытый фон;
   - **Stop-Archive-Server.bat** — остановка фонового процесса.
3. Браузер: `http://127.0.0.1:8787/`.

Подробности: `README-RU.txt` (корень комплекта) и `ArchiveServer\README-Archive-Server-RU.txt`.

### Ubuntu VPS

См. `server/linux/README-Ubuntu-RU.txt` и `install-ubuntu.sh` (systemd-служба `hsr-archive`). Бинарник собирается на Linux (`cargo build -p archive-server --release`).

## Сборка из исходников

### Windows x64

```powershell
cargo build -p archive-server --release
cargo build --release
```

Исполняемый файл сервера: `target\release\archive-server.exe`

Единый комплект (консоль + агент + сервер + UbuntuVPS):

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\pack-office-recorder.ps1
```

Результат: `target\OfficeRecorder\` (без секретов). Только сервер: подпапка `ArchiveServer\`.

### Воспроизведение в панели по IP (в т.ч. VPS)

Вход: `http://IP_СЕРВЕРА:8787/` (порт из `listen`, на Ubuntu обычно `0.0.0.0:8787`).  
Стриминг MP4 идёт с того же origin (`/api/media/...`), HTTP Range поддерживается — **доступ по IP сам по себе не ломает плеер**.

Ограничение — **кодек HEVC (H.265)** в браузере (Edge/Chrome на Windows нужен пакет HEVC из Microsoft Store; иначе скачайте файл и откройте в VLC). На macOS/Linux в браузере HEVC часто недоступен.

Для интернета лучше HTTPS через Caddy/nginx (см. ниже). Откройте порт в firewall (`ufw allow 8787/tcp`).

### Linux x64

```bash
cargo build -p archive-server --release
# binary: target/release/archive-server
```

Требуется Rust stable. Зависимости SQLite собраны с `bundled` — отдельный `libsqlite3` не нужен.

## Первый запуск (CLI)

```powershell
archive-server --config server.toml init --admin-password "СЕКРЕТ" --archive-dir "D:\VideoArchive" --listen "0.0.0.0:8787"
archive-server --config server.toml run
```

Команда `init` создаёт:

- `server.toml` (адрес, каталог архива, хеш пароля, токен агента);
- каталог архива;
- `AGENT-TOKEN.txt` рядом с конфигом (тот же токен текстом).

Существующий `server.toml` **не перезаписывается** без `--force`.

### Токен агента

- Создаётся при `init` / Setup.
- Хранится в `server.toml` в поле `agent_tokens` (можно прочитать снова).
- Дублируется в `AGENT-TOKEN.txt` для удобства.
- Повторный вывод без смены токена:

```powershell
archive-server --config server.toml show-agent-token
```

Это **не** пароль администратора. Пароль — только для входа в веб-интерфейс.

Откройте в браузере: `http://127.0.0.1:8787/` и войдите с паролем администратора.

## Структура архива

```text
archive/<PC_ID>/<YYYY>/<MM>/<DD>/<recording>.mp4
archive/index.sqlite
```

Отображаемое имя ПК меняется в веб-интерфейсе (двойной щелчок по компьютеру), постоянный ID не меняется.

## Подключение рабочего компьютера

В консоли Hidden Screen Recorder → **Сервер архива (HTTP)**:

1. Включить «Отправлять на сервер архива».
2. URL, например `http://192.168.0.10:8787` (IP ПК с сервером и порт из `listen`).
3. **Токен отправки** = токен агента из Setup / `AGENT-TOKEN.txt` / `show-agent-token` (не пароль админа).
4. Нажать «Проверить подключение».
5. Сохранить настройки.

Отправляются только **завершённые** файлы. При недоступности сервера запись не останавливается; файлы остаются в локальной очереди и уходят при следующей попытке (агент повторяет каждые ~30 с).

Локальное удаление после успешной отправки — отдельный переключатель (по умолчанию выкл.).

## Автозапуск

### Windows

Планировщик заданий или служба через `nssm` / встроенный «Запуск при входе»:

```text
archive-server.exe --config C:\Archive\server.toml run
```

Либо ярлык на `Start-Archive-Server.bat` в автозагрузке.

### Linux (systemd)

Готовый unit и установщик: `server/linux/` (`install-ubuntu.sh`, `hsr-archive.service`).

```bash
sudo systemctl enable --now hsr-archive
```

## Резервное копирование

Копируйте каталог архива целиком (включая `index.sqlite` и WAL-файлы после остановки сервера).  
При потере БД: `archive-server rescan` восстановит индекс по файлам на диске (длительность может быть пустой).

## Диагностика отправки

- На ПК: «Проверить подключение» в настройках; журнал диагностики (`archive upload` / `archive queue`).
- Статус очереди в настройках консоли.
- На сервере: ответ `POST /api/upload` (401 — неверный токен, 400 — нет файла / X-PC-ID).

## HTTPS

Сам сервер слушает HTTP. Для доступа из недоверенной сети поставьте обратный прокси (Caddy / nginx) с TLS и проксированием на `127.0.0.1:8787`. Токены и пароль администратора не должны светиться в логах прокси в открытом виде.

## Безопасность (кратко)

- Токен агента → только `/api/upload`.
- Пароль администратора → просмотр/скачивание.
- Пути файлов нормализуются; выход за каталог архива блокируется.
