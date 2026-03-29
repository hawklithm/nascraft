use crate::config::AppConfig;
use log::{error, info};
use local_ip_address::local_ip;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;

pub fn start_mdns_advertise(cfg: &AppConfig) -> std::io::Result<ServiceDaemon> {
    let mdns = ServiceDaemon::new().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, format!("Failed to create mDNS daemon: {e}"))
    })?;

    let host_name = format!("{}.local.", cfg.mdns_instance_name);
    info!(
        "mDNS advertise init: service_type={}, instance_name={}, hostname={}",
        cfg.mdns_service_type, cfg.mdns_instance_name, host_name
    );
    let mut mdns_properties: HashMap<String, String> = HashMap::new();
    mdns_properties.insert("proto".to_string(), "http".to_string());
    mdns_properties.insert("port".to_string(), cfg.server_port.to_string());

    let ip = local_ip().unwrap_or_else(|e| {
        error!("Failed to get local IP: {}", e);
        "127.0.0.1".parse().expect("127.0.0.1 should be valid")
    });

    info!("mDNS advertise address: ip={}, port={}", ip, cfg.server_port);

    let service_info = ServiceInfo::new(
        &cfg.mdns_service_type,
        &cfg.mdns_instance_name,
        &host_name,
        ip,
        cfg.server_port,
        mdns_properties,
    )
    .map(|s| s.enable_addr_auto())
    .map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to create mDNS service info: {e}"),
        )
    })?;

    mdns.register(service_info).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to register mDNS service: {e}"),
        )
    })?;

    info!(
        "mDNS service registered: type={}, instance={}, hostname={}, ip={}, port={}",
        cfg.mdns_service_type, cfg.mdns_instance_name, host_name, ip, cfg.server_port
    );

    Ok(mdns)
}

pub fn shutdown_mdns(mdns: ServiceDaemon) {
    if let Err(e) = mdns.shutdown() {
        error!("mDNS shutdown failed: {}", e);
    }
}
