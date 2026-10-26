#!/usr/bin/env bash
# Install archive-server as a systemd service on Ubuntu 22.04/24.04 x86_64.
set -euo pipefail

BINARY=""
ADMIN_PASSWORD=""
PORT="8787"
ARCHIVE_DIR="/var/lib/hsr-archive"
FORCE=0

usage() {
  echo "Usage: $0 --binary /path/to/archive-server --admin-password 'SECRET' [--port 8787] [--archive-dir DIR] [--force]"
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary) BINARY="$2"; shift 2 ;;
    --admin-password) ADMIN_PASSWORD="$2"; shift 2 ;;
    --port) PORT="$2"; shift 2 ;;
    --archive-dir) ARCHIVE_DIR="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    *) usage ;;
  esac
done

[[ -n "$BINARY" && -n "$ADMIN_PASSWORD" ]] || usage
[[ -f "$BINARY" ]] || { echo "Binary not found: $BINARY"; exit 1; }

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Run as root: sudo $0 ..."
  exit 1
fi

# Sanity: ELF Linux x86_64 (not a Windows .exe copied by mistake)
if ! file -b "$BINARY" 2>/dev/null | grep -qiE 'ELF.*(x86-64|x86_64)'; then
  echo "WARNING: $BINARY does not look like a Linux x86_64 ELF binary."
  echo "Build on the VPS with: cargo build -p archive-server --release"
fi

id hsr >/dev/null 2>&1 || useradd --system --home /opt/hsr --shell /usr/sbin/nologin hsr
mkdir -p /opt/hsr /etc/hsr "$ARCHIVE_DIR"
install -m 755 "$BINARY" /opt/hsr/archive-server
chown -R hsr:hsr /opt/hsr "$ARCHIVE_DIR"
# hsr must be able to write server.toml / AGENT-TOKEN.txt during init
chown hsr:hsr /etc/hsr
chmod 750 /etc/hsr

CONFIG=/etc/hsr/server.toml
if [[ -f "$CONFIG" && "$FORCE" -ne 1 ]]; then
  echo "Config exists: $CONFIG (use --force to recreate)"
else
  rm -f "$CONFIG" /etc/hsr/AGENT-TOKEN.txt
  sudo -u hsr /opt/hsr/archive-server --config "$CONFIG" init \
    --admin-password "$ADMIN_PASSWORD" \
    --archive-dir "$ARCHIVE_DIR" \
    --listen "0.0.0.0:${PORT}"
  chmod 640 "$CONFIG" 2>/dev/null || true
  chmod 640 /etc/hsr/AGENT-TOKEN.txt 2>/dev/null || true
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
install -m 644 "$SCRIPT_DIR/hsr-archive.service" /etc/systemd/system/hsr-archive.service
systemctl daemon-reload
systemctl enable --now hsr-archive

# Quick health check
sleep 1
if ! systemctl is-active --quiet hsr-archive; then
  echo "ERROR: service failed to start. Logs:"
  journalctl -u hsr-archive -n 40 --no-pager || true
  exit 1
fi

if command -v curl >/dev/null 2>&1; then
  CODE="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/" || true)"
  if [[ "$CODE" != "200" ]]; then
    echo "WARNING: HTTP check returned '$CODE' (expected 200)"
  else
    echo "HTTP check OK (200)"
  fi
fi

PUBLIC_IP="$(curl -4 -fsS --max-time 3 ifconfig.me 2>/dev/null || true)"
echo
echo "=== Installed ==="
echo "Status:  systemctl status hsr-archive"
echo "Logs:    journalctl -u hsr-archive -f"
echo "Local:   http://127.0.0.1:${PORT}/"
if [[ -n "$PUBLIC_IP" ]]; then
  echo "Public:  http://${PUBLIC_IP}:${PORT}/"
fi
echo "Token:   sudo -u hsr /opt/hsr/archive-server --config $CONFIG show-agent-token"
echo "Also:    /etc/hsr/AGENT-TOKEN.txt"
echo
echo "Firewall (if ufw enabled): sudo ufw allow ${PORT}/tcp && sudo ufw reload"
