use sqlx::mysql::MySqlPool;
use sqlx::query;
use log::error;
use sqlx::types::BigDecimal;
use bigdecimal::ToPrimitive;

pub async fn fetch_file_record(db_pool: &MySqlPool, file_id: &str) -> Result<(String, String, i64), String> {
    match query!(
        "SELECT filename, checksum, total_size FROM upload_file_meta WHERE id = ?",
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
        "UPDATE upload_file_meta SET status = ? WHERE id = ? AND status = ?",
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
    db_pool: &MySqlPool,
    file_id: &str,
    safe_filename: &str,
    total_size: u64,
    start_offset: u64,
    end_offset: u64,
) -> Result<(), String> {
    if let Err(e) = query!(
        "INSERT INTO upload_progress (file_id, checksum, filename, total_size, uploaded_size, start_offset, end_offset) VALUES (?, ?, ?, ?, ?, ?, ?)",
        file_id,
        "", // Initial checksum is empty
        safe_filename,
        total_size,
        0, // Initial uploaded size is 0
        start_offset,
        end_offset
    )
    .execute(db_pool)
    .await
    {
        error!("Failed to initialize upload progress: {}", e);
        return Err("Failed to initialize upload progress".to_string());
    }
    Ok(())
}

pub async fn save_upload_state_to_db(
    pool: &MySqlPool,
    id: &str,
    filename: &str,
    total_size: u64,
    checksum: &str,
) -> Result<(), String> {
    if let Err(e) = query!(
        "INSERT INTO upload_file_meta (id, filename, total_size, checksum) VALUES (?, ?, ?, ?)",
        id,
        filename,
        total_size,
        checksum
    )
    .execute(pool)
    .await
    {
        error!("Failed to save upload state: {}", e);
        return Err("Failed to save upload state".to_string());
    }
    Ok(())
}
