use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use log::error;
use crate::upload_dao::fetch_file_record;
use crate::AppContext;

pub async fn download_file(
    State(ctx): State<AppContext>,
    Path(file_id_str): Path<String>,
) -> impl IntoResponse {
    let db_pool = &ctx.app_state.db_pool;

    // Fetch file record to get the file path
    let (_, _, _, _, file_path) = match fetch_file_record(db_pool, &file_id_str).await {
        Ok(record) => record,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    // Open the file
    let mut file = match File::open(&file_path).await {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open file: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to open file").into_response();
        }
    };

    // Read the file content
    let mut buffer = Vec::new();
    if let Err(e) = file.read_to_end(&mut buffer).await {
        error!("Failed to read file: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read file").into_response();
    }

    // Return the file content as a response
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        buffer,
    )
        .into_response()
}