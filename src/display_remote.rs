use std::path::{Path, PathBuf};
use std::time::Duration;
use log::{info, error};
use local_ip_address::local_ip;
use mime_guess::from_path;
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
        info!("开始设备发现...");
        let search_target = SearchTarget::from_str("urn:schemas-upnp-org:service:AVTransport:1")
            .map_err(|e| format!("创建搜索目标失败: {}", e))?;

        let mut stream = Box::pin(rupnp::discover(&search_target, Duration::from_secs(5))
            .await
            .map_err(|e| format!("设备搜索失败: {}", e))?);
        
        let mut devices = Vec::new();
        while let Some(device_result) = stream.next().await {
            match device_result {
                Ok(device) => {
                    info!("发现设备: {}", device.url());
                    devices.push(device);
                }
                Err(e) => {
                    error!("获取设备失败: {}", e);
                    return Err(format!("获取设备失败: {}", e));
                }
            }
        }

        if devices.is_empty() {
            error!("未找到支持DLNA的设备");
            return Err("未找到支持DLNA的设备".to_string());
        }

        info!("设备发现完成，共发现 {} 个设备", devices.len());
        Ok(devices)
    }

    pub async fn connect_to_device(&mut self, device: Device) -> Result<(), String> {
        info!("尝试连接到设备: {}", device.url());
        let av_transport = device
            .find_service(&URN::service("schemas-upnp-org", "AVTransport", 1))
            .ok_or("设备不支持 AVTransport 服务")?;

        self.device = Some(device.clone());
        self.av_transport = Some(av_transport.clone());
        info!("成功连接到设备: {}", device.url());
        Ok(())
    }

    pub async fn play_video(&self, video_path: &Path) -> Result<(), String> {
        let av_transport = self.av_transport.as_ref().ok_or("未连接到设备")?;
        
        if !video_path.exists() {
            error!("文件不存在: {}", video_path.display());
            return Err(format!("文件不存在: {}", video_path.display()));
        }
        
        let local_ip = local_ip()
            .map_err(|e| format!("获取本地IP地址失败: {}", e))?;
            
        let video_url = format!(
            "http://{}:{}/media/{}",
            local_ip,
            self.media_server_port,
            video_path.file_name()
                .ok_or("无效的文件路径")?
                .to_string_lossy()
        );

        info!("播放视频URL: {}", video_url);

        let mime_type = from_path(video_path)
            .first_or_octet_stream()
            .to_string();

        info!("视频MIME类型: {}", mime_type);

        let uri = Uri::from_static("http://example.com"); // 使用实际的 URI
        let args = format!(
            "<InstanceID>0</InstanceID><CurrentURI>{}</CurrentURI><CurrentURIMetaData></CurrentURIMetaData>",
            video_url
        );

        av_transport
            .action(&uri, "SetAVTransportURI", &args)
            .await
            .map_err(|e| {
                error!("设置视频URI失败: {}", e);
                format!("设置视频URI失败: {}", e)
            })?;

        let args = "<InstanceID>0</InstanceID><Speed>1</Speed>";
        av_transport
            .action(&uri, "Play", &args)
            .await
            .map_err(|e| {
                error!("播放失败: {}", e);
                format!("播放失败: {}", e)
            })?;

        info!("视频播放开始");
        Ok(())
    }

    pub async fn pause(&self) -> Result<(), String> {
        if let Some(av_transport) = &self.av_transport {
            let uri = Uri::from_static("http://example.com"); // 使用实际的 URI
            let args = "<InstanceID>0</InstanceID>";
            av_transport
                .action(&uri, "Pause", &args)
                .await
                .map_err(|e| {
                    error!("暂停失败: {}", e);
                    format!("暂停失败: {}", e)
                })?;
            info!("视频已暂停");
        }
        Ok(())
    }

    pub async fn resume(&self) -> Result<(), String> {
        if let Some(av_transport) = &self.av_transport {
            let uri = Uri::from_static("http://example.com"); // 使用实际的 URI
            let args = "<InstanceID>0</InstanceID><Speed>1</Speed>";
            av_transport
                .action(&uri, "Play", &args)
                .await
                .map_err(|e| {
                    error!("恢复播放失败: {}", e);
                    format!("恢复播放失败: {}", e)
                })?;
            info!("视频已恢复播放");
        }
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), String> {
        if let Some(av_transport) = &self.av_transport {
            let uri = Uri::from_static("http://example.com"); // 使用实际的 URI
            let args = "<InstanceID>0</InstanceID>";
            av_transport
                .action(&uri, "Stop", &args)
                .await
                .map_err(|e| {
                    error!("停止播放失败: {}", e);
                    format!("停止播放失败: {}", e)
                })?;
            info!("视频已停止播放");
        }
        Ok(())
    }

    pub async fn get_playback_status(&self) -> Result<TransportState, String> {
        if let Some(av_transport) = &self.av_transport {
            let uri = Uri::from_static("http://example.com"); // 使用实际的 URI
            let args = "<InstanceID>0</InstanceID>";
            let response = av_transport
                .action(&uri, "GetTransportInfo", &args)
                .await
                .map_err(|e| {
                    error!("获取播放状态失败: {}", e);
                    format!("获取播放状态失败: {}", e)
                })?;

            let state = response.get("CurrentTransportState").cloned().unwrap_or("UNKNOWN".to_string());
            let transport_state = match state.as_str() {
                "PLAYING" => TransportState::Playing,
                "PAUSED_PLAYBACK" => TransportState::Paused,
                "STOPPED" => TransportState::Stopped,
                _ => TransportState::Unknown,
            };
            info!("当前播放状态: {:?}", transport_state);
            Ok(transport_state)
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
    dlna_player: web::Data<Arc<Mutex<DLNAPlayer>>>,
) -> Result<HttpResponse, Error> {
    println!("start discover_devices");
    let mut player = dlna_player.lock().await;
    match player.discover_devices().await {
        Ok(devices) => {
            let device_infos: Vec<DeviceInfo> = devices
                .iter()
                .map(|d| {
                    let friendly_name = d.friendly_name();
                    DeviceInfo {
                        name: friendly_name.to_string(),
                        location: d.url().to_string(),
                    }
                })
                .collect();
            Ok(HttpResponse::Ok().json(device_infos))
        }
        Err(e) => {
            error!("设备搜索失败: {}", e);
            Ok(HttpResponse::InternalServerError().body(e))
        }
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
    NamedFile::open(media_path).map_err(Error::from)
}

pub async fn hello() -> Result<HttpResponse, Error> {
    Ok(HttpResponse::Ok().body("Hello, the service is alive!"))
} 