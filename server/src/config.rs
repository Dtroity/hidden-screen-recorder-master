use std::{
    fs,
    path::{Path, PathBuf},
};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
    pub archive_dir: PathBuf,
    /// SHA-256 hex of admin password (UTF-8).
    pub admin_password_hash: String,
    /// Shared secrets for agent uploads.
    pub agent_tokens: Vec<String>,
    #[serde(default = "default_session_hours")]
    pub session_hours: u64,
}

fn default_session_hours() -> u64 {
    12
}

impl ServerConfig {
    pub fn load(path: &PathBuf) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("Invalid config: {e}"))
    }

    pub fn save(&self, path: &PathBuf) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(path, text).map_err(|e| e.to_string())
    }
}

pub fn hash_password(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn verify_password(password: &str, hash_hex: &str) -> bool {
    let got = hash_password(password);
    got.as_bytes().ct_eq(hash_hex.as_bytes()).into()
}

pub fn random_token() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn init_config(
    path: &Path,
    archive_dir: PathBuf,
    admin_password: &str,
    listen: &str,
    force: bool,
) -> Result<ServerConfig, String> {
    if path.is_file() && !force {
        return Err(format!(
            "Config already exists: {}. Refusing to overwrite. Use --force to replace (creates a new agent token).",
            path.display()
        ));
    }
    if admin_password.trim().is_empty() {
        return Err("Admin password must not be empty".into());
    }
    let listen = listen.trim();
    if listen.is_empty() {
        return Err("Listen address must not be empty".into());
    }
    let agent = random_token();
    let cfg = ServerConfig {
        listen: listen.to_owned(),
        archive_dir: archive_dir.clone(),
        admin_password_hash: hash_password(admin_password),
        agent_tokens: vec![agent.clone()],
        session_hours: 12,
    };
    let path_buf = path.to_path_buf();
    cfg.save(&path_buf)?;
    fs::create_dir_all(&archive_dir).map_err(|e| e.to_string())?;
    // Same token as in server.toml — convenience file for first setup (not a second secret).
    if let Some(parent) = path.parent() {
        let token_note = parent.join("AGENT-TOKEN.txt");
        let body = format!(
            "Agent upload token for Hidden Screen Recorder\r\n\
             ==================================================\r\n\
             IMPORTANT: this is NOT the admin password.\r\n\
             Paste this token into the agent settings field «Токен отправки».\r\n\
             \r\n\
             {agent}\r\n\
             \r\n\
             The same value is stored in {config} under agent_tokens.\r\n\
             To show it again later: archive-server.exe --config \"{config}\" show-agent-token\r\n\
             Keep this file private. Do not put it in shared folders or logs.\r\n",
            config = path.display()
        );
        let _ = fs::write(&token_note, body);
    }
    eprintln!("Wrote {}", path.display());
    eprintln!("Archive directory: {}", archive_dir.display());
    eprintln!("Listen address (edit in config if needed): {}", cfg.listen);
    eprintln!();
    eprintln!("=== AGENT UPLOAD TOKEN (not the admin password) ===");
    eprintln!("{agent}");
    eprintln!("===================================================");
    eprintln!("Also saved next to the config as AGENT-TOKEN.txt");
    eprintln!("Recover later: archive-server --config <path> show-agent-token");
    eprintln!("(token is stored in server.toml under agent_tokens)");
    eprintln!("Admin password was hashed into the config (not logged).");
    Ok(cfg)
}

/// Print existing agent tokens from a saved config (recovery; does not create new ones).
pub fn print_agent_tokens(cfg: &ServerConfig) {
    if cfg.agent_tokens.is_empty() {
        eprintln!("No agent_tokens in config. Run init or add a token to agent_tokens.");
        return;
    }
    eprintln!("Agent upload token(s) from config (NOT the admin password):");
    for (i, t) in cfg.agent_tokens.iter().enumerate() {
        eprintln!("  [{}] {t}", i + 1);
    }
}
