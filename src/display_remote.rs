use std::path::{Path, PathBuf};
use std::time::Duration;
use log::{info, error};
use local_ip_address::local_ip;
use mime_guess::from_path;
use std::net::SocketAddr;
use actix_web::{web, HttpResponse, Error};
use actix_files::NamedFile;
use serde::{Deserialize, Serialize};
use rupnp::ssdp::SearchTarget;
use rupnp::{Device, Service};
use std::str::FromStr;

#[derive(Debug, Serialize)]
pub enum TransportState {
    Playing,
    Paused,
    Stopped,
    Unknown,
}

pub struct DLNAPlayer {
    device: Option<Device>,
    av_transport: Option<Service>,
    media_server_port: u16,
}

impl DLNAPlayer {
    pub async fn new() -> Self {
        DLNAPlayer {
            device: None,
            av_transport: None,
            media_server_port: 8081,
        }
    }

    pub async fn discover_devices(&mut self) -> Result<Vec<Device>, String> {
        let search_target = SearchTarget::from_str("urn:schemas-upnp-org:service:AVTransport:1")
            .map_err(|e| format!("创建搜索目标失败: {}", e))?;

        let devices = rupnp::discover(&search_target, Duration::from_secs(5))
            .await
            .map_err(|e| format!("设备搜索失败: {}", e))?
            .collect::<Vec<_>>()
            .await;

        if devices.is_empty() {
            return Err("未找到支持DLNA的设备".to_string());
        }

        Ok(devices)
    }

    pub async fn connect_to_device(&mut self, device: Device) -> Result<(), String> {
        // 查找 AVTransport 服务
        let av_transport = device
            .find_service("urn:schemas-upnp-org:service:AVTransport:1")
            .ok_or("设备不支持 AVTransport 服务")?;

        self.device = Some(device);
        self.av_transport = Some(av_transport);
        Ok(())
    }

    pub async fn play_video(&self, video_path: &Path) -> Result<(), String> {
        let av_transport = self.av_transport.as_ref().ok_or("未连接到设备")?;
        
        // 验证文件是否存在
        if !video_path.exists() {
            return Err(format!("文件不存在: {}", video_path.display()));
        }
        
        // 获取本地IP地址
        let local_ip = local_ip()
            .map_err(|e| format!("获取本地IP地址失败: {}", e))?;
            
        // 构建视频URL
        let video_url = format!(
            "http://{}:{}/media/{}",
            local_ip,
            self.media_server_port,
            video_path.file_name()
                .ok_or("无效的文件路径")?
                .to_string_lossy()
        );

        info!("播放视频URL: {}", video_url);

        // 获取MIME类型
        let mime_type = from_path(video_path)
            .first_or_octet_stream()
            .to_string();

        info!("视频MIME类型: {}", mime_type);

        // 设置 URI
        let mut args = vec![
            ("InstanceID", "0"),
            ("CurrentURI", &video_url),
            ("CurrentURIMetaData", ""),
        ];

        av_transport
            .call_action("SetAVTransportURI", args)
            .await
            .map_err(|e| format!("设置视频URI失败: {}", e))?;

        // 开始播放
        args = vec![("InstanceID", "0"), ("Speed", "1")];
        av_transport
            .call_action("Play", args)
            .await
            .map_err(|e| format!("播放失败: {}", e))?;

        Ok(())
    }

    pub async fn pause(&self) -> Result<(), String> {
        if let Some(av_transport) = &self.av_transport {
            let args = vec![("InstanceID", "0")];
            av_transport
                .call_action("Pause", args)
                .await
                .map_err(|e| format!("暂停失败: {}", e))?;
        }
        Ok(())
    }

    pub async fn resume(&self) -> Result<(), String> {
        if let Some(av_transport) = &self.av_transport {
            let args = vec![("InstanceID", "0"), ("Speed", "1")];
            av_transport
                .call_action("Play", args)
                .await
                .map_err(|e| format!("恢复播放失败: {}", e))?;
        }
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), String> {
        if let Some(av_transport) = &self.av_transport {
            let args = vec![("InstanceID", "0")];
            av_transport
                .call_action("Stop", args)
                .await
                .map_err(|e| format!("停止播放失败: {}", e))?;
        }
        Ok(())
    }

    pub async fn get_playback_status(&self) -> Result<TransportState, String> {
        if let Some(av_transport) = &self.av_transport {
            let args = vec![("InstanceID", "0")];
            let response = av_transport
                .call_action("GetTransportInfo", args)
                .await
                .map_err(|e| format!("获取播放状态失败: {}", e))?;

            let state = response.get("CurrentTransportState").unwrap_or("UNKNOWN");
            Ok(match state {
                "PLAYING" => TransportState::Playing,
                "PAUSED_PLAYBACK" => TransportState::Paused,
                "STOPPED" => TransportState::Stopped,
                _ => TransportState::Unknown,
            })
        } else {
            Err("未连接到设备".to_string())
        }
    }
}

#[derive(Deserialize)]
pub struct PlayVideoRequest {
    file_path: String,
}

#[derive(Serialize)]
pub struct DeviceInfo {
    name: String,
    location: String,
}

pub async fn discover_devices(
    dlna_player: web::Data<tokio::sync::Mutex<DLNAPlayer>>,
) -> Result<HttpResponse, Error> {
    let mut player = dlna_player.lock().await;
    match player.discover_devices().await {
        Ok(devices) => {
            let device_infos: Vec<DeviceInfo> = devices
                .iter()
                .map(|d| DeviceInfo {
                    name: d.description().device().friendly_name.clone(),
                    location: d.url().to_string(),
                })
                .collect();
            Ok(HttpResponse::Ok().json(device_infos))
        }
        Err(e) => Ok(HttpResponse::InternalServerError().body(e)),
    }
}

pub async fn connect_device(
    dlna_player: web::Data<tokio::sync::Mutex<DLNAPlayer>>,
    device_location: web::Path<String>,
) -> Result<HttpResponse, Error> {
    let mut player = dlna_player.lock().await;
    
    // 先搜索设备
    let devices = match player.discover_devices().await {
        Ok(devices) => devices,
        Err(e) => return Ok(HttpResponse::InternalServerError().body(e)),
    };
    
    // 查找指定位置的设备
    let device = match devices.into_iter().find(|d| d.url().to_string() == device_location.as_str()) {
        Some(device) => device,
        None => return Ok(HttpResponse::NotFound().body("设备未找到")),
    };
    
    match player.connect_to_device(device).await {
        Ok(_) => Ok(HttpResponse::Ok().body("连接成功")),
        Err(e) => Ok(HttpResponse::InternalServerError().body(e)),
    }
}

pub async fn play_video(
    dlna_player: web::Data<tokio::sync::Mutex<DLNAPlayer>>,
    req: web::Json<PlayVideoRequest>,
) -> Result<HttpResponse, Error> {
    let player = dlna_player.lock().await;
    let path = PathBuf::from(&req.file_path);
    
    // 确保media目录存在
    std::fs::create_dir_all("media").map_err(|e| {
        error!("创建media目录失败: {}", e);
        actix_web::error::ErrorInternalServerError(e)
    })?;

    // 如果文件不在media目录中，复制到media目录
    let media_path = PathBuf::from("media").join(path.file_name().ok_or_else(|| {
        error!("无效的文件名");
        actix_web::error::ErrorBadRequest("无效的文件名")
    })?);

    if !media_path.exists() {
        std::fs::copy(&path, &media_path).map_err(|e| {
            error!("复制文件到media目录失败: {}", e);
            actix_web::error::ErrorInternalServerError(e)
        })?;
    }
    
    match player.play_video(&media_path).await {
        Ok(_) => Ok(HttpResponse::Ok().body("开始播放")),
        Err(e) => {
            error!("播放视频失败: {}", e);
            Ok(HttpResponse::InternalServerError().body(e))
        }
    }
}

pub async fn pause_video(
    dlna_player: web::Data<tokio::sync::Mutex<DLNAPlayer>>,
) -> Result<HttpResponse, Error> {
    let player = dlna_player.lock().await;
    match player.pause().await {
        Ok(_) => Ok(HttpResponse::Ok().body("已暂停")),
        Err(e) => Ok(HttpResponse::InternalServerError().body(e)),
    }
}

pub async fn resume_video(
    dlna_player: web::Data<tokio::sync::Mutex<DLNAPlayer>>,
) -> Result<HttpResponse, Error> {
    let player = dlna_player.lock().await;
    match player.resume().await {
        Ok(_) => Ok(HttpResponse::Ok().body("已恢复播放")),
        Err(e) => Ok(HttpResponse::InternalServerError().body(e)),
    }
}

pub async fn stop_video(
    dlna_player: web::Data<tokio::sync::Mutex<DLNAPlayer>>,
) -> Result<HttpResponse, Error> {
    let player = dlna_player.lock().await;
    match player.stop().await {
        Ok(_) => Ok(HttpResponse::Ok().body("已停止播放")),
        Err(e) => Ok(HttpResponse::InternalServerError().body(e)),
    }
}

pub async fn get_status(
    dlna_player: web::Data<tokio::sync::Mutex<DLNAPlayer>>,
) -> Result<HttpResponse, Error> {
    let player = dlna_player.lock().await;
    match player.get_playback_status().await {
        Ok(status) => Ok(HttpResponse::Ok().json(status)),
        Err(e) => Ok(HttpResponse::InternalServerError().body(e)),
    }
}

// 新增：处理媒体文件的请求
pub async fn serve_media(path: web::Path<String>) -> Result<NamedFile, Error> {
    let media_path = PathBuf::from("media").join(path.into_inner());
    Ok(NamedFile::open(media_path)?)
} 