//! The config file, `/etc/laserhop/config.toml` by default.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PATH: &str = "/etc/laserhop/config.toml";

/// Identifies a USB device by vendor and product id, and by serial number when it has one,
/// so the right device is found again after replugging or a reboot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsbMatch {
    pub vendor_id: String,
    pub product_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Only for people reading the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LaserConfig {
    pub enabled: bool,
    /// TCP port. Rayforge: "GRBL (Telnet)" with this port.
    pub port: u16,
    pub baud: u32,
    /// How long to wait for the controller's banner after opening the port.
    pub settle_ms: u64,
    /// Send one empty line after opening the port and drop the reply. Some controllers,
    /// such as the Creality Falcon A1, answer the first line after the port opens with error:2.
    pub wake_line: bool,
    /// Ask for the GRBL status ("?") every 2 s while no client is connected, for the status page.
    pub poll_status: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<UsbMatch>,
}

impl Default for LaserConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 2000,
            baud: 115200,
            settle_ms: 1500,
            wake_line: true,
            poll_status: true,
            device: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraConfig {
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<UsbMatch>,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            width: 1280,
            height: 960,
            fps: 10,
            device: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Status page, camera stream (/stream) and snapshot (/snapshot).
    pub port: u16,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self { port: 8080 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MdnsConfig {
    /// Announce the service and `<hostname>.local` on the local network.
    pub enabled: bool,
    /// Service name shown to browsers. Defaults to the hostname.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Default for MdnsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            name: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub laser: LaserConfig,
    pub camera: CameraConfig,
    pub web: WebConfig,
    pub mdns: MdnsConfig,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("invalid config in {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {} (run with sudo)", dir.display()))?;
        }
        let body = toml::to_string_pretty(self)?;
        let text = format!(
            "# laserhop config. Change it with `sudo laserhop setup`, or edit it and run\n\
             # `sudo systemctl restart laserhop`.\n\n{body}"
        );
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)
            .with_context(|| format!("cannot write {} (run with sudo)", tmp.display()))?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let mut cfg = Config::default();
        cfg.laser.enabled = true;
        cfg.laser.device = Some(UsbMatch {
            vendor_id: "303a".into(),
            product_id: "4001".into(),
            serial: None,
            name: Some("Falcon A1".into()),
        });
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.laser.device, cfg.laser.device);
        assert!(back.camera.device.is_none());
        assert_eq!(back.web.port, 8080);
    }

    #[test]
    fn missing_keys_use_defaults() {
        let cfg: Config = toml::from_str("[laser]\nport = 23\n").unwrap();
        assert_eq!(cfg.laser.port, 23);
        assert_eq!(cfg.laser.baud, 115200);
        assert!(cfg.mdns.enabled);
    }
}
