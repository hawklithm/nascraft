mod init_env;
mod upload;
mod upload_dao;
mod download;
mod display_remote;
mod helper;

use axum::{routing::{get, post}, Router};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use log::{error, info};
use upload::{upload_file, submit_file_metadata, AppState, get_uploaded_files, get_upload_status};
use init_env::init_db_pool;
use simplelog::*;
use std::env;
use std::path::{Path, PathBuf};
use download::download_file;
use display_remote::{
    DLNAPlayer, discovered_devices,
    play_video, pause_video, resume_video, stop_video, hello, browse_files
};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use local_ip_address::local_ip;

#[derive(Clone)]
pub struct AppContext {
    pub app_state: Arc<AppState>,
    pub dlna_player: Arc<Mutex<DLNAPlayer>>,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    dotenv::dotenv().ok(); // 加载 .env 文件

    // 设置日志输出
    let log_file_path = match env::var("LOG_FILE_PATH") {
        Ok(path) => path,
        Err(_) => {
            error!("LOG_FILE_PATH must be set");
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "LOG_FILE_PATH not set"));
        }
    };

    // 确保日志目录存在
    let log_path = Path::new(&log_file_path);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 获取日志文件的绝对路径
    let absolute_log_path = std::env::current_dir()?
        .join(log_path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&log_file_path));
    
    println!("Log file absolute path: {}", absolute_log_path.display());

    CombinedLogger::init(vec![
        WriteLogger::new(
            LevelFilter::Debug,
            Config::default(),
            std::fs::File::create(&log_file_path).unwrap_or_else(|e| {
                error!("Failed to create log file: {}", e);
                std::process::exit(1);
            }),
        ),
    ])
    .unwrap();

    if let Err(e) = std::fs::create_dir_all("uploads") {
        error!("Failed to create uploads directory: {}", e);
        return Err(e);
    }

    // 创建media目录
    if let Err(e) = std::fs::create_dir_all("media") {
        error!("Failed to create media directory: {}", e);
        return Err(e);
    }

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
    let dlna_player = Arc::new(Mutex::new(DLNAPlayer::new().await));

    let ctx = AppContext {
        app_state: app_state.clone(),
        dlna_player: dlna_player.clone(),
    };

    let server_port: u16 = env::var("NASCRAFT_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(8080);

    let mdns_service_type = env::var("NASCRAFT_MDNS_SERVICE_TYPE")
        .unwrap_or_else(|_| "_nascraft._tcp.local.".to_string());
    let mdns_instance_name = env::var("NASCRAFT_MDNS_INSTANCE")
        .unwrap_or_else(|_| "nascraft".to_string());

    let mdns = ServiceDaemon::new().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Failed to create mDNS daemon: {e}"))
    })?;

    let ip = local_ip().unwrap_or_else(|e| {
        error!("Failed to get local IP: {}", e);
        "127.0.0.1".parse().expect("127.0.0.1 should be valid")
    });
    let host_name = format!("{}.local.", mdns_instance_name);
    let mut mdns_properties: HashMap<String, String> = HashMap::new();
    mdns_properties.insert("proto".to_string(), "http".to_string());
    mdns_properties.insert("port".to_string(), server_port.to_string());
    let service_info = ServiceInfo::new(
        &mdns_service_type,
        &mdns_instance_name,
        &host_name,
        ip,
        server_port,
        mdns_properties,
    )
    .map(|s| s.enable_addr_auto())
    .map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Failed to create mDNS service info: {e}"))
    })?;

    mdns.register(service_info).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Failed to register mDNS service: {e}"))
    })?;

    info!(
        "mDNS service registered: type={}, instance={}, ip={}, port={}",
        mdns_service_type, mdns_instance_name, ip, server_port
    );

    info!("Starting server at http://0.0.0.0:{}", server_port);
    println!("Starting server at http://0.0.0.0:{}", server_port);

    // Main API server
    let app = Router::new()
        .route("/api/upload", post(upload_file))
        .route("/api/submit_metadata", post(submit_file_metadata))
        .route("/api/upload_status/:file_id", get(get_upload_status))
        .route("/api/download/:file_id", get(download_file))
        .route("/api/uploaded_files", get(get_uploaded_files))
        .route("/api/dlna/devices", get(discovered_devices))
        .route("/api/dlna/play", post(play_video))
        .route("/api/dlna/pause", post(pause_video))
        .route("/api/dlna/resume", post(resume_video))
        .route("/api/dlna/stop", post(stop_video))
        .route("/api/dlna/browse", post(browse_files))
        .route("/api/hello", get(hello))
        .with_state(ctx.clone());

    let bind_addr = format!("0.0.0.0:{}", server_port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;

    // // Media static server (for DLNA)
    // let media_app = Router::new().nest_service("/", ServeDir::new("./media"));
    // let media_listener = tokio::net::TcpListener::bind("0.0.0.0:8081").await?;

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            error!("Main server error: {}", e);
        }
    });

    // tokio::spawn(async move {
    //     if let Err(e) = axum::serve(media_listener, media_app).await {
    //         error!("Media server error: {}", e);
    //     }
    // });

    tokio::signal::ctrl_c().await.expect("Failed to listen for ctrl-c");
    if let Err(e) = mdns.shutdown() {
        error!("mDNS shutdown failed: {}", e);
    }
    Ok(())
}
