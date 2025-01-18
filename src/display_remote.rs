use std::path::{Path, PathBuf};
use std::time::Duration;
use log::{info, error};
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
        
        // 创建搜索目标
        let service_type = URN::service("schemas-upnp-org", "AVTransport", 1);
        let search_target = SearchTarget::URN(service_type);
        info!("搜索目标: {:?}", search_target);

        // 开始搜索设备
        let mut stream = Box::pin(rupnp::discover(&search_target, Duration::from_secs(5))
            .await
            .map_err(|e| {
                error!("设备搜索失败: {}", e);
                format!("设备搜索失败: {}", e)
            })?);
        
        let mut devices = Vec::new();
        while let Some(device_result) = stream.next().await {
            match device_result {
                Ok(device) => {
                    info!("发现设备: {} at {}", device.friendly_name(), device.url());
                    
                    // 检查设备是否支持 AVTransport 服务
                    let service_type = URN::service("schemas-upnp-org", "AVTransport", 1);
                    if device.find_service(&service_type).is_some() {
                        info!("设备支持 AVTransport 服务");
                        devices.push(device);
                    } else {
                        info!("设备不支持 AVTransport 服务，跳过");
                    }
                }
                Err(e) => {
                    error!("获取设备失败: {}", e);
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

    pub async fn connect_to_device(&mut self, mut device: Device) -> Result<(), String> {
        info!("尝试连接到设备: {}", device.url());
        
        // 查找 AVTransport 服务
        let service_type = URN::service("schemas-upnp-org", "AVTransport", 1);
        let av_transport = device
            .find_service(&service_type)
            .ok_or_else(|| {
                error!("设备不支持 AVTransport 服务");
                "设备不支持 AVTransport 服务".to_string()
            })?
            .clone();

        info!("找到 AVTransport 服务");

        self.device = Some(device);
        self.av_transport = Some(av_transport);
        info!("成功连接到设备");
        Ok(())
    }

    pub async fn play_video(&self, video_path: &Path) -> Result<(), String> {
        let av_transport = self.av_transport.as_ref().ok_or("未连接到设备")?;
        let device = self.device.as_ref().ok_or("未连接到设备")?;
        
        if !video_path.exists() {
            error!("文件不存在: {}", video_path.display());
            return Err(format!("文件不存在: {}", video_path.display()));
        }
        
        let local_ip = local_ip()
            .map_err(|e| format!("获取本地IP地址失败: {}", e))?;
            
        let video_url = format!(
            "http://{}:{}/{}",
            local_ip,
            self.media_server_port,
            video_path.file_name()
                .ok_or("无效的文件路径")?
                .to_string_lossy()
        );

        info!("播放视频URL: {}", video_url);

        // 获取控制URL - 从设备URL中提取基础部分
        let device_url = device.url().to_string();
        let base_url = device_url.trim_end_matches("description.xml").trim_end_matches('/');
        let control_url = format!("{}/AVTransport/Control", base_url);
        info!("控制URL: {}", control_url);

        let uri = Uri::try_from(&control_url).map_err(|e| {
            error!("无效的控制URI: {}", e);
            format!("无效的控制URI: {}", e)
        })?;

        // 构造更符合DLNA标准的metadata
        let mime_type = mime_guess::from_path(video_path)
            .first_or_octet_stream()
            .to_string();
            
        let protocol_info = format!(
            "http-get:*:{}:DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000",
            mime_type
        );

        let metadata = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" 
                      xmlns:dc="http://purl.org/dc/elements/1.1/" 
                      xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/"
                      xmlns:dlna="urn:schemas-dlna-org:metadata-1-0/">
                <item id="0" parentID="-1" restricted="1">
                    <dc:title>{}</dc:title>
                    <upnp:class>object.item.videoItem</upnp:class>
                    <res protocolInfo="{}" size="0">{}</res>
                </item>
            </DIDL-Lite>"#,
            video_path.file_name().unwrap().to_string_lossy(),
            protocol_info,
            video_url
        );

        let metadata = metadata.replace('\n', "").replace("    ", "");

        // 先停止当前播放
        let stop_args = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
                <s:Body>
                    <u:Stop xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
                        <InstanceID>0</InstanceID>
                    </u:Stop>
                </s:Body>
            </s:Envelope>"#
        );

        let _ = av_transport.action(&uri, "Stop", &stop_args).await;

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // 设置新的 URI
        let set_uri_args = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
                <s:Body>
                    <u:SetAVTransportURI xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
                        <InstanceID>0</InstanceID>
                        <CurrentURI>{}</CurrentURI>
                        <CurrentURIMetaData>{}</CurrentURIMetaData>
                    </u:SetAVTransportURI>
                </s:Body>
            </s:Envelope>"#,
            video_url,
            metadata
        );

        info!("SetAVTransportURI args: {}", set_uri_args);

        av_transport
            .action(&uri, "SetAVTransportURI", &set_uri_args)
            .await
            .map_err(|e| {
                error!("设置视频URI失败: {}", e);
                format!("设置视频URI失败: {}", e)
            })?;

        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

        // 开始播放
        let play_args = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
                <s:Body>
                    <u:Play xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
                        <InstanceID>0</InstanceID>
                        <Speed>1</Speed>
                    </u:Play>
                </s:Body>
            </s:Envelope>"#
        );

        av_transport
            .action(&uri, "Play", &play_args)
            .await
            .map_err(|e| {
                error!("播放失败: {}", e);
                format!("播放失败: {}", e)
            })?;

        info!("视频播放开始");
        Ok(())
    }

    pub async fn pause(&self) -> Result<(), String> {
        let av_transport = self.av_transport.as_ref().ok_or("未连接到设备")?;
        let device = self.device.as_ref().ok_or("未连接到设备")?;
        
        let device_url = device.url().to_string();
        let base_url = device_url.trim_end_matches("description.xml").trim_end_matches('/');
        let control_url = format!("{}/AVTransport/Control", base_url);
        let uri = Uri::try_from(&control_url).map_err(|e| format!("无效的控制URI: {}", e))?;

        let pause_args = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
                <s:Body>
                    <u:Pause xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
                        <InstanceID>0</InstanceID>
                    </u:Pause>
                </s:Body>
            </s:Envelope>"#
        );

        av_transport
            .action(&uri, "Pause", &pause_args)
            .await
            .map_err(|e| format!("暂停失败: {}", e))?;

        info!("视频已暂停");
        Ok(())
    }

    pub async fn resume(&self) -> Result<(), String> {
        let av_transport = self.av_transport.as_ref().ok_or("未连接到设备")?;
        let device = self.device.as_ref().ok_or("未连接到设备")?;
        
        let device_url = device.url().to_string();
        let base_url = device_url.trim_end_matches("description.xml").trim_end_matches('/');
        let control_url = format!("{}/AVTransport/Control", base_url);
        let uri = Uri::try_from(&control_url).map_err(|e| format!("无效的控制URI: {}", e))?;

        let play_args = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
                <s:Body>
                    <u:Play xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
                        <InstanceID>0</InstanceID>
                        <Speed>1</Speed>
                    </u:Play>
                </s:Body>
            </s:Envelope>"#
        );

        av_transport
            .action(&uri, "Play", &play_args)
            .await
            .map_err(|e| format!("继续播放失败: {}", e))?;

        info!("视频继续播放");
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), String> {
        let av_transport = self.av_transport.as_ref().ok_or("未连接到设备")?;
        let device = self.device.as_ref().ok_or("未连接到设备")?;
        
        let device_url = device.url().to_string();
        let base_url = device_url.trim_end_matches("description.xml").trim_end_matches('/');
        let control_url = format!("{}/AVTransport/Control", base_url);
        let uri = Uri::try_from(&control_url).map_err(|e| format!("无效的控制URI: {}", e))?;

        let stop_args = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
                <s:Body>
                    <u:Stop xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
                        <InstanceID>0</InstanceID>
                    </u:Stop>
                </s:Body>
            </s:Envelope>"#
        );

        av_transport
            .action(&uri, "Stop", &stop_args)
            .await
            .map_err(|e| format!("停止失败: {}", e))?;

        info!("视频已停止");
        Ok(())
    }

    pub async fn get_playback_status(&self) -> Result<TransportState, String> {
        let av_transport = self.av_transport.as_ref().ok_or("未连接到设备")?;
        let device = self.device.as_ref().ok_or("未连接到设备")?;
        
        let device_url = device.url().to_string();
        let base_url = device_url.trim_end_matches("description.xml").trim_end_matches('/');
        let control_url = format!("{}/AVTransport/Control", base_url);
        let uri = Uri::try_from(&control_url).map_err(|e| format!("无效的控制URI: {}", e))?;

        let get_transport_info_args = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
            <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
                <s:Body>
                    <u:GetTransportInfo xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
                        <InstanceID>0</InstanceID>
                    </u:GetTransportInfo>
                </s:Body>
            </s:Envelope>"#
        );

        let response = av_transport
            .action(&uri, "GetTransportInfo", &get_transport_info_args)
            .await
            .map_err(|e| format!("获取播放状态失败: {}", e))?;

        info!("获取到的播放状态: {:?}", response);

        let state = response.get("CurrentTransportState")
            .map(String::as_str)
            .unwrap_or("UNKNOWN");

        let transport_state = match state {
            "PLAYING" => TransportState::Playing,
            "PAUSED_PLAYBACK" => TransportState::Paused,
            "STOPPED" => TransportState::Stopped,
            _ => TransportState::Unknown,
        };

        info!("当前播放状态: {:?}", transport_state);
        Ok(transport_state)
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

#[derive(Deserialize)]
pub struct ConnectDeviceRequest {
    device_location: String,
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
    dlna_player: web::Data<Arc<Mutex<DLNAPlayer>>>,
    req: web::Json<ConnectDeviceRequest>,
) -> Result<HttpResponse, Error> {
    let mut player = dlna_player.lock().await;
    
    // 先搜索设备
    let devices = match player.discover_devices().await {
        Ok(devices) => devices,
        Err(e) => return Ok(HttpResponse::InternalServerError().body(e)),
    };
    
    // 查找指定位置的设备
    let device = match devices.into_iter().find(|d| d.url().to_string() == req.device_location) {
        Some(device) => device,
        None => return Ok(HttpResponse::NotFound().body("设备未找到")),
    };
    
    match player.connect_to_device(device).await {
        Ok(_) => Ok(HttpResponse::Ok().body("连接成功")),
        Err(e) => Ok(HttpResponse::InternalServerError().body(e)),
    }
}

pub async fn play_video(
    dlna_player: web::Data<Arc<Mutex<DLNAPlayer>>>,
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

    // 获取本地IP地址
    let local_ip = local_ip().map_err(|e| {
        error!("获取本地IP地址失败: {}", e);
        actix_web::error::ErrorInternalServerError(e)
    })?;

    // 生成视频的局域网可访问URI
    let video_uri = format!(
        "http://{}:{}/{}",
        local_ip,
        8081, // 假设你的服务器运行在这个端口
        media_path.file_name().unwrap().to_string_lossy()
    );

    info!("生成的视频URI: {}", video_uri);

    match player.play_video(&media_path).await {
        Ok(_) => Ok(HttpResponse::Ok().body(format!("开始播放: {}", video_uri))),
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