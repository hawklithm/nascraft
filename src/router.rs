use axum::{extract::State, routing::{get, post}, Router};
use axum::http::StatusCode;
use axum::response::{Json, IntoResponse};
use serde::{Deserialize, Serialize};

use crate::context::AppContext;
use crate::display_remote::{
    browse_files, discovered_devices, hello, pause_video, play_video, resume_video, stop_video,
};
use crate::dlna_renderer::{self, MediaRenderer, PlaybackInfo, RendererManager};
use crate::download::{download_file, serve_thumbnail};
use crate::ssdp::ssdp_routes;
use crate::upload::{
    get_uploaded_files, get_upload_status, submit_file_metadata, upload_file,
};
use crate::helper::ApiResponse;

#[derive(Debug, Deserialize)]
pub struct PlayOnRendererRequest {
    pub uuid: String,
    pub file_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SeekRequest {
    pub uuid: String,
    pub position: u32,
}

#[derive(Debug, Deserialize)]
pub struct VolumeRequest {
    pub uuid: String,
    pub volume: i32,
}

#[derive(Debug, Deserialize)]
pub struct MuteRequest {
    pub uuid: String,
    pub mute: bool,
}

#[derive(Debug, Serialize)]
pub struct DeviceListResponse {
    pub devices: Vec<(MediaRenderer, PlaybackInfo)>,
}

async fn list_renderers(
    State(ctx): State<AppContext>,
) -> impl IntoResponse {
    let devices = ctx.renderer_manager.list_devices().await;
    (StatusCode::OK, Json(ApiResponse::success(DeviceListResponse { devices })))
}

async fn play_on_renderer(
    State(ctx): State<AppContext>,
    Json(req): Json<PlayOnRendererRequest>,
) -> impl IntoResponse {
    // 构造完整的下载 URL
    let server_url = ctx.config.external_url.clone()
        .unwrap_or_else(|| format!("http://{}:{}", get_local_ip(), ctx.config.server_port));

    let playback_url = format!("{}/api/download/{}", server_url.trim_end_matches('/'), req.file_id);

    match ctx.renderer_manager.play_uri(req.uuid.as_str(), playback_url, None).await {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error("500".to_string(), e))),
    }
}

async fn pause_renderer(
    State(ctx): State<AppContext>,
    Json(req): Json<crate::dlna_renderer::DeviceControlRequest>,
) -> impl IntoResponse {
    match ctx.renderer_manager.pause(&req.uuid).await {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error("500".to_string(), e))),
    }
}

async fn resume_renderer(
    State(ctx): State<AppContext>,
    Json(req): Json<crate::dlna_renderer::DeviceControlRequest>,
) -> impl IntoResponse {
    match ctx.renderer_manager.play(&req.uuid).await {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error("500".to_string(), e))),
    }
}

async fn stop_renderer(
    State(ctx): State<AppContext>,
    Json(req): Json<crate::dlna_renderer::DeviceControlRequest>,
) -> impl IntoResponse {
    match ctx.renderer_manager.stop(&req.uuid).await {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error("500".to_string(), e))),
    }
}

async fn seek_renderer(
    State(ctx): State<AppContext>,
    Json(req): Json<SeekRequest>,
) -> impl IntoResponse {
    match ctx.renderer_manager.seek(&req.uuid, req.position).await {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error("500".to_string(), e))),
    }
}

async fn set_volume(
    State(ctx): State<AppContext>,
    Json(req): Json<VolumeRequest>,
) -> impl IntoResponse {
    match ctx.renderer_manager.set_volume(&req.uuid, req.volume).await {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error("500".to_string(), e))),
    }
}

async fn set_mute(
    State(ctx): State<AppContext>,
    Json(req): Json<MuteRequest>,
) -> impl IntoResponse {
    match ctx.renderer_manager.set_mute(&req.uuid, req.mute).await {
        Ok(_) => (StatusCode::OK, Json(ApiResponse::<()>::success(()))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResponse::<()>::error("500".to_string(), e))),
    }
}

fn get_local_ip() -> String {
    match local_ip_address::local_ip() {
        Ok(ip) => ip.to_string(),
        Err(_) => "localhost".to_string(),
    }
}

pub fn build_router(ctx: AppContext) -> Router {
    let router = Router::new()
        .route("/api/upload", post(upload_file))
        .route("/api/submit_metadata", post(submit_file_metadata))
        .route("/api/upload_status/:file_id", get(get_upload_status))
        .route("/api/download/:file_id", get(download_file))
        .route("/api/thumbnail/:file_id", get(serve_thumbnail))
        .route("/api/uploaded_files", get(get_uploaded_files))
        // Legacy DLNA (external player) API
        .route("/api/dlna/devices", get(discovered_devices))
        .route("/api/dlna/play", post(play_video))
        .route("/api/dlna/pause", post(pause_video))
        .route("/api/dlna/resume", post(resume_video))
        .route("/api/dlna/stop", post(stop_video))
        .route("/api/dlna/browse", post(browse_files))
        // Native DLNA renderer discovery and control API
        .route("/api/dlna/renderers", get(list_renderers))
        .route("/api/dlna/renderer/play", post(play_on_renderer))
        .route("/api/dlna/renderer/pause", post(pause_renderer))
        .route("/api/dlna/renderer/resume", post(resume_renderer))
        .route("/api/dlna/renderer/stop", post(stop_renderer))
        .route("/api/dlna/renderer/seek", post(seek_renderer))
        .route("/api/dlna/renderer/volume", post(set_volume))
        .route("/api/dlna/renderer/mute", post(set_mute))
        .route("/api/hello", get(hello))
        .with_state(ctx);

    ssdp_routes(router)
}
