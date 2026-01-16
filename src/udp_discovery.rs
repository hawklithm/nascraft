use crate::config::AppConfig;
use log::{error, info};
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
                info!("UDP discovery recv failed: {}", e);
                continue;
            }
        };

        let probe: Result<UdpDiscoveryProbe, _> = serde_json::from_slice(&buf[..n]);
        let probe = match probe {
            Ok(p) => p,
            Err(e) => {
                info!("UDP discovery: ignoring invalid JSON from {}: {}", peer, e);
                continue;
            }
        };

        if probe.t != "nascraft_discover" || probe.v != 1 {
            info!(
                "UDP discovery: ignoring unexpected probe from {}: t={}, v={}",
                peer, probe.t, probe.v
            );
            continue;
        }

        info!("UDP discovery probe received: peer={}", peer);

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
                info!("UDP discovery encode response failed: {}", e);
                continue;
            }
        };

        if let Err(e) = sock.send_to(&payload, peer).await {
            info!("UDP discovery send failed: peer={}, err={}", peer, e);
        } else {
            info!(
                "UDP discovery response sent: peer={}, server_port={}, bytes={}",
                peer,
                cfg.server_port,
                payload.len()
            );
        }
    }
}
