use crate::config::AppConfig;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;

#[derive(Debug, Deserialize)]
struct UdpDiscoveryProbe {
    t: String,
    v: u32,
}

#[derive(Debug, Serialize)]
struct UdpDiscoveryResponse {
    t: String,
    v: u32,
    name: String,
    proto: String,
    port: u16,
}

pub async fn run_udp_discovery_responder(cfg: AppConfig) {
    let bind_addr = format!("0.0.0.0:{}", cfg.udp_discovery_port);
    let sock = match UdpSocket::bind(&bind_addr).await {
        Ok(s) => s,
        Err(e) => {
            error!("UDP discovery bind failed: addr={}, err={}", bind_addr, e);
            return;
        }
    };

    info!(
        "UDP discovery responder started: addr={}, server_port={}",
        bind_addr, cfg.server_port
    );

    let mut buf = vec![0u8; 2048];
    loop {
        let (n, peer) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                warn!("UDP discovery recv failed: {}", e);
                continue;
            }
        };

        let probe: Result<UdpDiscoveryProbe, _> = serde_json::from_slice(&buf[..n]);
        let probe = match probe {
            Ok(p) => p,
            Err(_) => continue,
        };

        if probe.t != "nascraft_discover" || probe.v != 1 {
            continue;
        }

        let resp = UdpDiscoveryResponse {
            t: "nascraft_here".to_string(),
            v: 1,
            name: cfg.mdns_instance_name.clone(),
            proto: "http".to_string(),
            port: cfg.server_port,
        };

        let payload = match serde_json::to_vec(&resp) {
            Ok(v) => v,
            Err(e) => {
                warn!("UDP discovery encode response failed: {}", e);
                continue;
            }
        };

        if let Err(e) = sock.send_to(&payload, peer).await {
            warn!("UDP discovery send failed: peer={}, err={}", peer, e);
        }
    }
}
