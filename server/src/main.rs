mod auth;
mod config;
mod db;
mod routes;
mod setup_wizard;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post},
    Router,
};
use clap::{Parser, Subcommand};

use crate::{
    auth::Sessions,
    config::{init_config, print_agent_tokens, ServerConfig},
    db::Db,
    routes::AppState,
};

#[derive(Parser, Debug)]
#[command(name = "archive-server", about = "Video archive server for Hidden Screen Recorder")]
struct Cli {
    #[arg(long, default_value = "server.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create server.toml, archive folder, admin password hash and an agent token.
    Init {
        #[arg(long, default_value = "./archive")]
        archive_dir: PathBuf,
        #[arg(long)]
        admin_password: String,
        /// Bind address, e.g. 0.0.0.0:8787
        #[arg(long, default_value = "0.0.0.0:8787")]
        listen: String,
        /// Replace an existing config (creates a new agent token).
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Interactive first-time setup (recommended on Windows).
    Setup,
    /// Print agent upload token(s) already stored in the config file.
    ShowAgentToken,
    /// Print local/LAN URLs for the web UI and agents (no secrets).
    ShowUrls,
    /// Re-scan archive folder into SQLite index.
    Rescan,
    /// Run HTTP server (default).
    Run,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Run) {
        Commands::Init {
            archive_dir,
            admin_password,
            listen,
            force,
        } => {
            if let Err(e) = init_config(&cli.config, archive_dir, &admin_password, &listen, force)
            {
                eprintln!("init failed: {e}");
                std::process::exit(1);
            }
        }
        Commands::Setup => {
            if let Err(e) = setup_wizard::run_wizard(&cli.config) {
                eprintln!("setup failed: {e}");
                std::process::exit(1);
            }
        }
        Commands::ShowAgentToken => {
            let cfg = load_or_exit(&cli.config);
            print_agent_tokens(&cfg);
        }
        Commands::ShowUrls => {
            let cfg = load_or_exit(&cli.config);
            setup_wizard::print_access_urls(&cfg.listen);
        }
        Commands::Rescan => {
            let cfg = load_or_exit(&cli.config);
            let db = Db::open(&cfg.archive_dir).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            match db.rescan(&cfg.archive_dir) {
                Ok(n) => println!("Indexed {n} new file(s)."),
                Err(e) => {
                    eprintln!("rescan failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Run => {
            let cfg = load_or_exit(&cli.config);
            if let Err(e) = run(cfg).await {
                eprintln!("server error: {e}");
                std::process::exit(1);
            }
        }
    }
}

fn load_or_exit(path: &PathBuf) -> ServerConfig {
    ServerConfig::load(path).unwrap_or_else(|e| {
        eprintln!("{e}");
        eprintln!("First-time setup: double-click Setup-Archive-Server.bat");
        eprintln!("Or: archive-server --config <path> setup");
        std::process::exit(1);
    })
}

async fn run(cfg: ServerConfig) -> Result<(), String> {
    std::fs::create_dir_all(&cfg.archive_dir).map_err(|e| e.to_string())?;
    let db = Db::open(&cfg.archive_dir)?;
    let sessions = Arc::new(Sessions::new(cfg.session_hours));
    let state = AppState {
        cfg: Arc::new(cfg.clone()),
        db,
        sessions,
    };

    // Axum default body limit is 2 MiB — office HEVC segments are tens of MB+.
    // Without this, large uploads get a TCP reset (client: os error 10054).
    const UPLOAD_BODY_LIMIT: usize = 4 * 1024 * 1024 * 1024; // 4 GiB

    let app = Router::new()
        .route("/", get(routes::index))
        .route("/api/login", post(routes::login))
        .route("/api/storage", get(routes::storage_info))
        .route("/api/computers", get(routes::list_computers))
        .route(
            "/api/computers/{pc_id}",
            patch(routes::rename_computer).delete(routes::delete_computer_archive),
        )
        .route("/api/recordings", get(routes::list_recordings))
        .route("/api/recordings/delete", post(routes::delete_recordings_batch))
        .route("/api/recordings/{id}", delete(routes::delete_recording))
        .route("/api/media/{id}", get(routes::media))
        .route(
            "/api/upload",
            post(routes::upload).layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)),
        )
        .with_state(state);

    let addr: SocketAddr = cfg
        .listen
        .parse()
        .map_err(|e| format!("bad listen address {}: {e}", cfg.listen))?;
    println!("Archive server listening on http://{addr}");
    println!("Archive directory: {}", cfg.archive_dir.display());
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| e.to_string())?;
    axum::serve(listener, app).await.map_err(|e| e.to_string())
}
