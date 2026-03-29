use crate::config::AppConfig;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use log::{error, info};
use local_ip_address::local_ip;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::time::interval;

const SSDP_MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;

// Custom search target for Nascraft.
// We accept both this and "ssdp:all" in incoming queries.
pub const NASCRAFT_SSDP_ST: &str = "urn:nascraft:service:remote:1";

pub async fn run_ssdp_responder(cfg: AppConfig) {
    // 首先获取本地IP用于多播绑定
    let local_ipv4 = match local_ip() {
        Ok(IpAddr::V4(v4)) => v4,
        Ok(_) => {
            error!("SSDP: local_ip() returned non-IPv4 address");
            Ipv4Addr::new(127, 0, 0, 1)
        },
        Err(e) => {
            error!("SSDP: failed to get local IP: {}", e);
            Ipv4Addr::new(127, 0, 0, 1)
        }
    };

    // 绑定到0.0.0.0:1900，这样可以接收来自任何接口的多播流量
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), SSDP_PORT);

    info!("SSDP attempting to bind to: {}", bind_addr);

    let sock = match std::net::UdpSocket::bind(bind_addr) {
        Ok(s) => s,
        Err(e) => {
            error!("SSDP bind failed: addr={}, err={}", bind_addr, e);
            return;
        }
    };

    if let Err(e) = sock.set_nonblocking(true) {
        error!("SSDP set_nonblocking failed: {}", e);
        return;
    }

    // 尝试加入多播组到所有接口
    if let Err(e) = sock.join_multicast_v4(&SSDP_MULTICAST_ADDR, &local_ipv4) {
        error!("SSDP join_multicast_v4 failed: {}", e);
        error!("SSDP may still work for unicast M-SEARCH queries, but multicast support is limited");
    } else {
        info!("SSDP successfully joined multicast group {} on interface {}",
            SSDP_MULTICAST_ADDR, local_ipv4);
    }

    // 额外尝试加入多播组到所有接口（某些系统需要）
    match std::net::UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)) {
        Ok(multicast_sock) => {
            // 不需要额外的socket，只是记录
            info!("Additional multicast socket creation test successful");
            drop(multicast_sock);
        }
        Err(e) => {
            info!("Additional multicast socket creation test failed: {}", e);
        }
    }

    let sock = match UdpSocket::from_std(sock) {
        Ok(s) => s,
        Err(e) => {
            error!("SSDP wrap socket failed: {}", e);
            return;
        }
    };

    info!(
        "SSDP responder started: bind={}, local_ipv4={}, http_port={}, st={}",
        bind_addr, local_ipv4, cfg.server_port, NASCRAFT_SSDP_ST
    );

    let mut buf = vec![0u8; 4096];

    loop {
        let (n, peer) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                info!("SSDP recv failed: {}", e);
                continue;
            }
        };

        let msg = String::from_utf8_lossy(&buf[..n]);
        info!("SSDP received {} bytes from {}: {}", n, peer,
            msg.lines().take(5).collect::<Vec<_>>().join("; "));

        if !msg.starts_with("M-SEARCH") {
            continue;
        }

        let st = find_header_value(&msg, "st");
        let man = find_header_value(&msg, "man");
        let mx = find_header_value(&msg, "mx");

        // Must be a discovery query.
        if man.as_deref() != Some("\"ssdp:discover\"") {
            info!("SSDP: invalid MAN header, expected \"\\\"ssdp:discover\\\"\", got {:?}", man);
            continue;
        }

        let st_val = match st {
            Some(v) => v,
            None => {
                info!("SSDP: missing ST header, skipping");
                continue;
            }
        };

        if st_val != "ssdp:all" && st_val != NASCRAFT_SSDP_ST {
            info!("SSDP: ST header '{}' not matching, skipping", st_val);
            continue;
        }

        info!("SSDP: valid M-SEARCH received: st={}, from {}", st_val, peer);

        // Best-effort delay respecting MX.
        if let Some(mx) = mx.and_then(|v| v.parse::<u64>().ok()) {
            let ms = (mx * 250).min(500); // keep bounded to avoid long delays
            info!("SSDP: delaying response by {}ms (MX={})", ms, mx);
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }

        let ip = match local_ip() {
            Ok(IpAddr::V4(v4)) => v4,
            Ok(_) => local_ipv4,
            Err(_) => local_ipv4,
        };

        let location = format!("http://{}:{}/ssdp/desc.xml", ip, cfg.server_port);
        let usn = format!("uuid:nascraft-{}::{}", cfg.mdns_instance_name, NASCRAFT_SSDP_ST);

        // Date-ish token just for logging/debug.
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let resp = format!(
            "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nDATE: {}\r\nEXT:\r\nLOCATION: {}\r\nSERVER: nascraft/0.1 UPnP/1.1\r\nST: {}\r\nUSN: {}\r\n\r\n",
            ts, location, NASCRAFT_SSDP_ST, usn
        );

        info!("SSDP: sending response to {}: location={}", peer, location);
        info!("SSDP: response headers: {}", resp.lines().take(6).collect::<Vec<_>>().join("; "));

        // UPnP规范：M-SEARCH的响应应该通过单播发送回源地址
        if let Err(e) = sock.send_to(resp.as_bytes(), peer).await {
            error!("SSDP send failed: peer={}, err={}", peer, e);
        } else {
            info!("SSDP response sent successfully: peer={}, location={}, st={}", peer, location, st_val);
        }
    }
}

fn find_header_value(msg: &str, header: &str) -> Option<String> {
    let header_lower = header.to_ascii_lowercase();
    for line in msg.lines() {
        let trimmed = line.trim();
        let mut parts = trimmed.splitn(2, ':');
        let name = parts.next()?.trim().to_ascii_lowercase();
        if name != header_lower {
            continue;
        }
        let value = parts.next().unwrap_or("").trim();
        if value.is_empty() {
            return None;
        }
        return Some(value.to_string());
    }
    None
}

pub fn ssdp_routes(router: Router) -> Router {
    router.route("/ssdp/desc.xml", get(ssdp_device_desc))
}

async fn ssdp_device_desc() -> Response {
    // Minimal UPnP device description. Android client only needs LOCATION to exist.
    let xml = r#"<?xml version=\"1.0\"?>
<root xmlns=\"urn:schemas-upnp-org:device-1-0\">
  <specVersion>
    <major>1</major>
    <minor>0</minor>
  </specVersion>
  <device>
    <deviceType>urn:nascraft:device:server:1</deviceType>
    <friendlyName>Nascraft</friendlyName>
    <manufacturer>Nascraft</manufacturer>
    <modelName>Nascraft</modelName>
    <UDN>uuid:nascraft</UDN>
  </device>
</root>
"#;

    (
        [(header::CONTENT_TYPE, "text/xml; charset=utf-8")],
        xml.to_string(),
    )
        .into_response()
}

/// 主动广播SSDP NOTIFY消息
pub async fn run_ssdp_announcer(cfg: AppConfig) {
    let bind_addr = format!("0.0.0.0:{}", SSDP_PORT);
    let sock = match std::net::UdpSocket::bind(&bind_addr) {
        Ok(s) => s,
        Err(e) => {
            error!("SSDP announcer bind failed: addr={}, err={}", bind_addr, e);
            return;
        }
    };

    if let Err(e) = sock.set_nonblocking(true) {
        error!("SSDP announcer set_nonblocking failed: {}", e);
        return;
    }

    let local_ipv4 = match local_ip() {
        Ok(IpAddr::V4(v4)) => v4,
        Ok(_) => {
            error!("SSDP announcer: local_ip() returned non-IPv4 address");
            return;
        },
        Err(e) => {
            error!("SSDP announcer: failed to get local IP: {}", e);
            return;
        }
    };

    if let Err(e) = sock.join_multicast_v4(&SSDP_MULTICAST_ADDR, &local_ipv4) {
        error!("SSDP announcer join_multicast_v4 failed: {}", e);
    }

    let sock = match UdpSocket::from_std(sock) {
        Ok(s) => s,
        Err(e) => {
            error!("SSDP announcer wrap socket failed: {}", e);
            return;
        }
    };

    info!("SSDP announcer started: bind={}, interface={}", bind_addr, local_ipv4);

    let mut interval = interval(Duration::from_secs(10));

    loop {
        interval.tick().await;

        let location = format!("http://{}:{}/ssdp/desc.xml", local_ipv4, cfg.server_port);
        let usn = format!("uuid:nascraft-{}::{}", cfg.mdns_instance_name, NASCRAFT_SSDP_ST);

        let alive_notify = format!(
            "NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nCACHE-CONTROL: max-age=120\r\nLOCATION: {}\r\nNT: {}\r\nNTS: ssdp:alive\r\nSERVER: nascraft/0.1 UPnP/1.1\r\nUSN: {}\r\n\r\n",
            location, NASCRAFT_SSDP_ST, usn
        );

        let target = SocketAddr::new(IpAddr::V4(SSDP_MULTICAST_ADDR), SSDP_PORT);

        if let Err(e) = sock.send_to(alive_notify.as_bytes(), target).await {
            error!("SSDP NOTIFY send failed: target={}, err={}", target, e);
        } else {
            info!(
                "SSDP NOTIFY alive sent: server={}, location={}, usn={}",
                cfg.mdns_instance_name, location, usn
            );
        }
    }
}
