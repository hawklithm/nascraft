use std::path::{Path, PathBuf};
use std::time::Duration;
use log::{info, error, debug};
use local_ip_address::local_ip;
use actix_web::{web, HttpResponse, Error};
use actix_files::NamedFile;
use serde::{Deserialize, Serialize};
use rupnp::ssdp::SearchTarget;
use rupnp::{Device, Service};
use std::str::FromStr;
use rupnp::ssdp::URN;
use futures::StreamExt;
use actix_web::http::Uri;
use tokio::sync::Mutex;
use std::sync::Arc;
use std::convert::TryFrom;
use reqwest;
use tokio::sync::broadcast;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeviceState {
    #[serde(default)]
    pub playback: i32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub volume: i32,
    #[serde(default = "empty_string")]
    pub position: String,
    #[serde(default = "empty_string")]
    pub duration: String,
    #[serde(default)]
    pub buffer: i32,
    #[serde(default = "empty_string")]
    pub name: String,
    #[serde(default = "empty_string")]
    pub uri: String,
    #[serde(default = "empty_string")]
    pub metadata: String,
}

fn empty_string() -> String {
    String::new()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RendererAction {
    RendererAdd,
    RendererDelete,
    RendererUpdate,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceMessage {
    pub id: i32,
    pub name: String,
    pub address: String,
    pub uuid: String,
    pub icon: String,
    #[serde(rename = "iconOverlays")]
    pub icon_overlays: String,
    pub playing: String,
    pub time: String,
    #[serde(rename = "progressPercent")]
    pub progress_percent: i32,
    #[serde(rename = "userId")]
    pub user_id: i32,
    pub state: DeviceState,
    #[serde(rename = "isActive")]
    pub is_active: bool,
    #[serde(rename = "isAllowed")]
    pub is_allowed: bool,
    #[serde(rename = "isAuthenticated")]
    pub is_authenticated: bool,
    pub controls: i32,
    pub action: String,
}

pub struct SSEListener {
    devices: Arc<Mutex<HashMap<String, DeviceMessage>>>,
    tx: broadcast::Sender<DeviceMessage>,
    media_server_port: u16,
}

impl SSEListener {
    pub fn new(port: u16) -> Self {
        info!("Creating new SSE listener");
        let (tx, _) = broadcast::channel(100);
        SSEListener {
            devices: Arc::new(Mutex::new(HashMap::new())),
            tx,
            media_server_port: port,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DeviceMessage> {
        info!("New subscriber connected to SSE listener");
        self.tx.subscribe()
    }

    pub async fn get_devices(&self) -> HashMap<String, DeviceMessage> {
        info!("Retrieving current device list");
        let devices = self.devices.lock().await.clone();
        info!("Found {} devices in cache", devices.len());
        devices
    }

    pub async fn start_listening(self: Arc<Self>) {
        info!("Starting SSE listener");
        tokio::spawn(async move {
            loop {
                if let Err(e) = self.listen().await {
                    error!("SSE listener error: {}", e);
                    info!("Retrying SSE connection in 5 seconds");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        });
    }

    async fn listen(&self) -> Result<(), String> {
        info!("Establishing SSE connection to server");
        let client = reqwest::Client::new();
        let base_url = format!("http://localhost:{}", self.media_server_port);
        let mut response = client.get(format!("{}/v1/api/sse/", base_url))
            .header("Accept", "text/event-stream")
            .header("Authorization", "Bearer null")
            .header("Referer", format!("{}/", base_url))
            .send()
            .await
            .map_err(|e| format!("Failed to connect to SSE stream: {}", e))?;

        info!("SSE connection established successfully");
        let mut event_type = String::new();
        let mut event_data = String::new();

        while let Ok(Some(chunk)) = response.chunk().await {
            let text = String::from_utf8_lossy(&chunk);
            for line in text.lines() {
                debug!("origin message: {}", line);
                if line.is_empty() {
                    if !event_data.is_empty() {
                        self.handle_event(&event_type, &event_data).await?;
                        event_type.clear();
                        event_data.clear();
                    }
                } else if line.starts_with("event: ") {
                    event_type = line[7..].to_string();
                } else if line.starts_with("data: ") {
                    event_data = line[6..].to_string();
                }
            }
        }
        error!("SSE connection closed unexpectedly");
        Ok(())
    }

    async fn handle_event(&self, event_type: &str, data: &str) -> Result<(), String> {
        match event_type {
            "message" => {
                debug!("Parsing message data: {}", data);
                match serde_json::from_str::<DeviceMessage>(data) {
                    Ok(msg) => {
                        match msg.action.as_str() {
                            "renderer_add" | "renderer_delete" | "renderer_update" => {
                                info!("收到设备事件 - 动作: {}, ID: {}, 名称: {}", 
                                    msg.action, msg.id, msg.name);
                                self.devices.lock().await.insert(msg.uuid.clone(), msg.clone());
                                let _ = self.tx.send(msg);
                            }
                            _ => {
                                debug!("忽略未知的渲染器动作: {}", msg.action);
                            }
                        }
                    }
                    Err(e) => {
                        error!("解析设备消息失败: {} - 原始数据: {}", e, data);
                    }
                }
            }
            _ => {
                debug!("忽略未知的事件类型: {}", event_type);
            }
        }
        Ok(())
    }
}

pub struct DLNAPlayer {
    media_server_port: u16,
    sse_listener: Arc<SSEListener>,
}

impl DLNAPlayer {
    pub async fn new() -> Self {
        info!("Initializing DLNA player");
        let media_server_port = 9001;
        let sse_listener = Arc::new(SSEListener::new(media_server_port));
        sse_listener.clone().start_listening().await;
        
        info!("DLNA player initialized with media server port: {}", media_server_port);
        DLNAPlayer {
            media_server_port,
            sse_listener,
        }
    }

    async fn send_control_request(&self, device_id: i32, action: &str, value: Option<String>) -> Result<(), String> {
        info!("Sending control request - Device ID: {}, Action: {}", device_id, action);
        if let Some(val) = &value {
            info!("Control request value: {}", val);
        }

        let client = reqwest::Client::new();
        
        let mut control_request = serde_json::json!({
            "id": device_id,
            "action": action
        });

        if let Some(val) = value {
            control_request["value"] = serde_json::Value::String(val);
        }

        let request_url = format!("http://localhost:{}/v1/api/renderers/control", self.media_server_port);
        info!("Sending request to: {}", request_url);
        debug!("Request payload: {}", serde_json::to_string_pretty(&control_request).unwrap());

        let response = client.post(&request_url)
            .json(&control_request)
            .send()
            .await
            .map_err(|e| {
                error!("Failed to send control request: {}", e);
                format!("Failed to send control request: {}", e)
            })?;

        let status = response.status();
        info!("Received response with status: {}", status);

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            error!("Control request failed with error: {}", error_text);
            return Err(format!("Control request failed: {}", error_text));
        }

        info!("Control request completed successfully");
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct DeviceResponse {
    pub id: i32,
    pub name: String,
    pub address: String,
    pub uuid: String,
    pub state: DeviceState,
    pub is_active: bool,
}

pub async fn discovered_devices(
    dlna_player: web::Data<Arc<Mutex<DLNAPlayer>>>,
) -> Result<HttpResponse, Error> {
    info!("Handling device discovery request");
    let player = dlna_player.lock().await;
    let devices = player.sse_listener.get_devices().await;
    
    info!("Converting device messages to response format");
    let device_responses: Vec<DeviceResponse> = devices.values()
        .map(|msg| {
            debug!("Processing device - ID: {}, Name: {}", msg.id, msg.name);
            DeviceResponse {
                id: msg.id,
                name: msg.name.clone(),
                address: msg.address.clone(),
                uuid: msg.uuid.clone(),
                state: msg.state.clone(),
                is_active: msg.is_active,
            }
        })
        .collect();

    info!("Returning {} devices in response", device_responses.len());
    Ok(HttpResponse::Ok().json(device_responses))
}

#[derive(Debug, Deserialize)]
pub struct PlayVideoRequest {
    device_id: i32,
    media_id: String,
}

#[derive(Debug, Deserialize)]
pub struct DeviceControlRequest {
    device_id: i32,
}

pub async fn play_video(
    dlna_player: web::Data<Arc<Mutex<DLNAPlayer>>>,
    req: web::Json<PlayVideoRequest>,
) -> Result<HttpResponse, Error> {
    info!("Handling play video request - Device ID: {}, Media ID: {}", 
        req.device_id, req.media_id);
    
    let player = dlna_player.lock().await;
    match player.send_control_request(req.device_id, "mediaid", Some(req.media_id.clone())).await {
        Ok(_) => {
            info!("Play video request sent successfully");
            Ok(HttpResponse::Ok().body("Play request sent successfully"))
        }
        Err(e) => {
            error!("Failed to send play request: {}", e);
            Ok(HttpResponse::InternalServerError().body(e))
        }
    }
}

pub async fn pause_video(
    dlna_player: web::Data<Arc<Mutex<DLNAPlayer>>>,
    req: web::Json<DeviceControlRequest>,
) -> Result<HttpResponse, Error> {
    info!("Handling pause video request - Device ID: {}", req.device_id);
    
    let player = dlna_player.lock().await;
    match player.send_control_request(req.device_id, "pause", None).await {
        Ok(_) => {
            info!("Pause request sent successfully");
            Ok(HttpResponse::Ok().body("Pause request sent successfully"))
        }
        Err(e) => {
            error!("Failed to send pause request: {}", e);
            Ok(HttpResponse::InternalServerError().body(e))
        }
    }
}

pub async fn resume_video(
    dlna_player: web::Data<Arc<Mutex<DLNAPlayer>>>,
    req: web::Json<DeviceControlRequest>,
) -> Result<HttpResponse, Error> {
    info!("Handling resume video request - Device ID: {}", req.device_id);
    
    let player = dlna_player.lock().await;
    match player.send_control_request(req.device_id, "play", None).await {
        Ok(_) => {
            info!("Resume request sent successfully");
            Ok(HttpResponse::Ok().body("Resume request sent successfully"))
        }
        Err(e) => {
            error!("Failed to send resume request: {}", e);
            Ok(HttpResponse::InternalServerError().body(e))
        }
    }
}

pub async fn stop_video(
    dlna_player: web::Data<Arc<Mutex<DLNAPlayer>>>,
    req: web::Json<DeviceControlRequest>,
) -> Result<HttpResponse, Error> {
    info!("Handling stop video request - Device ID: {}", req.device_id);
    
    let player = dlna_player.lock().await;
    match player.send_control_request(req.device_id, "stop", None).await {
        Ok(_) => {
            info!("Stop request sent successfully");
            Ok(HttpResponse::Ok().body("Stop request sent successfully"))
        }
        Err(e) => {
            error!("Failed to send stop request: {}", e);
            Ok(HttpResponse::InternalServerError().body(e))
        }
    }
}

// 新增：处理媒体文件的请求
pub async fn serve_media(path: web::Path<String>) -> Result<NamedFile, Error> {
    let media_path = PathBuf::from("media").join(path.into_inner());
    info!("Serving media file: {}", media_path.display());
    match NamedFile::open(&media_path) {
        Ok(file) => {
            info!("Media file served successfully");
            Ok(file)
        }
        Err(e) => {
            error!("Failed to serve media file: {}", e);
            Err(Error::from(e))
        }
    }
}

pub async fn hello() -> Result<HttpResponse, Error> {
    info!("Handling health check request");
    Ok(HttpResponse::Ok().body("Service is alive"))
}

#[derive(Debug, Serialize)]
pub enum TransportState {
    Playing,
    Paused,
    Stopped,
    Unknown,
} 