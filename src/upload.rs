use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use futures::StreamExt;
use sha2::{Sha256, Digest as ShaDigest};
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncSeekExt, AsyncWriteExt, AsyncReadExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use log::{error, info};
use sanitize_filename::sanitize;
use uuid::Uuid;
use sqlx::{SqlitePool, Transaction, Sqlite};
use crate::init_env::check_system_initialized;
use crate::upload_dao::{fetch_file_record, update_upload_progress, get_total_uploaded, update_file_status_and_path, fetch_chunk_size, initialize_upload_progress, save_upload_state_to_db, fetch_uploaded_files, fetch_total_uploaded_files,  fetch_upload_progress, fetch_file_by_checksum, fetch_incomplete_by_checksum, fetch_completed_chunk_offsets, update_file_status_cas, update_file_meta_info, fetch_file_chunk_size, resolve_chunk_size, clear_upload_progress};
use chrono::Utc;
use md5::Md5;
use crate::context::AppContext;
use crate::thumbnail::{is_image_file, is_video_file, generate_thumbnail, generate_video_thumbnail, ThumbnailConfig};
use crate::upload_dao::update_file_thumbnail_path;
use crate::upload_dao::fetch_distinct_source_devices;
use crate::exif_parser::parse_and_store_taken_at;

#[derive(Debug)]
pub struct AppState {
    pub db_pool: SqlitePool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            db_pool: SqlitePool::connect_lazy("sqlite::memory:").expect("failed to create default sqlite pool"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UploadState {
    pub id: String,
    pub filename: String,
    pub total_size: u64,
    pub checksum: String,
    pub source_device: Option<String>,
    pub chunk_size: u64,
}

impl UploadState {
    pub async fn save_to_db(&self, tx: &mut Transaction<'_, Sqlite>, file_path: &str) -> Result<(), String> {
        save_upload_state_to_db(tx, &self.id, &self.filename, self.total_size, &self.checksum, self.source_device.as_deref(), file_path, self.chunk_size).await
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
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    body: Body,
) -> impl IntoResponse {
    let db_pool = &ctx.app_state.db_pool;
    if let Err(_) = check_system_initialized(db_pool).await {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::error(
            "System not initialized",
            "SYSTEM_NOT_INITIALIZED"
        ))).into_response();
    }


    let file_id = match headers
        .get("X-File-ID")
        .and_then(|h| h.to_str().ok()) {
            Some(id) => id.to_string(),
            None => {
                return (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::error(
                    "Missing file ID",
                    "MISSING_FILE_ID"
                ))).into_response();
            }
        };

    let start_offset = match headers
        .get("X-Start-Offset")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<u64>().ok()) {
            Some(offset) => offset,
            None => {
                error!("Missing or invalid start offset");
                return (StatusCode::BAD_REQUEST, "Missing or invalid start offset").into_response();
            }
        };

    let (filename, _, total_size, _, _) = match fetch_file_record(db_pool, &file_id).await {
        Ok(record) => record,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    let safe_filename = sanitize(&filename);
    let total_size = total_size as u64;

    let content_length = match headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.parse::<u64>().ok()) {
            Some(len) => len,
            None => {
                error!("Invalid content length");
                return (StatusCode::BAD_REQUEST, "Invalid content length").into_response();
            }
        };

    let content_range = headers
        .get(axum::http::header::CONTENT_RANGE)
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
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("File error: {}", e)).into_response();
            }
        };

    // 移动文件指针到 start_pos
    if let Err(e) = file.seek(tokio::io::SeekFrom::Start(start_pos-start_offset)).await {
        error!("Failed to seek file: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to seek file: {}", e)).into_response();
    }

    let mut hasher = Sha256::new();
    let mut uploaded_size = start_pos;

    let mut payload = body.into_data_stream();
    while let Some(chunk) = payload.next().await {
        let chunk = chunk.map_err(|e| {
            error!("Payload error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Payload error: {}", e)).into_response()
        });
        let chunk = match chunk {
            Ok(c) => c,
            Err(resp) => return resp,
        };

        // 计算剩余需要写入的字节数
        let remaining_bytes = content_length.saturating_sub(uploaded_size - start_pos);
        let bytes_to_write = chunk.len().min(remaining_bytes as usize);

        if let Err(e) = file.write_all(&chunk[..bytes_to_write]).await {
            error!("Write error: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("Write error: {}", e)).into_response();
        }
        hasher.update(&chunk[..bytes_to_write]);
        uploaded_size += bytes_to_write as u64;
        info!("file_id: {}, uploaded_size: {}, bytes_to_write: {},start_offset: {}, start_pos: {}, content_length: {}", file_id, uploaded_size, bytes_to_write, start_offset, start_pos, content_length);

        let checksum = format!("{:x}", hasher.clone().finalize());

        // 更新上传进度表，仅更新 uploaded_size 和 checksum
        if let Err(e) = update_upload_progress(db_pool, uploaded_size-start_pos, &checksum, &file_id, start_offset).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
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
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    if total_uploaded >= total_size {
        // CAS：只有把 status 从 0 改成 1 成功的请求负责合并，避免并发最后分片双合并
        let acquired = match update_file_status_cas(db_pool, &file_id, 0, 1).await {
            Ok(acquired) => acquired,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        };
        if !acquired {
            info!("File {} is being finalized by another request, skipping merge", file_id);
            return (StatusCode::OK, Json(ApiResponse::success(
                "Upload already finalizing",
                json!({
                    "status": "finalizing",
                    "filename": safe_filename,
                    "skipped": true
                })
            ))).into_response();
        }

        // 组合分片文件为完整文件（分片偏移按该文件持久化的 chunk_size 生成，合并必须用同一大小步进）
        let chunk_size = match fetch_file_chunk_size(db_pool, &file_id).await {
            Ok(size) if size > 0 => size,
            // 旧记录未存 chunk_size，回退到系统默认配置
            Ok(_) => fetch_chunk_size(db_pool).await.unwrap_or(1024 * 1024),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        };
        let final_file_path = format!("uploads/{}", safe_filename);
        if let Err(e) = merge_chunks(&safe_filename, total_size, chunk_size).await {
            error!("Failed to merge chunks for {}: {}", file_id, e);
            rollback_finalize(db_pool, &file_id, &final_file_path).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }

        // Log successful merge
        info!("Chunks merged successfully for file ID: {}", file_id);

        // 计算合并后文件的 MD5 哈希值
        let mut file = match OpenOptions::new().read(true).open(&final_file_path).await {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open final file for hashing: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to open final file for hashing").into_response();
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
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read final file for hashing").into_response();
                }
            };
            hasher.update(&buffer[..n]);
        }
        let calculated_md5 = format!("{:x}", hasher.finalize());

        // 从数据库中获取预期的哈希值
        let (_, expected_md5, _, _, _) = match fetch_file_record(db_pool, &file_id).await {
            Ok(record) => record,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        };

        // 比较哈希值
        if calculated_md5 != expected_md5 {
            error!("MD5 mismatch for file_id {} (expected {}), rolling back", file_id, expected_md5);
            rollback_finalize(db_pool, &file_id, &final_file_path).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, "File is corrupted: MD5 hash mismatch").into_response();
        }

        // Log successful checksum validation
        info!("Checksum validated successfully for file ID: {}", file_id);

        // 获取文件元信息
        let file_metadata = match fs::metadata(&final_file_path).await {
            Ok(meta) => meta,
            Err(e) => {
                error!("Failed to get file metadata: {}", e);
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to get file metadata: {}", e)).into_response();
            }
        };

        let file_mtime = file_metadata.modified()
            .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
            .unwrap_or(0);
        let file_ctime = file_metadata.created()
            .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
            .unwrap_or(file_mtime);

        // 获取inode（仅Unix-like系统；Windows无inode，记为0）
        #[cfg(unix)]
        let file_ino = std::fs::metadata(&final_file_path)
            .ok()
            .and_then(|m| std::os::unix::fs::MetadataExt::ino(&m).try_into().ok())
            .unwrap_or(0i64);
        #[cfg(not(unix))]
        let file_ino = 0i64;

        // 更新文件元信息
        if let Err(e) = update_file_meta_info(db_pool, &file_id, file_mtime, file_ctime, file_ino).await {
            error!("Failed to update file meta info: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }

        // 更新文件状态为已完成并更新文件路径
        if let Err(e) = update_file_status_and_path(db_pool, &file_id, 1, 2, &final_file_path).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }

        // Generate thumbnail if this is an image or video file
        if is_image_file(&safe_filename) {
            let config = ThumbnailConfig::default();
            if let Some(thumbnail_path) = generate_thumbnail(&config, &final_file_path, &calculated_md5).await {
                if let Err(e) = update_file_thumbnail_path(db_pool, &file_id, &thumbnail_path).await {
                    error!("Failed to save thumbnail path to database: {}", e);
                    // Don't fail the upload if thumbnail generation fails
                }
            }
        } else if is_video_file(&safe_filename) {
            // 视频缩略图（ffmpeg 提取第一帧）放后台执行，不阻塞上传完成响应
            let config = ThumbnailConfig::default();
            let pool = ctx.app_state.db_pool.clone();
            let fid = file_id.clone();
            let final_path = final_file_path.clone();
            let md5 = calculated_md5.clone();
            tokio::spawn(async move {
                if let Some(thumbnail_path) = generate_video_thumbnail(&config, &final_path, &md5).await {
                    if let Err(e) = update_file_thumbnail_path(&pool, &fid, &thumbnail_path).await {
                        error!("Failed to save video thumbnail path: {}", e);
                    }
                }
            });
        }

        // 异步解析 EXIF 拍摄时间（不阻塞上传响应；后台 worker 也会定时兜底）
        {
            let pool = ctx.app_state.db_pool.clone();
            let fid = file_id.clone();
            tokio::spawn(async move {
                parse_and_store_taken_at(&pool, &fid).await;
            });
        }

        (StatusCode::OK, Json(ApiResponse::success(
            "File upload completed successfully",
            json!({
                "status": "success",
                "filename": safe_filename,
                "size": total_size,
                "checksum": calculated_md5
            })
        ))).into_response()
    } else {
        let final_checksum = format!("{:x}", hasher.finalize());

        (StatusCode::OK, Json(ApiResponse::success(
            "Chunk upload successful",
            json!({
                "status": "range_success",
                "filename": safe_filename,
                "size": uploaded_size,
                "checksum": final_checksum
            })
        ))).into_response()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMetadata {
    pub filename: String,
    pub total_size: u64,
    pub checksum: String,
    #[serde(default)]
    pub source_device: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChunkInfo {
    pub start_offset: u64,
    pub end_offset: u64,
    pub chunk_size: u64,
}

/// 根据文件大小和分片大小计算分片列表（与 initialize_upload_progress 的偏移规则保持一致）
fn compute_chunks(total_size: u64, chunk_size: u64) -> Vec<ChunkInfo> {
    if chunk_size == 0 {
        return Vec::new();
    }
    let num_chunks = (total_size + chunk_size - 1) / chunk_size;
    let mut chunks = Vec::with_capacity(num_chunks as usize);
    for i in 0..num_chunks {
        let start_offset = i * chunk_size;
        let end_offset = ((i + 1) * chunk_size).min(total_size) - 1;
        let chunk_size = end_offset - start_offset + 1;
        chunks.push(ChunkInfo {
            start_offset,
            end_offset,
            chunk_size,
        });
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_chunks_even_split() {
        let chunks = compute_chunks(8, 4);
        assert_eq!(chunks.len(), 2);
        assert_eq!((chunks[0].start_offset, chunks[0].end_offset, chunks[0].chunk_size), (0, 3, 4));
        assert_eq!((chunks[1].start_offset, chunks[1].end_offset, chunks[1].chunk_size), (4, 7, 4));
    }

    #[test]
    fn compute_chunks_last_chunk_truncated() {
        let chunks = compute_chunks(10, 4);
        assert_eq!(chunks.len(), 3);
        assert_eq!((chunks[2].start_offset, chunks[2].end_offset, chunks[2].chunk_size), (8, 9, 2));
    }

    #[test]
    fn compute_chunks_single_chunk() {
        let chunks = compute_chunks(5, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!((chunks[0].start_offset, chunks[0].end_offset, chunks[0].chunk_size), (0, 4, 5));
    }

    #[test]
    fn compute_chunks_empty_and_zero_size() {
        assert!(compute_chunks(0, 4).is_empty());
        assert!(compute_chunks(10, 0).is_empty());
    }
}

pub async fn submit_file_metadata(
    State(ctx): State<AppContext>,
    Json(metadata): Json<FileMetadata>,
) -> impl IntoResponse {
    let db_pool = &ctx.app_state.db_pool;
    if let Err(_) = check_system_initialized(db_pool).await {
        return (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::error(
            "System not initialized",
            "SYSTEM_NOT_INITIALIZED"
        ))).into_response();
    }

    let safe_filename = sanitize(&metadata.filename);

    // 检查文件是否已存在（基于 checksum 去重）
    match fetch_file_by_checksum(db_pool, &metadata.checksum).await {
        Ok(Some((existing_file_id, existing_filename, existing_file_path))) => {
            info!("File with checksum {} already exists (file_id: {}), skipping upload", metadata.checksum, existing_file_id);
            return (StatusCode::OK, Json(ApiResponse::success(
                "File already exists, upload skipped",
                json!({
                    "status": "duplicate",
                    "message": "File with same checksum already exists on server",
                    "id": existing_file_id,
                    "filename": existing_filename,
                    "file_path": existing_file_path,
                    "total_size": metadata.total_size,
                    "checksum": metadata.checksum,
                    "skipped": true
                })
            ))).into_response();
        }
        Ok(None) => {
            info!("File with checksum {} not found, proceeding with upload", metadata.checksum);

            // 查找同 checksum 的未完成上传记录：客户端重传时复用原记录，跳过已完成分片（断点续传）
            match fetch_incomplete_by_checksum(db_pool, &metadata.checksum).await {
                Ok(Some((existing_file_id, existing_total_size, existing_chunk_size, existing_status))) if existing_total_size == metadata.total_size as i64 => {
                    // status=1 表示之前已进入合并/校验阶段但失败或中断（僵尸）。
                    // 该状态既不在 completed 也不在 uploading 查询范围内，若不处理会重复建记录。
                    // 这里把僵尸重置为 status=0 并清空分片进度，让客户端从头重传。
                    if existing_status == 1 {
                        info!(
                            "Found stuck status=1 record for checksum {} (file_id {}), resetting to re-upload",
                            metadata.checksum, existing_file_id
                        );
                        if let Err(e) = clear_upload_progress(db_pool, &existing_file_id).await {
                            error!("Failed to clear progress of stuck record {}: {}", existing_file_id, e);
                        }
                        if let Err(e) = update_file_status_cas(db_pool, &existing_file_id, 1, 0).await {
                            error!("Failed to reset stuck record {} to status 0: {}", existing_file_id, e);
                        }
                    }

                    // 断点续传必须复用该文件原本的分片大小，否则分片偏移与已存进度错位
                    let chunk_size = if existing_chunk_size > 0 {
                        existing_chunk_size as u64
                    } else {
                        // 旧记录未持久化 chunk_size，回退到按文件名解析
                        let configured = fetch_chunk_size(db_pool).await.unwrap_or(1024 * 1024);
                        resolve_chunk_size(&safe_filename, configured)
                    };
                    let chunks = compute_chunks(metadata.total_size, chunk_size);
                    let uploaded_chunks = if existing_status == 1 {
                        // 僵尸已重置，进度已清空，客户端应重传全部分片
                        Vec::new()
                    } else {
                        fetch_completed_chunk_offsets(db_pool, &existing_file_id).await.unwrap_or_default()
                    };
                    info!(
                        "Resuming upload for checksum {} as file_id {} ({} of {} chunks already uploaded)",
                        metadata.checksum, existing_file_id, uploaded_chunks.len(), chunks.len()
                    );
                    return (StatusCode::OK, Json(ApiResponse::success(
                        "Resuming previous upload",
                        json!({
                            "status": "resume",
                            "id": existing_file_id,
                            "filename": safe_filename,
                            "total_size": metadata.total_size,
                            "checksum": metadata.checksum,
                            "chunk_size": chunk_size,
                            "total_chunks": chunks.len(),
                            "chunks": chunks,
                            "uploaded_chunks": uploaded_chunks,
                            "skipped": false
                        })
                    ))).into_response();
                }
                Ok(_) => {
                    info!("No resumable record for checksum {}, starting fresh upload", metadata.checksum);
                }
                Err(e) => {
                    error!("Failed to check resumable upload: {}", e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
                        "RESUME_CHECK_ERROR",
                        &e,
                    ))).into_response();
                }
            }
        }
        Err(e) => {
            error!("Failed to check file by checksum: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
                &e,
                "CHECKSUM_CHECK_ERROR"
            ))).into_response();
        }
    }

    let unique_id = Uuid::new_v4().to_string();
    let file_id = unique_id.clone();

    // Get chunk size configuration
    let configured_chunk_size = match fetch_chunk_size(db_pool).await {
        Ok(size) => size,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
                &e,
                "FETCH_CHUNK_SIZE_ERROR"
            ))).into_response();
        }
    };
    // 按文件类型解析分片大小：视频用更大分片，减少 HTTP 请求数与断点续传碎片
    let chunk_size = resolve_chunk_size(&safe_filename, configured_chunk_size);

    let upload_state = UploadState {
        id: unique_id.clone(),
        filename: safe_filename.clone(),
        total_size: metadata.total_size,
        checksum: metadata.checksum.clone(),
        source_device: metadata.source_device.clone(),
        chunk_size,
    };

    // Start a transaction
    let mut tx = match db_pool.begin().await {
        Ok(transaction) => transaction,
        Err(e) => {
            error!("Failed to begin transaction: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to begin transaction").into_response();
        }
    };

    // Save to database
    if let Err(e) = upload_state.save_to_db(&mut tx, "").await {
        tx.rollback().await.unwrap_or_else(|e| error!("Failed to rollback transaction: {}", e));
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
            &e,
            "DB_SAVE_ERROR"
        ))).into_response();
    }

    // 计算分片并初始化 upload_progress 表
    let chunks = compute_chunks(metadata.total_size, chunk_size);

    for chunk in &chunks {
        if let Err(e) = initialize_upload_progress(&mut tx, &file_id, &safe_filename, chunk.chunk_size, chunk.start_offset, chunk.end_offset).await {
            tx.rollback().await.unwrap_or_else(|e| error!("Failed to rollback transaction: {}", e));
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }
    }

    // Commit the transaction
    if let Err(e) = tx.commit().await {
        error!("Failed to commit transaction: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
            &e.to_string(),
            "COMMIT_TRANSACTION_ERROR"
        ))).into_response();
    }

    (StatusCode::OK, Json(ApiResponse::success(
        "Metadata submitted successfully",
        json!({
            "id": file_id,
            "filename": safe_filename,
            "total_size": metadata.total_size,
            "chunk_size": chunk_size,
            "total_chunks": chunks.len(),
            "chunks": chunks
        })
    ))).into_response()
}

/// 合并/校验失败时的回退：把文件状态从 status=1 回退到 0 并清空分片进度，
/// 删除残留的最终文件，让客户端下次重试时能从头重新上传，避免记录永久卡在 status=1。
async fn rollback_finalize(db_pool: &SqlitePool, file_id: &str, final_file_path: &str) {
    if let Err(e) = fs::remove_file(final_file_path).await {
        // 最终文件可能尚未生成，忽略 NotFound
        if e.kind() != std::io::ErrorKind::NotFound {
            error!("Failed to remove final file {} during rollback: {}", final_file_path, e);
        }
    }
    if let Err(e) = clear_upload_progress(db_pool, file_id).await {
        error!("Failed to clear progress during rollback for {}: {}", file_id, e);
    }
    if let Err(e) = update_file_status_cas(db_pool, file_id, 1, 0).await {
        error!("Failed to rollback status for {}: {}", file_id, e);
    }
}

async fn merge_chunks(filename: &str, total_size: u64, chunk_size: u64) -> Result<(), String> {
    if chunk_size == 0 {
        return Err("Invalid chunk size".to_string());
    }

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

    for start in (0..total_size).step_by(chunk_size as usize) {
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
    source_device: Option<String>,
    media_type: Option<String>,
}

pub async fn get_uploaded_files(
    State(ctx): State<AppContext>,
    Query(query): Query<Pagination>,
) -> impl IntoResponse {
    let page = query.page;
    let page_size = query.page_size;
    // 已上传文件列表默认只统计「已完成」（status=2）的记录，
    // 避免把上传中(0)/处理中(1)的僵尸记录混入总数与列表，导致数字对不齐。
    // 客户端仍可通过显式传 status 查询其他状态。
    let status = query.status.or(Some(2));
    let sort_by = query.sort_by.as_deref().unwrap_or("id");
    let order = query.order.as_deref().unwrap_or("asc");
    let source_device = query.source_device.as_deref();
    let media_type = query.media_type.as_deref();


    let db_pool = &ctx.app_state.db_pool;

    let (total_files, total_size) = match fetch_total_uploaded_files(db_pool, status, source_device, media_type).await {
        Ok((count, size)) => (count, size),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
            &e,
            "FETCH_TOTAL_FILES_ERROR",
        ))).into_response(),
    };

    match fetch_uploaded_files(db_pool, page, page_size, status, sort_by, order, source_device, media_type).await {
        Ok(mut files) => {
            // Add thumbnail_url for files that have a thumbnail
            // （缩略图路径为空字符串时也视为无缩略图，避免客户端拿到一个必然 404 的 URL）
            for file in &mut files {
                if file.thumbnail_path.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
                    file.thumbnail_url = Some(format!("/api/thumbnail/{}", file.file_id));
                }
            }

            (StatusCode::OK, Json(ApiResponse::success(
                "Fetched uploaded files successfully",
                json!({
                    "total_files": total_files,
                    "total_size": total_size,
                    "files": files
                }),
            ))).into_response()
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
            &e,
            "FETCH_FILES_ERROR",
        ))).into_response(),
    }
}

/// 返回所有去重后的来源设备（用于客户端来源筛选器）
pub async fn get_upload_sources(
    State(ctx): State<AppContext>,
) -> impl IntoResponse {
    let db_pool = &ctx.app_state.db_pool;

    match fetch_distinct_source_devices(db_pool).await {
        Ok(sources) => (StatusCode::OK, Json(ApiResponse::success(
            "Fetched source devices successfully",
            json!({ "sources": sources }),
        ))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
            &e,
            "FETCH_SOURCES_ERROR",
        ))).into_response(),
    }
}

pub async fn get_upload_status(
    State(ctx): State<AppContext>,
    Path(file_id_str): Path<String>,
) -> impl IntoResponse {
    let db_pool = &ctx.app_state.db_pool;

    // Fetch file record to get the current status
    let (_filename, _, _, status, _) = match fetch_file_record(db_pool, &file_id_str).await {
        Ok(record) => record,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
            &e,
            "FETCH_FILE_RECORD_ERROR",
        ))).into_response(),
    };

    // If status is 1 (processing) or 2 (completed), return it directly
    let status_str = match status {
        1 => "processing",
        2 => "completed",
        _ => {
            // Fetch upload progress for each chunk
            let chunk_progress = match fetch_upload_progress(db_pool, &file_id_str).await {
                Ok(progress) => progress,
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error(
                    &e,
                    "FETCH_PROGRESS_ERROR",
                ))).into_response(),
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

    (StatusCode::OK, Json(ApiResponse::success(
        "Fetched upload status successfully",
        response_data,
    ))).into_response()
}

