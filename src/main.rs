mod init_env;
mod upload;
mod upload_dao;
mod download;
mod display_remote;
mod helper;

use actix_web::{web, App, HttpServer};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use log::{error, info};
use upload::{upload_file, submit_file_metadata, AppState, get_uploaded_files, get_upload_status};
use init_env::{init_db_pool, check_table_structure_endpoint, ensure_table_structure_endpoint};
use simplelog::*;
use std::env;
use std::path::{Path, PathBuf};
use download::download_file;
use display_remote::{
    DLNAPlayer, discovered_devices, 
    play_video, pause_video, resume_video, stop_video, serve_media, hello, browse_files
};
use actix_files as fs;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv::dotenv().ok(); // 加载 .env 文件

    // 检查是否存在 DATABASE_URL
    let has_database = match env::var("DATABASE_URL") {
        Ok(_) => true,
        Err(_) => {
            info!("DATABASE_URL not found, skipping database initialization");
            false
        }
    };

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

    // 根据 has_database 决定是否初始化数据库
    let db_pool = if has_database {
        match init_db_pool().await {
            Ok(pool) => Some(pool),
            Err(e) => {
                error!("Failed to initialize database pool: {}", e);
                None
            }
        }
    } else {
        None
    };

    let app_state = Arc::new(AppState {
        uploads: Mutex::new(HashMap::new()),
        db_pool: db_pool.clone(),
    });

    // 创建DLNA播放器实例
    let dlna_player = Arc::new(Mutex::new(DLNAPlayer::new().await));

    info!("Starting server at http://127.0.0.1:8080");
    println!("Starting server at http://127.0.0.1:8080");

    // 启动主服务器
    let main_server = HttpServer::new(move || {
        let mut app = App::new()
            .app_data(web::Data::new(app_state.clone()))
            .app_data(web::Data::new(dlna_player.clone()))
            .route("/upload", web::post().to(upload_file));

        // 只有在有数据库连接时才添加数据库相关路由
        if has_database {
            app = app
                .app_data(web::Data::new(db_pool.clone().unwrap()))
                .route("/submit_metadata", web::post().to(submit_file_metadata))
                .route("/check_table_structure", web::get().to(check_table_structure_endpoint))
                .route("/ensure_table_structure", web::post().to(ensure_table_structure_endpoint))
                .route("/upload_status/{file_id}", web::get().to(get_upload_status))
                .route("/download/{file_id}", web::get().to(download_file))
                .route("/uploaded_files", web::get().to(get_uploaded_files));
        }

        // 添加 DLNA 相关路由并返回完整的 app
        app
            .route("/dlna/devices", web::get().to(discovered_devices))
            .route("/dlna/play", web::post().to(play_video))
            .route("/dlna/pause", web::post().to(pause_video))
            .route("/dlna/resume", web::post().to(resume_video))
            .route("/dlna/stop", web::post().to(stop_video))
            .route("/dlna/browse", web::post().to(browse_files))
            .route("/media/{filename:.*}", web::get().to(serve_media))
            .route("/hello", web::get().to(hello))
    })
    .bind("127.0.0.1:8080")?
    .run();

    // 启动媒体服务器
    let media_server = HttpServer::new(|| {
        App::new()
            .service(fs::Files::new("/", "./media").show_files_listing())
    })
    .bind("0.0.0.0:8081")?
    .run();

    // 使用 tokio::spawn 启动两个服务器
    tokio::spawn(main_server);
    tokio::spawn(media_server);

    // 保持主线程运行
    tokio::signal::ctrl_c().await.expect("Failed to listen for ctrl-c");

    Ok(())
}
