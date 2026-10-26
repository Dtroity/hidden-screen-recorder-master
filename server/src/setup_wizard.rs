//! Interactive first-time setup (console). Avoids fragile UTF-8 .bat scripts.

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::config::init_config;

pub fn run_wizard(config_path: &Path) -> Result<(), String> {
    if config_path.is_file() {
        eprintln!("Конфигурация уже есть: {}", config_path.display());
        eprintln!("Показать токен агента:");
        eprintln!(
            "  archive-server.exe --config \"{}\" show-agent-token",
            config_path.display()
        );
        eprintln!("Запуск сервера:");
        eprintln!(
            "  archive-server.exe --config \"{}\" run",
            config_path.display()
        );
        eprintln!("(Удаляйте server.toml только если нужен НОВЫЙ токен.)");
        return Ok(());
    }

    eprintln!("=== Настройка сервера видеоархива ===");
    eprintln!("Пароль администратора = вход в веб-интерфейс.");
    eprintln!("Токен агента          = отправка с рабочих ПК (создаётся сам).");
    eprintln!();

    let default_archive = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("archive");
    let archive_dir = prompt_line(&format!(
        "Каталог архива [{}]: ",
        default_archive.display()
    ))?;
    let archive_dir = if archive_dir.trim().is_empty() {
        default_archive
    } else {
        PathBuf::from(archive_dir.trim())
    };

    let password = prompt_line("Пароль администратора (веб): ")?;
    if password.trim().is_empty() {
        return Err("Пароль не может быть пустым".into());
    }
    let password2 = prompt_line("Повторите пароль: ")?;
    if password != password2 {
        return Err("Пароли не совпадают".into());
    }

    let port = prompt_line("Порт HTTP [8787]: ")?;
    let port = if port.trim().is_empty() {
        "8787".to_owned()
    } else {
        port.trim().to_owned()
    };
    if port.parse::<u16>().is_err() {
        return Err(format!("Неверный порт: {port}"));
    }
    let listen = format!("0.0.0.0:{port}");

    if port_in_use(&port) {
        eprintln!("ВНИМАНИЕ: порт {port} похоже занят. Выберите другой или остановите программу.");
    }

    init_config(config_path, archive_dir, password.trim(), &listen, false)?;

    let lan_urls = lan_http_urls(&port);
    eprintln!();
    eprintln!("Готово.");
    eprintln!("1) Запуск: Start-Archive-Server.bat");
    eprintln!("   (или Start-Archive-Server-Background.bat — без окна на экране)");
    eprintln!("2) Браузер на этом ПК: http://127.0.0.1:{port}/  (пароль администратора)");
    if lan_urls.is_empty() {
        eprintln!("3) На рабочих ПК укажите URL вида http://<IP_этого_ПК>:{port}");
        eprintln!("   (IP не определён автоматически — смотрите ipconfig / ifconfig)");
    } else {
        eprintln!("3) На рабочих ПК вставьте один из адресов + токен из AGENT-TOKEN.txt:");
        for url in &lan_urls {
            eprintln!("     {url}");
        }
    }
    Ok(())
}

/// Print browser/agent URLs for an existing config (no secrets).
pub fn print_access_urls(listen: &str) {
    let port = listen
        .rsplit_once(':')
        .map(|(_, p)| p.trim())
        .filter(|p| !p.is_empty())
        .unwrap_or("8787");
    eprintln!("На этом ПК:     http://127.0.0.1:{port}/");
    let urls = lan_http_urls(port);
    if urls.is_empty() {
        eprintln!("В сети:         http://<IP_этого_ПК>:{port}/  (IP не определён)");
    } else {
        eprintln!("Для рабочих ПК (скопируйте в «Адрес сервера»):");
        for url in urls {
            eprintln!("  {url}");
        }
    }
}

/// Best-effort LAN IPv4 list for agent URL hints (never logs tokens).
fn lan_http_urls(port: &str) -> Vec<String> {
    let mut ips = Vec::new();
    // Route-based guess: the address used toward the default gateway / internet.
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
        && socket.connect("8.8.8.8:80").is_ok()
        && let Ok(local) = socket.local_addr()
        && let std::net::IpAddr::V4(v4) = local.ip()
        && is_useful_lan_v4(v4)
    {
        ips.push(v4.to_string());
    }
    // Hostname resolution may add extra LAN addresses.
    if let Ok(iter) = dns_lookup_ips() {
        for ip in iter {
            if let std::net::IpAddr::V4(v4) = ip
                && is_useful_lan_v4(v4)
                && !ips.iter().any(|s| s == &v4.to_string())
            {
                ips.push(v4.to_string());
            }
        }
    }
    ips.into_iter()
        .map(|ip| format!("http://{ip}:{port}"))
        .collect()
}

fn dns_lookup_ips() -> Result<Vec<std::net::IpAddr>, ()> {
    let host = hostname().ok_or(())?;
    let addrs = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), 0)).map_err(|_| ())?;
    Ok(addrs.map(|a| a.ip()).collect())
}

fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.is_empty())
}

fn is_useful_lan_v4(v4: std::net::Ipv4Addr) -> bool {
    if v4.is_loopback() || v4.is_unspecified() || v4.is_link_local() || v4.is_multicast() {
        return false;
    }
    let o = v4.octets();
    // 198.18.0.0/15 — often VPN/virtual adapters; prefer real LAN for office agents.
    if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
        return false;
    }
    true
}

fn prompt_line(label: &str) -> Result<String, String> {
    eprint!("{label}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn port_in_use(port: &str) -> bool {
    let addr = format!("127.0.0.1:{port}");
    std::net::TcpListener::bind(&addr).is_err()
}
