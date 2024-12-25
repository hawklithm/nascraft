use actix_web::{web, HttpRequest, HttpResponse};
use futures::StreamExt;
use sha2::{Sha256, Digest};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use log::error;
use sanitize_filename::sanitize;
use uuid::Uuid;
use sqlx::mysql::MySqlPool;
use crate::upload_metadata::UploadState;
use crate::init_env::check_table_structure;
use crate::init_env::ensure_table_structure;

#[derive(Debug)]
pub struct AppState {
    pub uploads: Mutex<HashMap<String, UploadState>>,
    pub db_pool: MySqlPool,
}

pub async fn upload_file(
    req: HttpRequest,
    mut payload: web::Payload,
    data: web::Data<Arc<AppState>>,
) -> HttpResponse {
    let content_length = match req.headers()
        .get(actix_web::http::header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<u64>().ok()) {
            Some(len) => len,
            None => {
                error!("Invalid content length");
                return HttpResponse::BadRequest().body("Invalid content length");
            }
        };

    let content_range = req.headers()
        .get(actix_web::http::header::CONTENT_RANGE)
        .and_then(|h| h.to_str().ok());

    let filename = match req.headers()
        .get("X-Filename")
        .and_then(|h| h.to_str().ok()) {
            Some(name) => name.to_string(),
            None => {
                error!("Missing filename");
                return HttpResponse::BadRequest().body("Missing filename");
            }
        };

    let safe_filename = sanitize(&filename);
    let file_path = format!("uploads/{}", safe_filename);

    let (start_pos, end_pos) = match content_range {
        Some(range) => {
            let parts: Vec<&str> = range.split('/').next()
                .unwrap_or("bytes 0-0")
                .split('-')
                .collect();
            (
                parts[0].replace("bytes ", "").parse::<u64>().unwrap_or(0),
                parts.get(1).and_then(|&s| s.parse::<u64>().ok()).unwrap_or(content_length - 1)
            )
        },
        None => (0, content_length - 1)
    };

    let mut file = match OpenOptions::new()
        .create(true)
        .write(true)
        .open(&file_path)
        .await {
            Ok(f) => f,
            Err(e) => {
                error!("File error: {}", e);
                return HttpResponse::InternalServerError().body(format!("File error: {}", e));
            }
        };

    if let Err(e) = file.seek(tokio::io::SeekFrom::Start(start_pos)).await {
        error!("Seek error: {}", e);
        return HttpResponse::InternalServerError().body(format!("Seek error: {}", e));
    }

    let mut hasher = Sha256::new();
    let mut uploaded_size = start_pos;

    while let Some(chunk) = payload.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                error!("Payload error: {}", e);
                return HttpResponse::InternalServerError().body(format!("Payload error: {}", e));
            }
        };

        if let Err(e) = file.write_all(&chunk).await {
            error!("Write error: {}", e);
            return HttpResponse::InternalServerError().body(format!("Write error: {}", e));
        }
        hasher.update(&chunk);
        uploaded_size += chunk.len() as u64;

        let mut uploads = data.uploads.lock().await;
        uploads.insert(safe_filename.clone(), UploadState {
            id: Uuid::new_v4().to_string(),
            filename: safe_filename.clone(),
            total_size: end_pos + 1,
            uploaded_size,
            checksum: format!("{:x}", hasher.clone().finalize()),
        });

        // 更新数据库记录
        let upload_state = uploads.get(&safe_filename).unwrap();
        upload_state.update_in_db(&data.db_pool).await.unwrap();
    }

    let final_checksum = format!("{:x}", hasher.finalize());

    HttpResponse::Ok()
        .content_type("application/json")
        .json(json!({
            "status": "success",
            "filename": safe_filename,
            "size": uploaded_size,
            "checksum": final_checksum
        }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMetadata {
    pub filename: String,
    pub total_size: u64,
}

pub async fn submit_file_metadata(
    metadata: web::Json<FileMetadata>,
    data: web::Data<Arc<AppState>>,
) -> HttpResponse {
    let safe_filename = sanitize(&metadata.filename);

    let unique_id = Uuid::new_v4().to_string();

    let mut uploads = data.uploads.lock().await;
    uploads.insert(safe_filename.clone(), UploadState {
        id: unique_id.clone(),
        filename: safe_filename.clone(),
        total_size: metadata.total_size,
        uploaded_size: 0,
        checksum: String::new(),
    });

    // 保存到数据库
    let upload_state = uploads.get(&safe_filename).unwrap();
    upload_state.save_to_db(&data.db_pool).await.unwrap();

    HttpResponse::Ok().json(json!({
        "message": "Metadata submitted successfully",
        "id": unique_id
    }))
}

pub async fn init_db_pool() -> MySqlPool {
    MySqlPool::connect("mysql://user:password@localhost/database").await.unwrap()
}

pub async fn check_table_structure_endpoint(
    data: web::Data<MySqlPool>,
) -> HttpResponse {
    match check_table_structure(&data).await {
        Ok(_) => HttpResponse::Ok().json("Table structure is as expected."),
        Err(e) => HttpResponse::InternalServerError().body(format!("Table structure check failed: {}", e)),
    }
}

pub async fn ensure_table_structure_endpoint(
    data: web::Data<MySqlPool>,
) -> HttpResponse {
    match ensure_table_structure(&data).await {
        Ok(_) => HttpResponse::Ok().json("Table structure is ensured using init.sql."),
        Err(e) => HttpResponse::InternalServerError().body(format!("Failed to ensure table structure: {}", e)),
    }
}