use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct UploadState {
    pub id: String,
    pub filename: String,
    pub total_size: u64,
    pub checksum: String,
}

impl UploadState {
    pub async fn save_to_db(&self, pool: &MySqlPool) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "INSERT INTO upload_file_meta (id, filename, total_size, checksum) VALUES (?, ?, ?, ?)",
            self.id,
            self.filename,
            self.total_size,
            self.checksum
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn update_in_db(&self, pool: &MySqlPool) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE upload_file_meta SET checksum = ? WHERE id = ?",
            self.checksum,
            self.id
        )
        .execute(pool)
        .await?;
        Ok(())
    }
} 