use axum::{routing::{get, post}, Router};

use crate::context::AppContext;
use crate::display_remote::{
    browse_files, discovered_devices, hello, pause_video, play_video, resume_video, stop_video,
};
use crate::download::{download_file, serve_thumbnail};
use crate::ssdp::ssdp_routes;
use crate::upload::{
    get_uploaded_files, get_upload_status, submit_file_metadata, upload_file,
};

pub fn build_router(ctx: AppContext) -> Router {
    let router = Router::new()
        .route("/api/upload", post(upload_file))
        .route("/api/submit_metadata", post(submit_file_metadata))
        .route("/api/upload_status/:file_id", get(get_upload_status))
        .route("/api/download/:file_id", get(download_file))
        .route("/api/thumbnail/:file_id", get(serve_thumbnail))
        .route("/api/uploaded_files", get(get_uploaded_files))
        .route("/api/dlna/devices", get(discovered_devices))
        .route("/api/dlna/play", post(play_video))
        .route("/api/dlna/pause", post(pause_video))
        .route("/api/dlna/resume", post(resume_video))
        .route("/api/dlna/stop", post(stop_video))
        .route("/api/dlna/browse", post(browse_files))
        .route("/api/hello", get(hello))
        .with_state(ctx);

    ssdp_routes(router)
}
