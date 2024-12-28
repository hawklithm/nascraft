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
use crate::init_env::{check_table_structure_endpoint, ensure_table_structure_endpoint, check_system_initialized};
use crate::upload_dao::{fetch_file_record, update_upload_progress, get_total_uploaded, update_file_status, fetch_chunk_size, initialize_upload_progress, save_upload_state_to_db};

#[derive(Debug)]
pub struct AppState {
    pub uploads: Mutex<HashMap<String, UploadState>>,
    pub db_pool: MySqlPool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UploadState {
    pub id: String,
    pub filename: String,
    pub total_size: u64,
    pub checksum: String,
}

impl UploadState {
    pub async fn save_to_db(&self, pool: &MySqlPool) -> Result<(), String> {
        save_upload_state_to_db(pool, &self.id, &self.filename, self.total_size, &self.checksum).await
    }
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

    let (filename, _, total_size) = match fetch_file_record(&data.db_pool, &file_id).await {
        Ok(record) => record,
        Err(e) => return HttpResponse::InternalServerError().body(e),
    };

    let safe_filename = sanitize(&filename);
    let total_size = total_size as u64;

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
            let end = parts.get(1).and_then(|&s| s.parse::<u64>().ok()).unwrap_or(start + content_length - 1);
            (start, end)
        },
        None => (0u64, content_length - 1)
    };

    // 分片文件路径
    let chunk_file_path = format!("uploads/{}_chunk_{}", safe_filename, start_offset);

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

    // 移动文件指针到 start_pos
    if let Err(e) = file.seek(tokio::io::SeekFrom::Start(start_pos)).await {
        error!("Failed to seek file: {}", e);
        return HttpResponse::InternalServerError().body(format!("Failed to seek file: {}", e));
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

        // 计算剩余需要写入的字节数
        let remaining_bytes = content_length.saturating_sub(uploaded_size - start_pos);
        let bytes_to_write = chunk.len().min(remaining_bytes as usize);

        if let Err(e) = file.write_all(&chunk[..bytes_to_write]).await {
            error!("Write error: {}", e);
            return HttpResponse::InternalServerError().body(format!("Write error: {}", e));
        }
        hasher.update(&chunk[..bytes_to_write]);
        uploaded_size += bytes_to_write as u64;

        // 如果已经写入了足够的字节数，退出循环
        if uploaded_size - start_pos >= content_length {
            break;
        }

        let checksum = format!("{:x}", hasher.clone().finalize());

        // 更新上传进度表，仅更新 uploaded_size 和 checksum
        if let Err(e) = update_upload_progress(&data.db_pool, uploaded_size, &checksum, &file_id, start_offset).await {
            return HttpResponse::InternalServerError().body(e);
        }
    }

    // 检查所有分片是否上传完成
    let total_uploaded = match get_total_uploaded(&data.db_pool, &file_id).await {
        Ok(size) => size,
        Err(e) => return HttpResponse::InternalServerError().body(e),
    };

    if total_uploaded >= total_size {
        // 更新文件状态为处理中
        if let Err(e) = update_file_status(&data.db_pool, &file_id, 0, 1).await {
            return HttpResponse::InternalServerError().body(e);
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
        if let Err(e) = update_file_status(&data.db_pool, &file_id, 1, 2).await {
            return HttpResponse::InternalServerError().body(e);
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
    let chunk_size = match fetch_chunk_size(&data.db_pool).await {
        Ok(size) => size,
        Err(e) => return HttpResponse::InternalServerError().body(e),
    };

    // 计算分片数量并初始化 upload_progress 表
    let num_chunks = (metadata.total_size + chunk_size - 1) / chunk_size;
    for i in 0..num_chunks {
        let start_offset = i * chunk_size;
        let end_offset = ((i + 1) * chunk_size).min(metadata.total_size) - 1;

        if let Err(e) = initialize_upload_progress(&data.db_pool, metadata.file_id, &safe_filename, metadata.total_size, start_offset, end_offset).await {
            return HttpResponse::InternalServerError().body(e);
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
