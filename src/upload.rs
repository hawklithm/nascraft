use actix_web::{web, HttpRequest, HttpResponse, Result};
use futures::StreamExt;
use sha2::{Sha256, Digest as ShaDigest};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, AsyncReadExt};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::json;
use log::{error, info};
use sanitize_filename::sanitize;
use uuid::Uuid;
use sqlx::{MySqlPool, Transaction, MySql};
use crate::init_env::check_system_initialized;
use crate::upload_dao::{fetch_file_record, update_upload_progress, get_total_uploaded, update_file_status_and_path, fetch_chunk_size, initialize_upload_progress, save_upload_state_to_db, fetch_uploaded_files, fetch_total_uploaded_files,  fetch_upload_progress};
use chrono::Utc;
use md5::Md5;

#[derive(Debug)]
pub struct AppState {
    pub uploads: Mutex<HashMap<String, UploadState>>,
    pub db_pool: Option<MySqlPool>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            uploads: Mutex::new(HashMap::new()),
            db_pool: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UploadState {
    pub id: String,
    pub filename: String,
    pub total_size: u64,
    pub checksum: String,
}

impl UploadState {
    pub async fn save_to_db(&self, tx: &mut Transaction<'_, MySql>, file_path: &str) -> Result<(), String> {
        save_upload_state_to_db(tx, &self.id, &self.filename, self.total_size, &self.checksum, file_path).await
    }
}

#[derive(Debug, Serialize)]
struct ApiResponse<T> {
    message: String,
    status: i32,
    code: String,
    data: Option<T>,
}

impl<T> ApiResponse<T> {
    fn success(message: &str, data: T) -> Self {
        Self {
            message: message.to_string(),
            status: 1,
            code: "0".to_string(),
            data: Some(data),
        }
    }

    fn error(message: &str, code: &str) -> Self {
        Self {
            message: message.to_string(),
            status: 0,
            code: code.to_string(),
            data: None,
        }
    }
}

pub async fn upload_file(
    req: HttpRequest,
    mut payload: web::Payload,
    data: web::Data<Arc<AppState>>,
) -> HttpResponse {
    let db_pool = data.db_pool.as_ref().unwrap();
    if let Err(_) = check_system_initialized(db_pool).await {
        return HttpResponse::BadRequest().json(ApiResponse::<()>::error(
            "System not initialized",
            "SYSTEM_NOT_INITIALIZED"
        ));
    }


    let file_id = match req.headers()
        .get("X-File-ID")
        .and_then(|h| h.to_str().ok()) {
            Some(id) => id.to_string(),
            None => {
                return HttpResponse::BadRequest().json(ApiResponse::<()>::error(
                    "Missing file ID",
                    "MISSING_FILE_ID"
                ));
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

    let (filename, _, total_size, _, _) = match fetch_file_record(db_pool, &file_id).await {
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

    let (start_pos, _end_pos) = match content_range {
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
    if let Err(e) = file.seek(tokio::io::SeekFrom::Start(start_pos-start_offset)).await {
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
        info!("file_id: {}, uploaded_size: {}, bytes_to_write: {},start_offset: {}, start_pos: {}, content_length: {}", file_id, uploaded_size, bytes_to_write, start_offset, start_pos, content_length);

        let checksum = format!("{:x}", hasher.clone().finalize());

        // 更新上传进度表，仅更新 uploaded_size 和 checksum
        if let Err(e) = update_upload_progress(db_pool, uploaded_size-start_pos, &checksum, &file_id, start_offset).await {
            return HttpResponse::InternalServerError().body(e);
        }

        // 如果已经写入了足够的字节数，退出循环
        if uploaded_size - start_pos >= content_length {
            break;
        }
    }

    // Log successful chunk upload
    info!("Chunk uploaded successfully for file ID: {}, start_offset: {}", file_id, start_offset);

    // 检查所有分片是否上传完成
    let total_uploaded = match get_total_uploaded(db_pool, &file_id).await {
        Ok(size) => size,
        Err(e) => return HttpResponse::InternalServerError().body(e),
    };

    if total_uploaded >= total_size {
        // 更新文件状态为处理中
        if let Err(e) = update_file_status_and_path(db_pool, &file_id, 0, 1, "").await {
            return HttpResponse::InternalServerError().body(e);
        }

        // 组合分片文件为完整文件
        let final_file_path = format!("uploads/{}", safe_filename);
        if let Err(e) = merge_chunks(&safe_filename, total_size).await {
            return HttpResponse::InternalServerError().body(e);
        }

        // Log successful merge
        info!("Chunks merged successfully for file ID: {}", file_id);

        // 计算合并后文件的 MD5 哈希值
        let mut file = match OpenOptions::new().read(true).open(&final_file_path).await {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open final file for hashing: {}", e);
                return HttpResponse::InternalServerError().body("Failed to open final file for hashing");
            }
        };

        let mut hasher = Md5::new();
        let mut buffer = [0; 1024];
        loop {
            let n = match file.read(&mut buffer).await {
                Ok(n) if n == 0 => break,
                Ok(n) => n,
                Err(e) => {
                    error!("Failed to read final file for hashing: {}", e);
                    return HttpResponse::InternalServerError().body("Failed to read final file for hashing");
                }
            };
            hasher.update(&buffer[..n]);
        }
        let calculated_md5 = format!("{:x}", hasher.finalize());

        // 从数据库中获取预期的哈希值
        let (_, expected_md5, _, _, _) = match fetch_file_record(db_pool, &file_id).await {
            Ok(record) => record,
            Err(e) => return HttpResponse::InternalServerError().body(e),
        };

        // 比较哈希值
        if calculated_md5 != expected_md5 {
            return HttpResponse::InternalServerError().body("File is corrupted: MD5 hash mismatch");
        }

        // Log successful checksum validation
        info!("Checksum validated successfully for file ID: {}", file_id);

        // 更新文件状态为已完成并更新文件路径
        if let Err(e) = update_file_status_and_path(db_pool, &file_id, 1, 2, &final_file_path).await {
            return HttpResponse::InternalServerError().body(e);
        }

        HttpResponse::Ok().json(ApiResponse::success(
            "File upload completed successfully",
            json!({
                "status": "success",
                "filename": safe_filename,
                "size": total_size,
                "checksum": calculated_md5
            })
        ))
    } else {
        let final_checksum = format!("{:x}", hasher.finalize());

        HttpResponse::Ok().json(ApiResponse::success(
            "Chunk upload successful",
            json!({
                "status": "range_success",
                "filename": safe_filename,
                "size": uploaded_size,
                "checksum": final_checksum
            })
        ))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMetadata {
    pub filename: String,
    pub total_size: u64,
    pub checksum: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChunkInfo {
    pub start_offset: u64,
    pub end_offset: u64,
    pub chunk_size: u64,
}

pub async fn submit_file_metadata(
    metadata: web::Json<FileMetadata>,
    data: web::Data<Arc<AppState>>,
) -> HttpResponse {
    let db_pool = data.db_pool.as_ref().unwrap();
    if let Err(_) = check_system_initialized(db_pool).await {
        return HttpResponse::BadRequest().json(ApiResponse::<()>::error(
            "System not initialized",
            "SYSTEM_NOT_INITIALIZED"
        ));
    }

    let safe_filename = sanitize(&metadata.filename);
    let unique_id = Uuid::new_v4().to_string();
    let file_id = unique_id.clone();

    let mut uploads = data.uploads.lock().await;
    let upload_state = UploadState {
        id: unique_id.clone(),
        filename: safe_filename.clone(),
        total_size: metadata.total_size,
        checksum: metadata.checksum.clone(),
    };

    // Start a transaction
    let mut tx = match db_pool.begin().await {
        Ok(transaction) => transaction,
        Err(e) => {
            error!("Failed to begin transaction: {}", e);
            return HttpResponse::InternalServerError().body("Failed to begin transaction");
        }
    };

    // Save to database
    if let Err(e) = upload_state.save_to_db(&mut tx, "").await {
        tx.rollback().await.unwrap_or_else(|e| error!("Failed to rollback transaction: {}", e));
        return HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
            &e,
            "DB_SAVE_ERROR"
        ));
    }

    // Get chunk size configuration
    let chunk_size = match fetch_chunk_size(db_pool).await {
        Ok(size) => size,
        Err(e) => {
            tx.rollback().await.unwrap_or_else(|e| error!("Failed to rollback transaction: {}", e));
            return HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
                &e,
                "FETCH_CHUNK_SIZE_ERROR"
            ));
        }
    };

    // Calculate number of chunks and initialize upload_progress table
    let num_chunks = (metadata.total_size + chunk_size - 1) / chunk_size;
    let mut chunks = Vec::new();

    for i in 0..num_chunks {
        let start_offset = i * chunk_size;
        let end_offset = ((i + 1) * chunk_size).min(metadata.total_size)-1;
            let chunk_size= end_offset - start_offset+1;

        if let Err(e) = initialize_upload_progress(&mut tx, &file_id, &safe_filename, chunk_size, start_offset, end_offset).await {
            tx.rollback().await.unwrap_or_else(|e| error!("Failed to rollback transaction: {}", e));
            return HttpResponse::InternalServerError().body(e);
        }

        chunks.push(ChunkInfo {
            start_offset,
            end_offset,
            chunk_size,
        });
    }

    // Commit the transaction
    if let Err(e) = tx.commit().await {
        error!("Failed to commit transaction: {}", e);
        return HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
            &e.to_string(),
            "COMMIT_TRANSACTION_ERROR"
        ));
    }

    // Save to in-memory state
    uploads.insert(safe_filename.clone(), upload_state);

    HttpResponse::Ok().json(ApiResponse::success(
        "Metadata submitted successfully",
        json!({
            "id": file_id,
            "filename": safe_filename,
            "total_size": metadata.total_size,
            "chunk_size": chunk_size,
            "total_chunks": num_chunks,
            "chunks": chunks
        })
    ))
}

// 新增辅助函数
async fn merge_chunks(filename: &str, total_size: u64) -> Result<(), String> {
    let final_file_path = format!("uploads/{}", filename);
    let mut final_file = match OpenOptions::new()
        .create(true)
        .write(true)
        .open(&final_file_path)
        .await {
            Ok(file) => file,
            Err(e) => {
                error!("Failed to create final file: {}", e);
                return Err("Failed to create final file".to_string());
            }
        };

    for start in (0..total_size).step_by(1024 * 1024) {
        let chunk_file_path = format!("uploads/{}_chunk_{}", filename, start);
        let mut chunk_file = match OpenOptions::new()
            .read(true)
            .open(&chunk_file_path)
            .await {
                Ok(file) => file,
                Err(e) => {
                    error!("Failed to open chunk file: {}", e);
                    return Err("Failed to open chunk file".to_string());
                }
            };

        if let Err(e) = tokio::io::copy(&mut chunk_file, &mut final_file).await {
            error!("Failed to copy chunk to final file: {}", e);
            return Err("Failed to copy chunk to final file".to_string());
        }

        if let Err(e) = fs::remove_file(&chunk_file_path).await {
            error!("Failed to delete chunk file: {}", e);
            return Err("Failed to delete chunk file".to_string());
        }
    }

    Ok(())
}

#[derive(Deserialize)]
pub struct Pagination {
    page: u32,
    page_size: u32,
    status: Option<i32>,
    sort_by: Option<String>,
    order: Option<String>,
}

pub async fn get_uploaded_files(
    data: web::Data<Arc<AppState>>,
    query: web::Query<Pagination>,
) -> HttpResponse {
    let page = query.page;
    let page_size = query.page_size;
    let status = query.status;
    let sort_by = query.sort_by.as_deref().unwrap_or("id");
    let order = query.order.as_deref().unwrap_or("asc");


    let db_pool = data.db_pool.as_ref().unwrap();

    let total_files = match fetch_total_uploaded_files(db_pool, status).await {
        Ok(total) => total,
        Err(e) => return HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
            &e,
            "FETCH_TOTAL_FILES_ERROR",
        )),
    };

    match fetch_uploaded_files(db_pool, page, page_size, status, sort_by, order).await {
        Ok(files) => HttpResponse::Ok().json(ApiResponse::success(
            "Fetched uploaded files successfully",
            json!({
                "total_files": total_files,
                "files": files
            }),
        )),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
            &e,
            "FETCH_FILES_ERROR",
        )),
    }
}

pub async fn get_upload_status(
    data: web::Data<Arc<AppState>>,
    file_id: web::Path<String>,
) -> HttpResponse {
    let file_id_str = file_id.into_inner();

    let db_pool = data.db_pool.as_ref().unwrap();

    // Fetch file record to get the current status
    let (_filename, _, _, status, _) = match fetch_file_record(db_pool, &file_id_str).await {
        Ok(record) => record,
        Err(e) => return HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
            &e,
            "FETCH_FILE_RECORD_ERROR",
        )),
    };

    // If status is 1 (processing) or 2 (completed), return it directly
    let status_str = match status {
        1 => "processing",
        2 => "completed",
        _ => {
            // Fetch upload progress for each chunk
            let chunk_progress = match fetch_upload_progress(db_pool, &file_id_str).await {
                Ok(progress) => progress,
                Err(e) => return HttpResponse::InternalServerError().json(ApiResponse::<()>::error(
                    &e,
                    "FETCH_PROGRESS_ERROR",
                )),
            };

            // Determine overall status
            let now = Utc::now().timestamp();
            let is_paused = chunk_progress.iter().all(|chunk| {
                now - chunk.last_updated > 60 // Check if last updated is more than 60 seconds ago
            });

            if is_paused {
                "paused"
            } else {
                "uploading"
            }
        }
    };

    // Prepare the response
    let mut response_data = json!({
        "file_id": file_id_str,
        "status": status_str,
    });

    // Include chunk information only if status is not processing or completed
    if status_str != "processing" && status_str != "completed" {
        let chunk_progress = fetch_upload_progress(db_pool, &file_id_str).await.unwrap_or_default();
        response_data["chunks"] = json!(chunk_progress);
    }

    HttpResponse::Ok().json(ApiResponse::success(
        "Fetched upload status successfully",
        response_data,
    ))
}

