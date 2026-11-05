//! Announces the bridge on the local network, so it can be reached as `<hostname>.local`
//! and found by service browsers.

use std::collections::HashMap;

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use tracing::info;

use crate::config::Config;

/// Keep the returned daemon alive for as long as the announcement should last.
pub fn announce(cfg: &Config, hostname: &str) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    let instance = cfg
        .mdns
        .name
        .clone()
        .unwrap_or_else(|| hostname.to_string());
    let host = format!("{hostname}.local.");

    let mut props = HashMap::new();
    props.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
    props.insert("web_port".to_string(), cfg.web.port.to_string());
    if cfg.laser.enabled {
        props.insert("laser_port".to_string(), cfg.laser.port.to_string());
    }
    if cfg.camera.enabled {
        props.insert("stream".to_string(), "/stream".to_string());
        props.insert("snapshot".to_string(), "/snapshot".to_string());
    }

    for ty in ["_laserhop._tcp.local.", "_http._tcp.local."] {
        let info = ServiceInfo::new(ty, &instance, &host, (), cfg.web.port, props.clone())?
            .enable_addr_auto();
        daemon.register(info)?;
    }
    info!("mdns: announced as {hostname}.local");
    Ok(daemon)
}
