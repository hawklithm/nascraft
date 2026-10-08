use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use log::{info, error, debug};
use rupnp::{Device, Service};
use ssdp_client::{SearchTarget, URN};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::time;

use crate::config::AppConfig;

/// 设备连续多久未被重新发现就隐藏（发现轮询间隔约 35s，10 分钟 ≈ 17 轮兜底；
/// SSDP 多播本身不可靠，阈值过短会让设备在网络抖动后从列表消失）
const DEVICE_STALE_AFTER: Duration = Duration::from_secs(600);
/// 后台轮询的单轮搜索窗口
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// 打开设备列表时的即时搜索窗口（短一点，避免接口等待过久）
const ON_DEMAND_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(3);
const AV_TRANSPORT: &str = "AVTransport";
const RENDERING_CONTROL: &str = "RenderingControl";

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceControlRequest {
    pub uuid: String,
}

/// SOAP 参数是直接插入 XML 文本节点的，必须先转义
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// DLNA 媒体渲染器设备（对前端的 JSON 快照）
#[derive(Debug, Clone, Serialize)]
pub struct MediaRenderer {
    pub uuid: String,
    pub name: String,
    pub manufacturer: Option<String>,
    pub model_name: Option<String>,
    pub location: String,
    pub ip_addr: String,
    pub port: u16,
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

/// 设备缓存条目：JSON 快照 + 可直接用于控制的 rupnp Device
struct DeviceEntry {
    device: Device,
    renderer: MediaRenderer,
    playback: PlaybackInfo,
    last_seen: Instant,
}

/// DLNA 渲染器管理器：
/// - 发现：rupnp SSDP M-SEARCH（MediaRenderer:1），每 30s 一轮
/// - 控制：rupnp service.action()，使用设备描述 XML 里的真实 controlURL
pub struct RendererManager {
    devices: Arc<Mutex<HashMap<String, DeviceEntry>>>,
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

    /// 执行一次 SSDP 搜索（默认窗口）并合并进设备表
    async fn search_once(&self) -> Result<(), String> {
        self.search_with_timeout(DISCOVERY_TIMEOUT).await
    }

    /// 打开设备列表时主动搜索一轮，再返回最新列表
    pub async fn refresh_and_list(&self) -> Vec<(MediaRenderer, PlaybackInfo)> {
        if let Err(e) = self.search_with_timeout(ON_DEMAND_DISCOVERY_TIMEOUT).await {
            debug!("On-demand DLNA discovery round failed: {}", e);
        }
        self.list_devices().await
    }

    async fn search_with_timeout(&self, timeout: Duration) -> Result<(), String> {
        // UPnP standard MediaRenderer
        let urn = URN::device(
            "schemas-upnp-org",
            "MediaRenderer",
            1
        );
        let search_target = SearchTarget::URN(urn);
        let devices_result = rupnp::discover(&search_target, timeout).await;
        let mut devices = match devices_result {
            Ok(devices) => Box::pin(devices),
            Err(e) => return Err(format!("Search failed: {}", e)),
        };

        info!("DLNA discovery started");

        // 先消费完发现流（不持有设备表锁），再短锁合并
        let mut found: Vec<Device> = Vec::new();
        while let Some(result) = devices.next().await {
            match result {
                Ok(d) => found.push(d),
                Err(e) => {
                    error!("Failed to get device from search: {}", e);
                    continue;
                }
            }
        }

        let mut device_map = self.devices.lock().await;
        for device in found {
            let renderer = build_renderer(&device);
            let uuid = renderer.uuid.clone();

            if device_map.contains_key(&uuid) {
                info!("Updating existing DLNA device: {} ({})", renderer.name, uuid);
            } else {
                info!("Found new DLNA MediaRenderer: {} ({}) at {}", renderer.name, uuid, renderer.location);
            }

            let playback = device_map
                .get(&uuid)
                .map(|e| e.playback.clone())
                .unwrap_or_default();

            device_map.insert(uuid, DeviceEntry {
                device,
                renderer,
                playback,
                last_seen: Instant::now(),
            });
        }

        info!("DLNA discovery round finished, {} devices known", device_map.len());
        Ok(())
    }

    /// 获取所有未被判定离线的渲染器
    pub async fn list_devices(&self) -> Vec<(MediaRenderer, PlaybackInfo)> {
        let device_map = self.devices.lock().await;
        device_map
            .values()
            .filter(|e| e.last_seen.elapsed() < DEVICE_STALE_AFTER)
            .map(|e| (e.renderer.clone(), e.playback.clone()))
            .collect()
    }

    /// 在锁内取出可克隆的控制句柄，网络调用在锁外执行
    async fn call_action(&self, uuid: &str, service_name: &str, action: &str, args: &str) -> Result<(), String> {
        let (device, service) = {
            let device_map = self.devices.lock().await;
            let entry = device_map
                .get(uuid)
                .ok_or_else(|| format!("Device not found: {}", uuid))?;
            let service = find_service(&entry.device, service_name)
                .ok_or_else(|| format!("Device does not support {}", service_name))?;
            (entry.device.clone(), service.clone())
        };

        service
            .action(device.url(), action, args)
            .await
            .map_err(|e| format!("SOAP action {} failed: {}", action, e))?;

        debug!("SOAP action {} on {} completed", action, uuid);
        Ok(())
    }

    async fn call_av_action(&self, uuid: &str, action: &str, args: &str) -> Result<(), String> {
        self.call_action(uuid, AV_TRANSPORT, action, args).await
    }

    async fn patch_playback(&self, uuid: &str, f: impl FnOnce(&mut PlaybackInfo)) {
        let mut device_map = self.devices.lock().await;
        if let Some(entry) = device_map.get_mut(uuid) {
            f(&mut entry.playback);
        }
    }

    /// 播放指定 URI 的视频（SetAVTransportURI + Play）
    pub async fn play_uri(&self, uuid: &str, uri: String, metadata: Option<String>) -> Result<(), String> {
        let escaped_uri = xml_escape(&uri);
        let escaped_meta = metadata.as_deref().map(xml_escape).unwrap_or_default();
        let args = format!(
            "<InstanceID>0</InstanceID><CurrentURI>{}</CurrentURI><CurrentURIMetaData>{}</CurrentURIMetaData>",
            escaped_uri, escaped_meta
        );

        self.call_av_action(uuid, "SetAVTransportURI", &args).await?;
        self.call_av_action(uuid, "Play", "<InstanceID>0</InstanceID><Speed>1</Speed>").await?;

        // 更新缓存的播放信息
        self.patch_playback(uuid, |p| {
            p.state = PlaybackState::Playing;
            p.current_uri = Some(uri);
            p.current_metadata = metadata;
        }).await;

        Ok(())
    }

    /// 开始播放
    pub async fn play(&self, uuid: &str) -> Result<(), String> {
        self.call_av_action(uuid, "Play", "<InstanceID>0</InstanceID><Speed>1</Speed>").await?;
        self.patch_playback(uuid, |p| p.state = PlaybackState::Playing).await;
        Ok(())
    }

    /// 暂停播放
    pub async fn pause(&self, uuid: &str) -> Result<(), String> {
        self.call_av_action(uuid, "Pause", "<InstanceID>0</InstanceID>").await?;
        self.patch_playback(uuid, |p| p.state = PlaybackState::Paused).await;
        Ok(())
    }

    /// 停止播放
    pub async fn stop(&self, uuid: &str) -> Result<(), String> {
        self.call_av_action(uuid, "Stop", "<InstanceID>0</InstanceID>").await?;
        self.patch_playback(uuid, |p| {
            p.state = PlaybackState::Stopped;
            p.current_uri = None;
        }).await;
        Ok(())
    }

    /// 设置音量 (0-100)
    pub async fn set_volume(&self, uuid: &str, volume: i32) -> Result<(), String> {
        let volume = volume.clamp(0, 100);
        let args = format!(
            "<InstanceID>0</InstanceID><Channel>Master</Channel><DesiredVolume>{}</DesiredVolume>",
            volume
        );
        self.call_action(uuid, RENDERING_CONTROL, "SetVolume", &args).await?;
        self.patch_playback(uuid, |p| p.volume = volume).await;
        Ok(())
    }

    /// 设置静音
    pub async fn set_mute(&self, uuid: &str, mute: bool) -> Result<(), String> {
        let args = format!(
            "<InstanceID>0</InstanceID><Channel>Master</Channel><DesiredMute>{}</DesiredMute>",
            if mute { 1 } else { 0 }
        );
        self.call_action(uuid, RENDERING_CONTROL, "SetMute", &args).await?;
        self.patch_playback(uuid, |p| p.muted = mute).await;
        Ok(())
    }

    /// 跳转到指定位置（秒）
    pub async fn seek(&self, uuid: &str, time_seconds: u32) -> Result<(), String> {
        let hours = time_seconds / 3600;
        let minutes = (time_seconds % 3600) / 60;
        let seconds = time_seconds % 60;
        let time_str = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);

        let args = format!(
            "<InstanceID>0</InstanceID><Unit>REL_TIME</Unit><Target>{}</Target>",
            time_str
        );
        self.call_av_action(uuid, "Seek", &args).await?;
        info!("Seek to {} on {} completed", time_str, uuid);
        Ok(())
    }
}

/// 从 rupnp Device 构建 JSON 快照
fn build_renderer(device: &Device) -> MediaRenderer {
    let udn = device.udn();
    let uuid = udn.strip_prefix("uuid:").unwrap_or(udn).to_string();
    let location = device.url().to_string();

    let name = {
        let n = device.friendly_name();
        if n.is_empty() { "Unknown Renderer" } else { n }.to_string()
    };
    let manufacturer = {
        let m = device.manufacturer();
        if m.is_empty() { None } else { Some(m.to_string()) }
    };
    let model_name = {
        let m = device.model_name();
        if m.is_empty() { None } else { Some(m.to_string()) }
    };

    let ip_addr = device.url().host().unwrap_or("").to_string();
    let port = device.url().port_u16().unwrap_or(80);

    MediaRenderer {
        uuid,
        name,
        manufacturer,
        model_name,
        location,
        ip_addr,
        port,
    }
}

/// 查找指定名称的服务（宽松匹配，兼容 AVTransport:2/3 等版本变体）
fn find_service<'a>(device: &'a Device, name: &str) -> Option<&'a Service> {
    device
        .services_iter()
        .find(|s| s.service_type().to_string().contains(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_covers_special_chars() {
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(xml_escape("say \"hi\" 'ok'"), "say &quot;hi&quot; &apos;ok&apos;");
    }

    #[test]
    fn xml_escape_handles_url_query() {
        // 投屏 URI 带查询参数时，& 必须转义，否则 SOAP XML 会解析失败
        assert_eq!(
            xml_escape("http://192.168.1.5:8080/api/download/abc?a=1&b=2"),
            "http://192.168.1.5:8080/api/download/abc?a=1&amp;b=2"
        );
    }

    #[test]
    fn xml_escape_keeps_normal_text() {
        assert_eq!(xml_escape("plain text 123"), "plain text 123");
        assert_eq!(xml_escape(""), "");
    }
}
