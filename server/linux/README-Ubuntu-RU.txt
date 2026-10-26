Сервер видеоархива на Ubuntu VPS — конкретные команды
=====================================================

Требования: Ubuntu 22.04 или 24.04, архитектура x86_64.
Бинарник нужно СОБРАТЬ НА VPS (кросс-сборка с Windows в этом комплекте не используется).


1) Подготовка VPS
-----------------
  sudo apt update
  sudo apt install -y build-essential pkg-config curl git file
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
  rustc -V
  cargo -V


2) Исходники на VPS
-------------------
  Вариант A — git:
    git clone <URL_ВАШЕГО_РЕПОЗИТОРИЯ> hsr
    cd hsr

  Вариант B — копирование с Windows (пример):
    # на Windows (PowerShell), из корня проекта:
    scp -r . user@VPS_IP:~/hsr
    # на VPS:
    cd ~/hsr


3) Сборка archive-server
------------------------
  cd ~/hsr
  cargo build -p archive-server --release
  # результат: target/release/archive-server
  file target/release/archive-server
  # должно быть: ELF 64-bit LSB ... x86-64


4) Установка как службы systemd
-------------------------------
  sudo chmod +x server/linux/install-ubuntu.sh
  sudo bash server/linux/install-ubuntu.sh \
    --binary "$HOME/hsr/target/release/archive-server" \
    --admin-password 'ВАШ_ПАРОЛЬ_АДМИНА' \
    --port 8787 \
    --archive-dir /var/lib/hsr-archive

  Скрипт:
    - создаёт пользователя hsr
    - ставит /opt/hsr/archive-server
    - пишет /etc/hsr/server.toml + AGENT-TOKEN.txt
    - включает systemd unit hsr-archive (Restart=on-failure)
    - проверяет, что служба активна и отвечает HTTP 200


5) Файрвол
----------
  sudo ufw allow 8787/tcp
  sudo ufw reload
  sudo ufw status


6) Проверка
-----------
  sudo systemctl status hsr-archive
  curl -I http://127.0.0.1:8787/
  sudo -u hsr /opt/hsr/archive-server --config /etc/hsr/server.toml show-agent-token
  sudo -u hsr /opt/hsr/archive-server --config /etc/hsr/server.toml show-urls

  В браузере: http://ПУБЛИЧНЫЙ_IP:8787/  (пароль администратора из шага 4)


7) Управление
-------------
  sudo systemctl restart hsr-archive
  sudo systemctl stop hsr-archive
  sudo journalctl -u hsr-archive -f


8) Обновление бинарника
-----------------------
  cd ~/hsr
  git pull   # или снова scp
  cargo build -p archive-server --release
  sudo systemctl stop hsr-archive
  sudo install -m 755 target/release/archive-server /opt/hsr/archive-server
  sudo systemctl start hsr-archive


Примечания
----------
  - Токен агента ≠ пароль администратора.
  - Для HTTPS снаружи поставьте Caddy/nginx как reverse proxy на 127.0.0.1:8787.
  - Windows .exe на Linux не запустится — только ELF из шага 3.
