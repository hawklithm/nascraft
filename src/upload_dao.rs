use sqlx::{Sqlite, SqlitePool, Transaction, Row};
use log::{error, info};
use serde::Serialize;
use sqlx::FromRow;
use chrono;

pub async fn fetch_file_record(db_pool: &SqlitePool, file_id: &str) -> Result<(String, String, i64, i32, String), String> {
    match sqlx::query("SELECT filename, checksum, total_size, status, file_path, thumbnail_path FROM upload_file_meta WHERE file_id = ?")
        .bind(file_id)
        .fetch_one(db_pool)
        .await
    {
        Ok(row) => {
            let filename: String = row.get("filename");
            let checksum: String = row.get("checksum");
            let total_size: i64 = row.get("total_size");
            let status: Option<i32> = row.try_get("status").ok();
            let file_path: String = row.get("file_path");
            // We don't need thumbnail_path for this result type, just ignore it
            let _: Option<String> = row.try_get("thumbnail_path").ok();
            Ok((filename, checksum, total_size, status.unwrap_or(0), file_path))
        }
        Err(e) => {
            error!("Failed to fetch file record: {}", e);
            Err("Failed to fetch file record".to_string())
        }
    }
}

pub async fn update_upload_progress(db_pool: &SqlitePool, uploaded_size: u64, checksum: &str, file_id: &str, start_offset: u64) -> Result<(), String> {
    if let Err(e) = sqlx::query("UPDATE upload_progress SET uploaded_size = ?, checksum = ? WHERE file_id = ? AND start_offset = ?")
        .bind(uploaded_size as i64)
        .bind(checksum)
        .bind(file_id)
        .bind(start_offset as i64)
        .execute(db_pool)
        .await
    {
        error!("Failed to update upload progress: {}", e);
        return Err("Failed to update upload progress".to_string());
    }
    Ok(())
}

pub async fn get_total_uploaded(db_pool: &SqlitePool, file_id: &str) -> Result<u64, String> {
    match sqlx::query("SELECT COALESCE(SUM(uploaded_size), 0) as total_uploaded FROM upload_progress WHERE file_id = ?")
        .bind(file_id)
        .fetch_one(db_pool)
        .await
    {
        Ok(row) => {
            let total_uploaded: i64 = row.get("total_uploaded");
            Ok(total_uploaded.max(0) as u64)
        }
        Err(e) => {
            error!("Failed to get total uploaded size: {}", e);
            Err("Failed to get total uploaded size".to_string())
        }
    }
}

pub async fn update_file_status_and_path(
    db_pool: &SqlitePool,
    file_id: &str,
    current_status: i32,
    new_status: i32,
    file_path: &str,
) -> Result<(), String> {
    // Get current timestamp
    let current_time = chrono::Utc::now().timestamp();

    if let Err(e) = sqlx::query("UPDATE upload_file_meta SET status = ?, file_path = ?, last_updated = ? WHERE file_id = ? AND status = ?")
        .bind(new_status)
        .bind(file_path)
        .bind(current_time)
        .bind(file_id)
        .bind(current_status)
        .execute(db_pool)
        .await
    {
        error!("Failed to update file status and path: {}", e);
        return Err("Failed to update file status and path".to_string());
    }
    Ok(())
}

pub async fn fetch_chunk_size(db_pool: &SqlitePool) -> Result<u64, String> {
    match sqlx::query("SELECT config_value FROM system_config WHERE config_key = 'chunk_size'")
        .fetch_one(db_pool)
        .await
    {
        Ok(row) => {
            let config_value: String = row.get("config_value");
            config_value.parse().map_err(|_| "Invalid chunk size".to_string())
        }
        Err(e) => {
            error!("Failed to fetch chunk size: {}", e);
            Err("Failed to fetch chunk size".to_string())
        }
    }
}

pub async fn initialize_upload_progress(
    tx: &mut Transaction<'_, Sqlite>,
    file_id: &str,
    safe_filename: &str,
    total_size: u64,
    start_offset: u64,
    end_offset: u64,
) -> Result<(), String> {
    if let Err(e) = sqlx::query(
        "INSERT INTO upload_progress (file_id, checksum, filename, total_size, uploaded_size, start_offset, end_offset) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(file_id)
    .bind("") // Initial checksum is empty
    .bind(safe_filename)
    .bind(total_size as i64)
    .bind(0) // Initial uploaded size is 0
    .bind(start_offset as i64)
    .bind(end_offset as i64)
    .execute(&mut **tx)
    .await
    {
        error!("Failed to initialize upload progress: {}", e);
        return Err("Failed to initialize upload progress".to_string());
    }
    Ok(())
}

pub async fn save_upload_state_to_db(
    tx: &mut Transaction<'_, Sqlite>,
    file_id: &str,
    filename: &str,
    total_size: u64,
    checksum: &str,
    source_device: Option<&str>,
    file_path: &str,
) -> Result<(), String> {
    if let Err(e) = sqlx::query(
        "INSERT INTO upload_file_meta (file_id, filename, total_size, checksum, file_path, source_device, file_mtime, file_ctime, file_ino) VALUES (?, ?, ?, ?, ?, ?, 0, 0, 0)"
    )
    .bind(file_id)
    .bind(filename)
    .bind(total_size as i64)
    .bind(checksum)
    .bind(file_path)
    .bind(source_device)
    .execute(&mut **tx)
    .await
    {
        error!("Failed to save upload state: {}", e);
        return Err("Failed to save upload state".to_string());
    }
    info!("Successfully saved upload state for file '{}', ID: '{}'", filename, file_id);

    Ok(())
}

/// 更新文件元信息（文件系统元信息）
pub async fn update_file_meta_info(
    db_pool: &SqlitePool,
    file_id: &str,
    file_mtime: i64,
    file_ctime: i64,
    file_ino: i64,
) -> Result<(), String> {
    match sqlx::query(
        "UPDATE upload_file_meta SET file_mtime = ?, file_ctime = ?, file_ino = ?, last_updated = strftime('%s', 'now') WHERE file_id = ?"
    )
    .bind(file_mtime)
    .bind(file_ctime)
    .bind(file_ino)
    .bind(file_id)
    .execute(db_pool)
    .await
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Failed to update file meta info: {}", e)),
    }
}

#[derive(Debug, Serialize, FromRow)]
pub struct UploadedFile {
    pub file_id: String,
    pub filename: String,
    pub total_size: i64,
    pub checksum: String,
    pub status: i32,
    pub file_path: String,
    pub thumbnail_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[sqlx(default)]
    pub thumbnail_url: Option<String>,
    pub last_updated: i64,
    #[sqlx(default)]
    pub taken_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[sqlx(default)]
    pub source_device: Option<String>,
}

const IMAGE_EXTS: [&str; 6] = ["jpg", "jpeg", "png", "gif", "webp", "bmp"];
const VIDEO_EXTS: [&str; 8] = ["mp4", "webm", "mkv", "avi", "mov", "flv", "wmv", "m4v"];

fn exts_like(exts: &[&str]) -> String {
    exts.iter()
        .map(|e| format!("lower(filename) LIKE '%.{}'", e))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// 根据媒体类型生成 SQL 筛选条件（image/video/other，白名单，无注入风险）
fn media_type_condition(media_type: &str) -> String {
    match media_type {
        "image" => format!(" AND ({})", exts_like(&IMAGE_EXTS)),
        "video" => format!(" AND ({})", exts_like(&VIDEO_EXTS)),
        "other" => format!(
            " AND NOT ({}) AND NOT ({})",
            exts_like(&IMAGE_EXTS),
            exts_like(&VIDEO_EXTS)
        ),
        _ => String::new(),
    }
}

pub async fn fetch_uploaded_files(
    db_pool: &SqlitePool,
    page: u32,
    page_size: u32,
    status: Option<i32>,
    sort_by: &str,
    order: &str,
    source_device: Option<&str>,
    media_type: Option<&str>,
) -> Result<Vec<UploadedFile>, String> {
    let offset = (page - 1) * page_size;
    let mut query = String::from(
        "SELECT file_id, filename, total_size, checksum, status, file_path, thumbnail_path, last_updated, taken_at, source_device FROM upload_file_meta WHERE 1=1"
    );

    if status.is_some() {
        query.push_str(" AND status = ?");
    }
    if source_device.is_some() {
        query.push_str(" AND source_device = ?");
    }
    if let Some(mt) = media_type {
        query.push_str(&media_type_condition(mt));
    }

    match sort_by {
        "size" => query.push_str(" ORDER BY total_size"),
        "date" => query.push_str(" ORDER BY last_updated"),
        // 拍摄时间：未解析(taken_at=0)的排最后，其余按拍摄时间排序
        "taken_at" => query.push_str(" ORDER BY CASE WHEN taken_at = 0 THEN 1 ELSE 0 END ASC, taken_at"),
        _ => query.push_str(" ORDER BY id"), // Default sorting by id
    }

    match order {
        "desc" => query.push_str(" DESC"),
        _ => query.push_str(" ASC"), // Default order is ascending
    }

    query.push_str(&format!(" LIMIT {} OFFSET {}", page_size, offset));

    let mut q = sqlx::query_as::<_, UploadedFile>(&query);
    if let Some(s) = status {
        q = q.bind(s);
    }
    if let Some(sd) = source_device {
        q = q.bind(sd);
    }

    match q.fetch_all(db_pool).await {
        Ok(files) => Ok(files),
        Err(e) => {
            error!("Failed to fetch uploaded files: {}", e);
            Err("Failed to fetch uploaded files".to_string())
        }
    }
}

pub async fn fetch_total_uploaded_files(
    db_pool: &SqlitePool,
    status: Option<i32>,
    source_device: Option<&str>,
    media_type: Option<&str>,
) -> Result<i64, String> {
    let mut query_str = "SELECT COUNT(*) as total FROM upload_file_meta WHERE 1=1".to_string();

    if status.is_some() {
        query_str.push_str(" AND status = ?");
    }
    if source_device.is_some() {
        query_str.push_str(" AND source_device = ?");
    }
    if let Some(mt) = media_type {
        query_str.push_str(&media_type_condition(mt));
    }

    let mut q = sqlx::query(&query_str);
    if let Some(s) = status {
        q = q.bind(s);
    }
    if let Some(sd) = source_device {
        q = q.bind(sd);
    }

    match q.fetch_one(db_pool).await {
        Ok(row) => Ok(row.get::<i64, _>("total")),
        Err(e) => {
            error!("Failed to fetch total uploaded files: {}", e);
            Err("Failed to fetch total uploaded files".to_string())
        }
    }
}

/// 查询所有去重后的来源设备（用于客户端筛选器）
pub async fn fetch_distinct_source_devices(db_pool: &SqlitePool) -> Result<Vec<String>, String> {
    match sqlx::query(
        "SELECT DISTINCT source_device FROM upload_file_meta WHERE source_device IS NOT NULL AND source_device != '' ORDER BY source_device"
    )
    .fetch_all(db_pool)
    .await
    {
        Ok(rows) => {
            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                let source: String = row.get("source_device");
                result.push(source);
            }
            Ok(result)
        }
        Err(e) => {
            error!("Failed to fetch distinct source devices: {}", e);
            Err("Failed to fetch distinct source devices".to_string())
        }
    }
}

#[derive(Debug, Serialize, FromRow)]
pub struct ChunkProgress {
    pub start_offset: i64,
    pub end_offset: i64,
    pub uploaded_size: i64,
    pub last_updated: i64,
}

pub async fn fetch_upload_progress(db_pool: &SqlitePool, file_id: &str) -> Result<Vec<ChunkProgress>, String> {
    match sqlx::query_as::<_, ChunkProgress>(
        "SELECT start_offset, end_offset, uploaded_size, last_updated FROM upload_progress WHERE file_id = ?"
    )
    .bind(file_id)
    .fetch_all(db_pool)
    .await
    {
        Ok(chunks) => Ok(chunks),
        Err(e) => {
            error!("Failed to fetch upload progress: {}", e);
            Err("Failed to fetch upload progress".to_string())
        }
    }
}

/// 根据文件 MD5 checksum 查找已存在的文件记录
/// 返回 Option<(file_id, filename, file_path)>
pub async fn fetch_file_by_checksum(db_pool: &SqlitePool, checksum: &str) -> Result<Option<(String, String, String)>, String> {
    match sqlx::query_as::<_, (String, String, String)>(
        "SELECT file_id, filename, file_path FROM upload_file_meta WHERE checksum = ? AND status = 2"
    )
    .bind(checksum)
    .fetch_optional(db_pool)
    .await
    {
        Ok(result) => Ok(result),
        Err(e) => {
            error!("Failed to fetch file by checksum: {}", e);
            Err("Failed to fetch file by checksum".to_string())
        }
    }
}

/// 查找同 checksum 的未完成上传记录（status=0，合并未开始），用于断点续传。
/// 返回 (file_id, total_size)，仅当 total_size 一致才可安全复用分片进度。
pub async fn fetch_incomplete_by_checksum(db_pool: &SqlitePool, checksum: &str) -> Result<Option<(String, i64)>, String> {
    match sqlx::query_as::<_, (String, i64)>(
        "SELECT file_id, total_size FROM upload_file_meta WHERE checksum = ? AND status = 0 LIMIT 1",
    )
    .bind(checksum)
    .fetch_optional(db_pool)
    .await
    {
        Ok(result) => Ok(result),
        Err(e) => {
            error!("Failed to fetch incomplete file by checksum: {}", e);
            Err("Failed to fetch incomplete file by checksum".to_string())
        }
    }
}

/// 查询某文件已完成上传的分片起始偏移（uploaded_size 覆盖整个分片）
pub async fn fetch_completed_chunk_offsets(db_pool: &SqlitePool, file_id: &str) -> Result<Vec<i64>, String> {
    match sqlx::query_as::<_, (i64,)>(
        "SELECT start_offset FROM upload_progress WHERE file_id = ? AND uploaded_size >= (end_offset - start_offset + 1)",
    )
    .bind(file_id)
    .fetch_all(db_pool)
    .await
    {
        Ok(rows) => Ok(rows.into_iter().map(|(offset,)| offset).collect()),
        Err(e) => {
            error!("Failed to fetch completed chunk offsets: {}", e);
            Err("Failed to fetch completed chunk offsets".to_string())
        }
    }
}

/// CAS 状态流转：仅当当前 status 等于 expected 时更新为 new_status，返回是否抢到
pub async fn update_file_status_cas(db_pool: &SqlitePool, file_id: &str, expected_status: i32, new_status: i32) -> Result<bool, String> {
    match sqlx::query(
        "UPDATE upload_file_meta SET status = ?, last_updated = strftime('%s', 'now') WHERE file_id = ? AND status = ?",
    )
    .bind(new_status)
    .bind(file_id)
    .bind(expected_status)
    .execute(db_pool)
    .await
    {
        Ok(result) => Ok(result.rows_affected() > 0),
        Err(e) => {
            error!("Failed to CAS file status: {}", e);
            Err("Failed to CAS file status".to_string())
        }
    }
}

/// 清理上传僵尸记录：上传中途被遗弃、超过 staleness_days 天仍未完成（status 0/1）的记录。
/// 返回被清理掉的原文件名列表，供调用方删除磁盘上残留的分片文件。
pub async fn cleanup_zombie_uploads(db_pool: &SqlitePool, staleness_days: i64) -> Result<Vec<String>, String> {
    let stale_records = match sqlx::query_as::<_, (String, String)>(
        "SELECT file_id, filename FROM upload_file_meta WHERE status IN (0, 1) AND last_updated < strftime('%s', 'now') - (? * 86400)",
    )
    .bind(staleness_days)
    .fetch_all(db_pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            error!("Failed to query zombie uploads: {}", e);
            return Err("Failed to query zombie uploads".to_string());
        }
    };

    let mut cleaned_filenames = Vec::new();
    for (file_id, filename) in &stale_records {
        if let Err(e) = sqlx::query("DELETE FROM upload_progress WHERE file_id = ?")
            .bind(file_id)
            .execute(db_pool)
            .await
        {
            error!("Failed to delete zombie progress rows for {}: {}", file_id, e);
            continue;
        }
        if let Err(e) = sqlx::query("DELETE FROM upload_file_meta WHERE file_id = ?")
            .bind(file_id)
            .execute(db_pool)
            .await
        {
            error!("Failed to delete zombie record {}: {}", file_id, e);
            continue;
        }
        cleaned_filenames.push(filename.clone());
    }

    Ok(cleaned_filenames)
}

/// Fetch a complete UploadedFile by file_id
pub async fn fetch_uploaded_file_by_id(db_pool: &SqlitePool, file_id: &str) -> Result<Option<UploadedFile>, String> {
    match sqlx::query_as::<_, UploadedFile>(
        "SELECT file_id, filename, total_size, checksum, status, file_path, thumbnail_path, last_updated, taken_at, source_device FROM upload_file_meta WHERE file_id = ?"
    )
    .bind(file_id)
    .fetch_optional(db_pool)
    .await
    {
        Ok(result) => Ok(result),
        Err(e) => {
            error!("Failed to fetch uploaded file by id: {}", e);
            Err("Failed to fetch uploaded file".to_string())
        }
    }
}

/// 更新文件的缩略图路径
pub async fn update_file_thumbnail_path(
    db_pool: &SqlitePool,
    file_id: &str,
    thumbnail_path: &str,
) -> Result<(), String> {
    match sqlx::query(
        "UPDATE upload_file_meta SET thumbnail_path = ?, last_updated = strftime('%s', 'now') WHERE file_id = ?"
    )
    .bind(thumbnail_path)
    .bind(file_id)
    .execute(db_pool)
    .await
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Failed to update file thumbnail path: {}", e)),
    }
}

/// 查询已完成但尚未解析 EXIF 拍摄时间的文件（返回 file_id, file_path）
pub async fn fetch_files_needing_exif_parse(
    db_pool: &SqlitePool,
) -> Result<Vec<(String, String)>, String> {
    match sqlx::query(
        "SELECT file_id, file_path FROM upload_file_meta WHERE status = 2 AND exif_parsed = 0 AND file_path IS NOT NULL AND file_path != ''",
    )
    .fetch_all(db_pool)
    .await
    {
        Ok(rows) => {
            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                let file_id: String = row.get("file_id");
                let file_path: String = row.get("file_path");
                result.push((file_id, file_path));
            }
            Ok(result)
        }
        Err(e) => {
            error!("Failed to fetch files needing exif parse: {}", e);
            Err("Failed to fetch files needing exif parse".to_string())
        }
    }
}

/// 写入文件的拍摄时间并标记已解析
pub async fn update_file_taken_at(
    db_pool: &SqlitePool,
    file_id: &str,
    taken_at: i64,
) -> Result<(), String> {
    match sqlx::query("UPDATE upload_file_meta SET taken_at = ?, exif_parsed = 1 WHERE file_id = ?")
        .bind(taken_at)
        .bind(file_id)
        .execute(db_pool)
        .await
    {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Failed to update file taken_at: {}", e)),
    }
}
