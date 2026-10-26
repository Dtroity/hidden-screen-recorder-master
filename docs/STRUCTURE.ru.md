# Структура репозитория

```text
hidden-screen-recorder-master/
├── src/                      # Windows: консоль + библиотека + агент
│   ├── bin/recorder_agent.rs
│   ├── encode/               # Media Foundation / FFmpeg
│   └── assets/               # Иконки UI
├── server/                   # Crate archive-server (веб-архив)
│   ├── src/                  # HTTP API, SQLite, auth
│   ├── static/index.html     # Панель администратора
│   ├── windows/              # BAT/README для Windows-сервера
│   └── linux/                # install-ubuntu.sh + systemd
├── packaging/windows/        # Сборка комплекта OfficeRecorder
│   ├── pack-office-recorder.ps1
│   ├── Start-Console.bat, Start/Stop/Watch-Agent.bat
│   └── README-RU.txt
├── docs/                     # Документация и скриншоты
├── tools/                    # Вспомогательные скрипты разработчика
├── third_party/ffmpeg/       # Как поставлять ffmpeg.exe
├── vendor/speexdsp/          # Шумоподавление (собирается в build.rs)
├── .github/                  # CI / шаблоны Issues
├── LICENSE                   # MIT
└── Cargo.toml                # Workspace: приложение + server
```

Сборка поставки: `packaging/windows/pack-office-recorder.ps1` → `target/OfficeRecorder/`:

- корень: агент/консоль + `bin\`
- `ArchiveServer\`: только сервер (BAT + exe)
- `UbuntuVPS\`: скрипты Linux

Артефакты `target/` в git не входят.