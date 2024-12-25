use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct UploadState {
    pub id: String,
    pub filename: String,
    pub total_size: u64,
    pub uploaded_size: u64,
    pub checksum: String,
}

impl UploadState {
    pub async fn save_to_db(&self, pool: &MySqlPool) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "INSERT INTO upload_states (id, filename, total_size, uploaded_size, checksum) VALUES (?, ?, ?, ?, ?)",
            self.id,
            self.filename,
            self.total_size,
            self.uploaded_size,
            self.checksum
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn update_in_db(&self, pool: &MySqlPool) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE upload_states SET uploaded_size = ?, checksum = ? WHERE id = ?",
            self.uploaded_size,
            self.checksum,
            self.id
        )
        .execute(pool)
        .await?;
        Ok(())
    }
} 