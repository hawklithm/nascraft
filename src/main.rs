mod init_env;
mod upload;
mod upload_dao;
mod download;
mod display_remote;
mod helper;
mod config;
mod logging;
mod context;
mod router;
mod server;
mod mdns_advertise;
mod udp_discovery;
mod ssdp;
mod file_checker;
mod thumbnail;

use crate::config::AppConfig;
use crate::context::AppContext;
use crate::init_env::init_db_pool;
use crate::logging::{ensure_data_dirs, init_logging};
use crate::mdns_advertise::{shutdown_mdns, start_mdns_advertise};
use crate::router::build_router;
use crate::server::serve_http;
use crate::udp_discovery::{run_udp_discovery_responder, run_udp_broadcast_announcer};
use crate::ssdp::{run_ssdp_responder, run_ssdp_announcer};
use crate::file_checker::start_file_integrity_checker;
use crate::upload::AppState;
use tracing::info;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    init_logging()?;
    ensure_data_dirs()?;

    info!("Nascraft starting up");

    // DATABASE_URL is required
    if env::var("DATABASE_URL").is_err() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "DATABASE_URL must be set"));
    }

    // Initialize DB pool and ensure tables on startup
    let db_pool = init_db_pool()
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("Failed to initialize database pool: {}", e)))?;

    let app_state = Arc::new(AppState {
        uploads: Mutex::new(HashMap::new()),
        db_pool,
    });

    // 创建DLNA播放器实例
    let dlna_player = Arc::new(Mutex::new(crate::display_remote::DLNAPlayer::new().await));

    let ctx = AppContext {
        app_state: app_state.clone(),
        dlna_player: dlna_player.clone(),
    };

    let cfg = AppConfig::from_env();

    info!("Starting mDNS advertisement");

    let mdns = start_mdns_advertise(&cfg)?;

    info!("Starting UDP discovery responder and broadcaster");

    tokio::spawn(run_udp_discovery_responder(cfg.clone()));
    tokio::spawn(run_udp_broadcast_announcer(cfg.clone()));

    info!("Starting SSDP (UPnP) discovery responder and announcer");

    tokio::spawn(run_ssdp_responder(cfg.clone()));
    tokio::spawn(run_ssdp_announcer(cfg.clone()));

    info!("Starting file integrity checker (10-minute interval)");

    start_file_integrity_checker(app_state.db_pool.clone());

    println!("Starting server at http://0.0.0.0:{}", cfg.server_port);

    info!("Starting HTTP server on 0.0.0.0:{}", cfg.server_port);

    let app = build_router(ctx.clone());
    serve_http(app, cfg.server_port).await?;

    tokio::signal::ctrl_c().await.expect("Failed to listen for ctrl-c");
    info!("Shutdown signal received (ctrl-c)");
    shutdown_mdns(mdns);
    info!("Shutdown complete");
    Ok(())
}
