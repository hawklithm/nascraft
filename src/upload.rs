use actix_web::{web, HttpRequest, HttpResponse};
use futures::StreamExt;
use sha2::{Sha256, Digest};
use tokio::fs::{self, OpenOptions};
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
use crate::init_env::{check_table_structure_endpoint, ensure_table_structure_endpoint, check_system_initialized};
use sqlx::query;
use sqlx::types::BigDecimal;
use bigdecimal::ToPrimitive;

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
    if let Err(response) = check_system_initialized(&data.db_pool).await {
        return response;
    }

    let file_id = match req.headers()
        .get("X-File-ID")
        .and_then(|h| h.to_str().ok()) {
            Some(id) => id.to_string(),
            None => {
                error!("Missing file ID");
                return HttpResponse::BadRequest().body("Missing file ID");
            }
        };

    let start_offset = match req.headers()
        .get("X-Start-Offset")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<u64>().ok()) {
            Some(offset) => offset,
            None => {
                error!("Missing or invalid start offset");
                return HttpResponse::BadRequest().body("Missing or invalid start offset");
            }
        };

    let record = match query!(
        "SELECT filename, checksum, total_size FROM upload_file_meta WHERE id = ?",
        file_id
    )
    .fetch_one(&data.db_pool)
    .await
    {
        Ok(record) => record,
        Err(e) => {
            error!("Failed to fetch file record: {}", e);
            return HttpResponse::InternalServerError().body("Failed to fetch file record");
        }
    };

    let safe_filename = sanitize(&record.filename);
    let total_size = record.total_size as u64;

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

    let (start_pos, end_pos) = match content_range {
        Some(range) => {
            let parts: Vec<&str> = range.split('/').next()
                .unwrap_or("bytes 0-0")
                .split('-')
                .collect();
            let start = parts[0].replace("bytes ", "").parse::<u64>().unwrap_or(0);
            let end = parts.get(1).and_then(|&s| s.parse::<u64>().ok()).unwrap_or(total_size - 1);
            (start, end)
        },
        None => (0u64, total_size - 1)
    };

    // 分片文件路径
    let chunk_file_path = format!("uploads/{}_chunk_{}", safe_filename,start_offset);

    let mut file = match OpenOptions::new()
        .create(true)
        .write(true)
        .open(&chunk_file_path)
        .await {
            Ok(f) => f,
            Err(e) => {
                error!("File error: {}", e);
                return HttpResponse::InternalServerError().body(format!("File error: {}", e));
            }
        };

    let mut hasher = Sha256::new();
    let mut uploaded_size = 0;

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

        let checksum = format!("{:x}", hasher.clone().finalize());

        // 更新上传进度表，仅更新 uploaded_size 和 checksum
        if let Err(e) = query!(
            "UPDATE upload_progress SET uploaded_size = ?, checksum = ? WHERE file_id = ? AND start_offset = ?",
            uploaded_size,
            checksum,
            file_id,
            start_offset
        )
        .execute(&data.db_pool)
        .await
        {
            error!("Failed to update upload progress: {}", e);
            return HttpResponse::InternalServerError().body(format!("Failed to update upload progress: {}", e));
        }
    }

    // 检查所有分片是否上传完成
    let total_uploaded: u64 = query!(
        "SELECT SUM(uploaded_size) as total_uploaded FROM upload_progress WHERE file_id = ?",
        file_id
    )
    .fetch_one(&data.db_pool)
    .await
    .map(|row| row.total_uploaded.unwrap_or_else(|| BigDecimal::from(0)).to_u64().unwrap_or(0))
    .unwrap_or(0);

    if total_uploaded >= total_size {
        // 更新文件状态为处理中
        if let Err(e) = query!(
            "UPDATE upload_file_meta SET status = 1 WHERE id = ? AND status = 0",
            file_id
        )
        .execute(&data.db_pool)
        .await
        {
            error!("Failed to update file status to processing: {}", e);
            return HttpResponse::InternalServerError().body(format!("Failed to update file status: {}", e));
        }

        // 组合分片文件为完整文件
        let final_file_path = format!("uploads/{}", safe_filename);
        let mut final_file = match OpenOptions::new()
            .create(true)
            .write(true)
            .open(&final_file_path)
            .await
        {
            Ok(file) => file,
            Err(e) => {
                error!("Failed to create final file: {}", e);
                return HttpResponse::InternalServerError().body("Failed to create final file");
            }
        };

        for start in (0..total_size).step_by(1024 * 1024) { // 假设每个分片大小为1MB
            let chunk_file_path = format!("uploads/{}_chunk_{}", safe_filename, start);
            let mut chunk_file = match OpenOptions::new()
                .read(true)
                .open(&chunk_file_path)
                .await
            {
                Ok(file) => file,
                Err(e) => {
                    error!("Failed to open chunk file: {}", e);
                    return HttpResponse::InternalServerError().body("Failed to open chunk file");
                }
            };

            if let Err(e) = tokio::io::copy(&mut chunk_file, &mut final_file).await {
                error!("Failed to copy chunk to final file: {}", e);
                return HttpResponse::InternalServerError().body("Failed to copy chunk to final file");
            }

            // 删除分片文件
            if let Err(e) = fs::remove_file(&chunk_file_path).await {
                error!("Failed to delete chunk file: {}", e);
                return HttpResponse::InternalServerError().body("Failed to delete chunk file");
            }
        }

        // 更新文件状态为已完成
        if let Err(e) = query!(
            "UPDATE upload_file_meta SET status = 2 WHERE id = ? AND status = 1",
            file_id
        )
        .execute(&data.db_pool)
        .await
        {
            error!("Failed to update file status to completed: {}", e);
            return HttpResponse::InternalServerError().body(format!("Failed to update file status: {}", e));
        }

        // 获取最终文件大小
        let final_file_size = match fs::metadata(&final_file_path).await {
            Ok(metadata) => metadata.len(),
            Err(e) => {
                error!("Failed to get final file size: {}", e);
                return HttpResponse::InternalServerError().body("Failed to get final file size");
            }
        };

        let final_checksum = format!("{:x}", hasher.finalize());

        HttpResponse::Ok()
            .content_type("application/json")
            .json(json!({
                "status": "success",
                "filename": safe_filename,
                "size": final_file_size,  // 使用最终文件大小
                "checksum": final_checksum
            }))
    } else {
        let final_checksum = format!("{:x}", hasher.finalize());

        HttpResponse::Ok()
            .content_type("application/json")
            .json(json!({
                "status": "range_success",
                "filename": safe_filename,
                "size": uploaded_size,
                "checksum": final_checksum
            }))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMetadata {
    pub filename: String,
    pub file_id: u64,
    pub total_size: u64,
}

pub async fn submit_file_metadata(
    metadata: web::Json<FileMetadata>,
    data: web::Data<Arc<AppState>>,
) -> HttpResponse {
    if let Err(response) = check_system_initialized(&data.db_pool).await {
        return response;
    }

    let safe_filename = sanitize(&metadata.filename);

    let unique_id = Uuid::new_v4().to_string();

    let mut uploads = data.uploads.lock().await;
    uploads.insert(safe_filename.clone(), UploadState {
        id: unique_id.clone(),
        filename: safe_filename.clone(),
        total_size: metadata.total_size,
        checksum: String::new(),
    });

    // 获取分片大小配置
    let chunk_size: u64 = match query!(
        "SELECT config_value FROM system_config WHERE config_key = 'chunk_size'"
    )
    .fetch_one(&data.db_pool)
    .await
    {
        Ok(row) => row.config_value.parse().unwrap_or(1048576), // 默认1MB
        Err(e) => {
            error!("Failed to fetch chunk size: {}", e);
            return HttpResponse::InternalServerError().body("Failed to fetch chunk size");
        }
    };

    // 计算分片数量并初始化 upload_progress 表
    let num_chunks = (metadata.total_size + chunk_size - 1) / chunk_size;
    for i in 0..num_chunks {
        let start_offset = i * chunk_size;
        let end_offset = ((i + 1) * chunk_size).min(metadata.total_size) - 1;

        if let Err(e) = query!(
            "INSERT INTO upload_progress (file_id, checksum, filename, total_size, uploaded_size, start_offset, end_offset) VALUES (?, ?, ?, ?, ?, ?, ?)",
            metadata.file_id, // 使用 unique_id 作为 file_id
            "", // 初始时没有checksum
            safe_filename,
            metadata.total_size,
            0, // 初始上传大小为0
            start_offset,
            end_offset
        )
        .execute(&data.db_pool)
        .await
        {
            error!("Failed to initialize upload progress: {}", e);
            return HttpResponse::InternalServerError().body("Failed to initialize upload progress");
        }
    }

    // 保存到数据库
    let upload_state = uploads.get(&safe_filename).unwrap();
    upload_state.save_to_db(&data.db_pool).await.unwrap();

    HttpResponse::Ok().json(json!({
        "message": "Metadata submitted successfully",
        "id": unique_id
    }))
}
