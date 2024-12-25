mod upload;
mod upload_metadata;
mod init_env;

use actix_web::{web, App, HttpServer};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use log::{error, info};
use std::fs;
use upload::{upload_file, submit_file_metadata, AppState, check_table_structure_endpoint, ensure_table_structure_endpoint};
use init_env::init_db_pool;
use sqlx::{MySqlPool, Executor};
use simplelog::*;
use std::env;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv::dotenv().ok(); // 加载 .env 文件

    // 设置日志输出
    let log_file_path = env::var("LOG_FILE_PATH").expect("LOG_FILE_PATH must be set");
    CombinedLogger::init(vec![
        WriteLogger::new(
            LevelFilter::Info,
            Config::default(),
            std::fs::File::create(log_file_path).unwrap(),
        ),
    ])
    .unwrap();

    if let Err(e) = std::fs::create_dir_all("uploads") {
        error!("Failed to create uploads directory: {}", e);
        return Err(e);
    }

    let db_pool = init_db_pool().await;

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
            .route("/ensure_table_structure", web::get().to(ensure_table_structure_endpoint))
            // 其他路由保持不变
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
