use actix_web::{web, HttpResponse, Result};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use std::sync::Arc;
use log::error;
use crate::upload_dao::fetch_file_record;
use crate::AppState;

pub async fn download_file(
    data: web::Data<Arc<AppState>>,
    file_id: web::Path<String>,
) -> Result<HttpResponse> {
    let file_id_str = file_id.into_inner();

    // Fetch file record to get the file path
    let (_, _, _, _, file_path) = match fetch_file_record(&data.db_pool, &file_id_str).await {
        Ok(record) => record,
        Err(e) => return Ok(HttpResponse::InternalServerError().body(e)),
    };

    // Open the file
    let mut file = match File::open(&file_path).await {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open file: {}", e);
            return Ok(HttpResponse::InternalServerError().body("Failed to open file"));
        }
    };

    // Read the file content
    let mut buffer = Vec::new();
    if let Err(e) = file.read_to_end(&mut buffer).await {
        error!("Failed to read file: {}", e);
        return Ok(HttpResponse::InternalServerError().body("Failed to read file"));
    }

    // Return the file content as a response
    Ok(HttpResponse::Ok()
        .content_type("application/octet-stream")
        .body(buffer))
} 