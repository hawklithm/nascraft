use std::env;
use log::info;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub server_port: u16,
    pub mdns_service_type: String,
    pub mdns_instance_name: String,
    pub udp_discovery_port: u16,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let server_port: u16 = env::var("NASCRAFT_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(8080);

        let mdns_service_type = env::var("NASCRAFT_MDNS_SERVICE_TYPE")
            .unwrap_or_else(|_| "_nascraft._tcp.local.".to_string());

        let mdns_instance_name = env::var("NASCRAFT_MDNS_INSTANCE")
            .unwrap_or_else(|_| "nascraft".to_string());

        let udp_discovery_port: u16 = env::var("NASCRAFT_UDP_DISCOVERY_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(53530);

        info!(
            "Loaded config: server_port={}, mdns_service_type={}, mdns_instance_name={}, udp_discovery_port={}",
            server_port, mdns_service_type, mdns_instance_name, udp_discovery_port
        );

        Self {
            server_port,
            mdns_service_type,
            mdns_instance_name,
            udp_discovery_port,
        }
    }
}
