use axum::{extract::State, routing::{get, post}, Router};
use axum::http::StatusCode;
use axum::response::{Json, IntoResponse};
use serde::{Deserialize, Serialize};

use log::info;
use crate::context::AppContext;
use crate::upload_dao::fetch_file_record;
use crate::dlna_renderer::{best_local_ipv4_for, build_didl_metadata, MediaRenderer, PlaybackInfo};
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
    // 先主动搜索一轮再返回，避免只读到轮询间隙的旧缓存
    let devices = ctx.renderer_manager.refresh_and_list().await;
    (StatusCode::OK, Json(ApiResponse::success(DeviceListResponse { devices })))
}

async fn play_on_renderer(
    State(ctx): State<AppContext>,
    Json(req): Json<PlayOnRendererRequest>,
) -> impl IntoResponse {
    // 预校验文件确实存在于服务器磁盘，并拿到文件名用于生成 DIDL-Lite metadata
    let filename = match fetch_file_record(&ctx.app_state.db_pool, &req.file_id).await {
        Ok((filename, _, _, _, file_path)) => {
            if tokio::fs::metadata(&file_path).await.is_err() {
                return (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::error(
                    "400".to_string(),
                    "文件在服务器上不存在，无法投屏（可能已删除或上传未完成）".to_string(),
                )));
            }
            filename
        }
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::error(
                "400".to_string(),
                format!("文件不存在: {}", e),
            )));
        }
    };

    // 构造完整的下载 URL（电视会主动来这个地址拉流，必须是它们网段可达的地址）
    let server_url = match ctx.config.external_url.clone() {
        Some(url) => url,
        None => {
            // 优先选与目标电视同网段的本机 IP，多网卡环境下 local_ip() 可能选到虚拟网卡
            let local_ip = match ctx.renderer_manager.renderer_ip(&req.uuid).await {
                Some(target_ip) => best_local_ipv4_for(&target_ip).unwrap_or_else(get_local_ip),
                None => get_local_ip(),
            };
            let url = format!("http://{}:{}", local_ip, ctx.config.server_port);
            log::warn!(
                "NASCRAFT_EXTERNAL_URL 未配置，投屏使用本机 IP: {}（多网卡/Docker 环境可能不可达，届时请配置 NASCRAFT_EXTERNAL_URL）",
                url
            );
            url
        }
    };

    let playback_url = format!("{}/api/download/{}", server_url.trim_end_matches('/'), req.file_id);
    let metadata = build_didl_metadata(&filename, &playback_url);

    info!("Casting file {} ({}) to renderer {} via {}", req.file_id, filename, req.uuid, playback_url);

    match ctx.renderer_manager.play_uri(req.uuid.as_str(), playback_url, Some(metadata)).await {
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

async fn hello() -> &'static str {
    "Service is alive"
}

pub fn build_router(ctx: AppContext) -> Router {
    let router = Router::new()
        .route("/api/upload", post(upload_file))
        .route("/api/submit_metadata", post(submit_file_metadata))
        .route("/api/upload_status/:file_id", get(get_upload_status))
        .route("/api/download/:file_id", get(download_file))
        .route("/api/thumbnail/:file_id", get(serve_thumbnail))
        .route("/api/uploaded_files", get(get_uploaded_files))
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
