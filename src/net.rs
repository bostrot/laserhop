use std::net::{IpAddr, UdpSocket};

use anyhow::{Context, Result};
use tokio::net::TcpListener;

/// Listens on all IPv4 and IPv6 addresses, or IPv4 only where IPv6 is disabled.
pub async fn bind_tcp(port: u16) -> Result<TcpListener> {
    match TcpListener::bind(("::", port)).await {
        Ok(l) => Ok(l),
        Err(_) => TcpListener::bind(("0.0.0.0", port))
            .await
            .with_context(|| format!("cannot listen on port {port}; is another program using it?")),
    }
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "laserhop".into())
}

/// The address other machines reach this one on: the source address of the default route.
/// Connecting a UDP socket sends nothing.
pub fn primary_ip() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?;
    Some(sock.local_addr().ok()?.ip())
}
