use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use log::{info, error, debug};
use reqwest::{self, Url};
use rupnp::{Device, Service};
use ssdp_client::{SearchTarget, URN};
use serde::{Serialize, Deserialize};
use tokio::sync::Mutex;
use tokio::time;

use crate::config::AppConfig;

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceControlRequest {
    pub uuid: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayRequest {
    pub uuid: String,
    pub file_id: String,
}

/// DLNA 媒体渲染器设备
#[derive(Debug, Clone, Serialize)]
pub struct MediaRenderer {
    pub uuid: String,
    pub name: String,
    pub manufacturer: Option<String>,
    pub model_name: Option<String>,
    pub location: String,
    pub ip_addr: String,
    pub port: u16,
    pub av_transport_service: Option<ServiceInfo>,
    pub rendering_control_service: Option<ServiceInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub service_id: String,
    pub control_url: String,
    pub event_sub_url: String,
}

impl ServiceInfo {
    /// 从 Service 和设备基础 URL 创建 ServiceInfo
    /// Since all URL constructors are private in rupnp 1.1.0,
    /// we construct them by convention that control URL is at `/control` and eventsub at `/eventsub`
    /// relative to the device base URL
    pub fn from_service(service: &Service, device_url: &Url) -> Self {
        let service_id = service.service_id();

        // By DLNA convention, the control URL is typically /serviceId/control
        // and event sub is /serviceId/eventsub
        let base_path = device_url.path();
        let control_path = if base_path.ends_with('/') {
            format!("{}{}/control", base_path, service_id)
        } else {
            format!("{}/{}/control", base_path, service_id)
        };

        let event_sub_path = if base_path.ends_with('/') {
            format!("{}{}/eventsub", base_path, service_id)
        } else {
            format!("{}/{}/eventsub", base_path, service_id)
        };

        let mut control_url = device_url.clone();
        control_url.set_path(&control_path);

        let mut event_sub_url = device_url.clone();
        event_sub_url.set_path(&event_sub_path);

        ServiceInfo {
            service_id: service_id.to_string(),
            control_url: control_url.to_string(),
            event_sub_url: event_sub_url.to_string(),
        }
    }
}

/// 播放状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum PlaybackState {
    #[default]
    Unknown,
    Stopped,
    Playing,
    Paused,
    Transiting,
}

/// 当前播放信息
#[derive(Debug, Clone, Serialize, Default)]
pub struct PlaybackInfo {
    pub state: PlaybackState,
    pub current_uri: Option<String>,
    pub current_metadata: Option<String>,
    pub volume: i32,  // 0-100
    pub muted: bool,
    pub duration: Option<String>,  // HH:MM:SS
    pub position: Option<String>,  // HH:MM:SS
}

/// DLNA 渲染器管理器
pub struct RendererManager {
    devices: Arc<Mutex<HashMap<String, (MediaRenderer, PlaybackInfo)>>>,
    discovery_running: Arc<Mutex<bool>>,
}

impl RendererManager {
    pub fn new() -> Self {
        RendererManager {
            devices: Arc::new(Mutex::new(HashMap::new())),
            discovery_running: Arc::new(Mutex::new(false)),
        }
    }

    /// 开始持续发现设备
    pub async fn start_discovery(self: Arc<Self>, _config: &AppConfig) {
        let mut running = self.discovery_running.lock().await;
        if *running {
            info!("DLNA discovery already running");
            return;
        }
        *running = true;
        drop(running);

        info!("Starting DLNA MediaRenderer discovery");
        let manager_clone = self.clone();

        tokio::spawn(async move {
            manager_clone.run_discovery_loop().await;
        });
    }

    async fn run_discovery_loop(self: Arc<Self>) {
        loop {
            info!("Starting new DLNA discovery round");
            match self.search_once().await {
                Ok(_) => {
                    info!("DLNA discovery round completed");
                }
                Err(e) => {
                    error!("DLNA discovery round failed: {}", e);
                }
            }

            // 每 30 秒搜索一次更新设备列表
            time::sleep(Duration::from_secs(30)).await;
        }
    }

    /// 执行一次搜索
    async fn search_once(&self) -> Result<(), String> {
        // UPnP standard MediaRenderer
        let urn = URN::device(
            "schemas-upnp-org",
            "MediaRenderer",
            1
        );
        let search_target = SearchTarget::URN(urn);
        let devices_result = rupnp::discover(&search_target, Duration::from_secs(5)).await;
        let mut devices = match devices_result {
            Ok(devices) => Box::pin(devices),
            Err(e) => return Err(format!("Search failed: {}", e)),
        };

        info!("DLNA discovery started");

        let mut device_map = self.devices.lock().await;

        while let Some(result) = devices.next().await {
            let device = match result {
                Ok(d) => d,
                Err(e) => {
                    error!("Failed to get device from search: {}", e);
                    continue;
                }
            };

            self.process_device(&device, &mut device_map);
        }

        info!("DLNA discovery round finished, found {} devices", device_map.len());
        Ok(())
    }

    /// 处理发现的设备
    fn process_device(&self, device: &Device, device_map: &mut HashMap<String, (MediaRenderer, PlaybackInfo)>) {
        // Extract UUID from UDN
        let udn = device.udn();
        let uuid = udn.strip_prefix("uuid:").unwrap_or(udn).to_string();

        // Extract location URL
        let location = device.url().to_string();

        // 提取设备信息
        let name = device.friendly_name();
        let name = if name.is_empty() { "Unknown Renderer" } else { name }.to_string();

        // manufacturer and model_name return &str, may be empty
        let manufacturer = {
            let m = device.manufacturer();
            if m.is_empty() { None } else { Some(m.to_string()) }
        };
        let model_name = {
            let m = device.model_name();
            if m.is_empty() { None } else { Some(m.to_string()) }
        };

        // 解析 IP 和端口
        let ip_addr = match Url::parse(&location) {
            Ok(url) => url.host_str().unwrap_or("").to_string(),
            Err(_) => "".to_string(),
        };
        let port = match Url::parse(&location) {
            Ok(url) => url.port().unwrap_or(80),
            Err(_) => 80,
        };

        // 查找必需的服务
        let av_transport = find_service(device, "AVTransport");
        let rendering_control = find_service(device, "RenderingControl");

        let device_url = device.url();
        let device_url_parsed = match Url::parse(&device_url.to_string()) {
            Ok(url) => url,
            Err(e) => {
                error!("Invalid device URL: {}", e);
                return;
            }
        };

        let renderer = MediaRenderer {
            uuid: uuid.clone(),
            name,
            manufacturer,
            model_name,
            location: location.clone(),
            ip_addr,
            port,
            av_transport_service: av_transport.map(|s| ServiceInfo::from_service(s, &device_url_parsed)),
            rendering_control_service: rendering_control.map(|s| ServiceInfo::from_service(s, &device_url_parsed)),
        };

        // 如果是新设备或已更新，添加到列表
        if device_map.contains_key(&uuid) {
            info!("Updating existing DLNA device: {} ({})", renderer.name, uuid);
        } else {
            info!("Found new DLNA MediaRenderer: {} ({}) at {}", renderer.name, uuid, location);
        }

        // 获取之前的播放信息，如果有的话
        let playback_info = if let Some((_, old_info)) = device_map.get(&uuid) {
            old_info.clone()
        } else {
            PlaybackInfo::default()
        };

        device_map.insert(uuid, (renderer, playback_info));
    }

    /// 获取所有已发现的渲染器
    pub async fn list_devices(&self) -> Vec<(MediaRenderer, PlaybackInfo)> {
        let device_map = self.devices.lock().await;
        device_map.values().cloned().collect()
    }

    /// 获取单个渲染器
    pub async fn get_device(&self, uuid: &str) -> Option<(MediaRenderer, PlaybackInfo)> {
        let device_map = self.devices.lock().await;
        device_map.get(uuid).cloned()
    }

    /// 播放指定 URI 的视频
    pub async fn play_uri(&self, uuid: &str, uri: String, metadata: Option<String>) -> Result<(), String> {
        let (device, _) = match self.get_device(uuid).await {
            Some(d) => d,
            None => return Err(format!("Device not found: {}", uuid)),
        };

        let av_service = match &device.av_transport_service {
            Some(s) => s,
            None => return Err("Device does not support AVTransport".to_string()),
        };

        // 1. 设置 URI
        self.set_av_transport_uri(&device, av_service, &uri, metadata.as_deref()).await?;

        // 2. 开始播放
        self.play_remote(&device, av_service).await?;

        // 更新缓存的播放信息
        let mut device_map = self.devices.lock().await;
        if let Some((_, info)) = device_map.get_mut(uuid) {
            info.state = PlaybackState::Playing;
            info.current_uri = Some(uri);
            info.current_metadata = metadata.map(|s| s.clone());
        }

        Ok(())
    }

    /// 执行 SetAVTransportURI 动作
    async fn set_av_transport_uri(&self, device: &MediaRenderer, service: &ServiceInfo, uri: &str, metadata: Option<&str>) -> Result<(), String> {
        let control_url = self.make_control_url(device, service);
        let action = "SetAVTransportURI";

        // 构建 SOAP 请求
        let meta = metadata.unwrap_or("");
        let soap = format!(r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:SetAVTransportURI xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
      <InstanceID>0</InstanceID>
      <CurrentURI>{}</CurrentURI>
      <CurrentURIMetaData>{}</CurrentURIMetaData>
    </u:SetAVTransportURI>
  </s:Body>
</s:Envelope>"#, uri, meta);

        self.execute_soap_action(&control_url, "urn:schemas-upnp-org:service:AVTransport:1", action, &soap).await?;
        info!("SetAVTransportURI completed for {} on {}", uri, device.name);
        Ok(())
    }

    /// 开始播放
    pub async fn play(&self, uuid: &str) -> Result<(), String> {
        let (device, _) = match self.get_device(uuid).await {
            Some(d) => d,
            None => return Err(format!("Device not found: {}", uuid)),
        };

        let av_service = match &device.av_transport_service {
            Some(s) => s,
            None => return Err("Device does not support AVTransport".to_string()),
        };

        self.play_remote(&device, av_service).await?;

        // 更新缓存
        let mut device_map = self.devices.lock().await;
        if let Some((_, info)) = device_map.get_mut(uuid) {
            info.state = PlaybackState::Playing;
        }

        Ok(())
    }

    async fn play_remote(&self, device: &MediaRenderer, service: &ServiceInfo) -> Result<(), String> {
        let control_url = self.make_control_url(device, service);
        let action = "Play";

        let soap = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:Play xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
      <InstanceID>0</InstanceID>
      <Speed>1</Speed>
    </u:Play>
  </s:Body>
</s:Envelope>"#;

        self.execute_soap_action(&control_url, "urn:schemas-upnp-org:service:AVTransport:1", action, soap).await?;
        info!("Play completed on {}", device.name);
        Ok(())
    }

    /// 暂停播放
    pub async fn pause(&self, uuid: &str) -> Result<(), String> {
        let (device, _) = match self.get_device(uuid).await {
            Some(d) => d,
            None => return Err(format!("Device not found: {}", uuid)),
        };

        let av_service = match &device.av_transport_service {
            Some(s) => s,
            None => return Err("Device does not support AVTransport".to_string()),
        };

        let control_url = self.make_control_url(&device, av_service);
        let action = "Pause";

        let soap = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:Pause xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
      <InstanceID>0</InstanceID>
    </u:Pause>
  </s:Body>
</s:Envelope>"#;

        self.execute_soap_action(&control_url, "urn:schemas-upnp-org:service:AVTransport:1", action, soap).await?;

        // 更新缓存
        let mut device_map = self.devices.lock().await;
        if let Some((_, info)) = device_map.get_mut(uuid) {
            info.state = PlaybackState::Paused;
        }

        info!("Pause completed on {}", device.name);
        Ok(())
    }

    /// 停止播放
    pub async fn stop(&self, uuid: &str) -> Result<(), String> {
        let (device, _) = match self.get_device(uuid).await {
            Some(d) => d,
            None => return Err(format!("Device not found: {}", uuid)),
        };

        let av_service = match &device.av_transport_service {
            Some(s) => s,
            None => return Err("Device does not support AVTransport".to_string()),
        };

        let control_url = self.make_control_url(&device, av_service);
        let action = "Stop";

        let soap = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:Stop xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
      <InstanceID>0</InstanceID>
    </u:Stop>
  </s:Body>
</s:Envelope>"#;

        self.execute_soap_action(&control_url, "urn:schemas-upnp-org:service:AVTransport:1", action, soap).await?;

        // 更新缓存
        let mut device_map = self.devices.lock().await;
        if let Some((_, info)) = device_map.get_mut(uuid) {
            info.state = PlaybackState::Stopped;
            info.current_uri = None;
        }

        info!("Stop completed on {}", device.name);
        Ok(())
    }

    /// 设置音量 (0-100)
    pub async fn set_volume(&self, uuid: &str, volume: i32) -> Result<(), String> {
        let (device, _) = match self.get_device(uuid).await {
            Some(d) => d,
            None => return Err(format!("Device not found: {}", uuid)),
        };

        let rc_service = match &device.rendering_control_service {
            Some(s) => s,
            None => return Err("Device does not support RenderingControl".to_string()),
        };

        let control_url = self.make_control_url(&device, rc_service);
        let action = "SetVolume";

        // 剪辑到 0-100
        let volume = volume.clamp(0, 100);

        let soap = format!(r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:SetVolume xmlns:u="urn:schemas-upnp-org:service:RenderingControl:1">
      <InstanceID>0</InstanceID>
      <Channel>Master</Channel>
      <DesiredVolume>{}</DesiredVolume>
    </u:SetVolume>
  </s:Body>
</s:Envelope>"#, volume);

        self.execute_soap_action(&control_url, "urn:schemas-upnp-org:service:RenderingControl:1", action, &soap).await?;

        // 更新缓存
        let mut device_map = self.devices.lock().await;
        if let Some((_, info)) = device_map.get_mut(uuid) {
            info.volume = volume;
        }

        info!("SetVolume {} completed on {}", volume, device.name);
        Ok(())
    }

    /// 设置静音
    pub async fn set_mute(&self, uuid: &str, mute: bool) -> Result<(), String> {
        let (device, _) = match self.get_device(uuid).await {
            Some(d) => d,
            None => return Err(format!("Device not found: {}", uuid)),
        };

        let rc_service = match &device.rendering_control_service {
            Some(s) => s,
            None => return Err("Device does not support RenderingControl".to_string()),
        };

        let control_url = self.make_control_url(&device, rc_service);
        let action = "SetMute";

        let mute_val = if mute { 1 } else { 0 };

        let soap = format!(r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:SetMute xmlns:u="urn:schemas-upnp-org:service:RenderingControl:1">
      <InstanceID>0</InstanceID>
      <Channel>Master</Channel>
      <DesiredMute>{}</DesiredMute>
    </u:SetMute>
  </s:Body>
</s:Envelope>"#, mute_val);

        self.execute_soap_action(&control_url, "urn:schemas-upnp-org:service:RenderingControl:1", action, &soap).await?;

        // 更新缓存
        let mut device_map = self.devices.lock().await;
        if let Some((_, info)) = device_map.get_mut(uuid) {
            info.muted = mute;
        }

        info!("SetMute {} completed on {}", mute, device.name);
        Ok(())
    }

    /// 跳转到指定位置
    pub async fn seek(&self, uuid: &str, time_seconds: u32) -> Result<(), String> {
        let (device, _) = match self.get_device(uuid).await {
            Some(d) => d,
            None => return Err(format!("Device not found: {}", uuid)),
        };

        let av_service = match &device.av_transport_service {
            Some(s) => s,
            None => return Err("Device does not support AVTransport".to_string()),
        };

        let control_url = self.make_control_url(&device, av_service);
        let action = "Seek";

        // 转换为 HH:MM:SS
        let hours = time_seconds / 3600;
        let minutes = (time_seconds % 3600) / 60;
        let seconds = time_seconds % 60;
        let time_str = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);

        let soap = format!(r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"
            s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:Seek xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
      <InstanceID>0</InstanceID>
      <Unit>REL_TIME</Unit>
      <Target>{}</Target>
    </u:Seek>
  </s:Body>
</s:Envelope>"#, time_str);

        self.execute_soap_action(&control_url, "urn:schemas-upnp-org:service:AVTransport:1", action, &soap).await?;
        info!("Seek to {} completed on {}", time_str, device.name);
        Ok(())
    }

    /// 构造完整的控制 URL
    fn make_control_url(&self, device: &MediaRenderer, service: &ServiceInfo) -> String {
        // 如果 control_url 已经是绝对 URL，直接使用
        if service.control_url.starts_with("http://") || service.control_url.starts_with("https://") {
            return service.control_url.clone();
        }

        // 否则，基于 location 拼接
        if let Ok(mut url) = Url::parse(&device.location) {
            url.set_path(&service.control_url);
            url.to_string()
        } else {
            format!("http://{}:{}{}", device.ip_addr, device.port, service.control_url)
        }
    }

    /// 执行 SOAP 动作
    async fn execute_soap_action(&self, control_url: &str, service_type: &str, action: &str, body: &str) -> Result<(), String> {
        let client = reqwest::Client::new();

        debug!("Executing SOAP action {} on {}", action, control_url);

        let response = match client.post(control_url)
            .header("Content-Type", "text/xml; charset=utf-8")
            .header("SOAPACTION", format!("\"{}#{}\"", service_type, action))
            .body(body.to_string())
            .send()
            .await {
                Ok(r) => r,
                Err(e) => return Err(format!("Request failed: {}", e)),
            };

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_else(|_| "".to_string());
            return Err(format!("SOAP action failed with status {}: {}", status, error_body));
        }

        Ok(())
    }
}

/// 查找指定服务类型的服务
fn find_service<'a>(device: &'a Device, service_type: &str) -> Option<&'a Service> {
    for service in device.services() {
        let service_id = service.service_type().to_string();
        if service_id.contains(service_type) {
            return Some(service);
        }
    }
    None
}
