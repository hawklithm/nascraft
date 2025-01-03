mod init_env;
mod upload;
mod upload_dao;

use actix_web::{web, App, HttpServer};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use log::{error, info};
use upload::{upload_file, submit_file_metadata, AppState, get_uploaded_files};
use init_env::{init_db_pool, check_table_structure_endpoint, ensure_table_structure_endpoint};
use simplelog::*;
use std::env;
use std::path::{Path, PathBuf};

#[actix_web::main]
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
            LevelFilter::Info,
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

    // Initialize the database pool
    let db_pool = match init_db_pool().await {
        Ok(pool) => pool,
        Err(e) => {
            error!("Failed to initialize database pool: {}", e);
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "Failed to initialize database pool"));
        }
    };

    let app_state = Arc::new(AppState {
        uploads: Mutex::new(HashMap::new()),
        db_pool: db_pool.clone(),
    });

    info!("Starting server at http://127.0.0.1:8080");
    println!("Starting server at http://127.0.0.1:8080");

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(app_state.clone()))
            .app_data(web::Data::new(db_pool.clone()))
            .route("/upload", web::post().to(upload_file))
            .route("/submit_metadata", web::post().to(submit_file_metadata))
            .route("/check_table_structure", web::get().to(check_table_structure_endpoint))
            .route("/ensure_table_structure", web::post().to(ensure_table_structure_endpoint))
            .route("/uploaded_files", web::get().to(get_uploaded_files))
            // 其他路由保持不变
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
