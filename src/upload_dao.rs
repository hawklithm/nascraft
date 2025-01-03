use sqlx::{MySql, MySqlPool, Transaction};
use sqlx::query;
use log::{error, info};
use sqlx::types::BigDecimal;
use bigdecimal::ToPrimitive;
use serde::Serialize;
use sqlx::FromRow;

pub async fn fetch_file_record(db_pool: &MySqlPool, file_id: &str) -> Result<(String, String, i64), String> {
    match query!(
        "SELECT filename, checksum, total_size FROM upload_file_meta WHERE file_id = ?",
        file_id
    )
    .fetch_one(db_pool)
    .await
    {
        Ok(record) => Ok((record.filename, record.checksum, record.total_size)),
        Err(e) => {
            error!("Failed to fetch file record: {}", e);
            Err("Failed to fetch file record".to_string())
        }
    }
}

pub async fn update_upload_progress(db_pool: &MySqlPool, uploaded_size: u64, checksum: &str, file_id: &str, start_offset: u64) -> Result<(), String> {
    if let Err(e) = query!(
        "UPDATE upload_progress SET uploaded_size = ?, checksum = ? WHERE file_id = ? AND start_offset = ?",
        uploaded_size,
        checksum,
        file_id,
        start_offset
    )
    .execute(db_pool)
    .await
    {
        error!("Failed to update upload progress: {}", e);
        return Err("Failed to update upload progress".to_string());
    }
    Ok(())
}

pub async fn get_total_uploaded(db_pool: &MySqlPool, file_id: &str) -> Result<u64, String> {
    match query!(
        "SELECT SUM(uploaded_size) as total_uploaded FROM upload_progress WHERE file_id = ?",
        file_id
    )
    .fetch_one(db_pool)
    .await
    {
        Ok(row) => Ok(row.total_uploaded.unwrap_or_else(|| BigDecimal::from(0)).to_u64().unwrap_or(0)),
        Err(e) => {
            error!("Failed to get total uploaded size: {}", e);
            Err("Failed to get total uploaded size".to_string())
        }
    }
}

pub async fn update_file_status(db_pool: &MySqlPool, file_id: &str, current_status: i32, new_status: i32) -> Result<(), String> {
    if let Err(e) = query!(
        "UPDATE upload_file_meta SET status = ? WHERE file_id = ? AND status = ?",
        new_status,
        file_id,
        current_status
    )
    .execute(db_pool)
    .await
    {
        error!("Failed to update file status: {}", e);
        return Err("Failed to update file status".to_string());
    }
    Ok(())
}

pub async fn fetch_chunk_size(db_pool: &MySqlPool) -> Result<u64, String> {
    match query!(
        "SELECT config_value FROM system_config WHERE config_key = 'chunk_size'"
    )
    .fetch_one(db_pool)
    .await
    {
        Ok(row) => row.config_value.parse().map_err(|_| "Invalid chunk size".to_string()),
        Err(e) => {
            error!("Failed to fetch chunk size: {}", e);
            Err("Failed to fetch chunk size".to_string())
        }
    }
}

pub async fn initialize_upload_progress(
    tx: &mut Transaction<'_, MySql>,
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
    .bind(total_size)
    .bind(0) // Initial uploaded size is 0
    .bind(start_offset)
    .bind(end_offset)
    .execute(&mut **tx)
    .await
    {
        error!("Failed to initialize upload progress: {}", e);
        return Err("Failed to initialize upload progress".to_string());
    }
    Ok(())
}

pub async fn save_upload_state_to_db(
    tx: &mut Transaction<'_, MySql>,
    file_id: &str,
    filename: &str,
    total_size: u64,
    checksum: &str,
) -> Result<(), String> {
    if let Err(e) = sqlx::query(
        "INSERT INTO upload_file_meta (file_id, filename, total_size, checksum) VALUES (?, ?, ?, ?)"
    )
    .bind(file_id)
    .bind(filename)
    .bind(total_size)
    .bind(checksum)
    .execute(&mut **tx)
    .await
    {
        error!("Failed to save upload state: {}", e);
        return Err("Failed to save upload state".to_string());
    }
    info!("Successfully saved upload state for file '{}', ID: '{}'", filename, file_id);

    Ok(())
}

#[derive(Debug, Serialize, FromRow)]
pub struct UploadedFile {
    pub file_id: String,
    pub filename: String,
    pub total_size: i64,
    pub checksum: String,
    pub status: i32,
}

pub async fn fetch_uploaded_files(
    db_pool: &MySqlPool,
    page: u32,
    page_size: u32,
    status: Option<i32>,
    sort_by: &str,
    order: &str,
) -> Result<Vec<UploadedFile>, String> {
    let offset = (page - 1) * page_size;
    let mut query = format!(
        "SELECT file_id, filename, total_size, checksum, status FROM upload_file_meta WHERE 1=1"
    );

    if let Some(status) = status {
        query.push_str(&format!(" AND status = {}", status));
    }

    match sort_by {
        "size" => query.push_str(" ORDER BY total_size"),
        "date" => query.push_str(" ORDER BY last_updated"),
        _ => query.push_str(" ORDER BY id"), // Default sorting by id
    }

    match order {
        "desc" => query.push_str(" DESC"),
        _ => query.push_str(" ASC"), // Default order is ascending
    }

    query.push_str(&format!(" LIMIT {} OFFSET {}", page_size, offset));

    match sqlx::query_as::<_, UploadedFile>(&query)
        .fetch_all(db_pool)
        .await
    {
        Ok(files) => Ok(files),
        Err(e) => {
            error!("Failed to fetch uploaded files: {}", e);
            Err("Failed to fetch uploaded files".to_string())
        }
    }
}
