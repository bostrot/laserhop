//! Finds USB serial ports and cameras through sysfs.

use std::{
    fs,
    path::{Path, PathBuf},
};

use linuxvideo::{BufType, CapabilityFlags, Device, format::FrameSizes, format::PixelFormat};

use crate::config::UsbMatch;

#[derive(Debug, Clone)]
pub struct UsbInfo {
    pub vendor_id: String,
    pub product_id: String,
    pub serial: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

impl UsbInfo {
    pub fn label(&self) -> String {
        match (&self.manufacturer, &self.product) {
            (Some(m), Some(p)) if !p.starts_with(m.as_str()) => format!("{m} {p}"),
            (_, Some(p)) => p.clone(),
            (Some(m), None) => m.clone(),
            (None, None) => "Unknown USB device".into(),
        }
    }

    pub fn id(&self) -> String {
        format!("{}:{}", self.vendor_id, self.product_id)
    }

    pub fn matches(&self, m: &UsbMatch) -> bool {
        self.vendor_id.eq_ignore_ascii_case(&m.vendor_id)
            && self.product_id.eq_ignore_ascii_case(&m.product_id)
            && (m.serial.is_none() || m.serial == self.serial)
    }

    pub fn to_match(&self, name: String) -> UsbMatch {
        UsbMatch {
            vendor_id: self.vendor_id.clone(),
            product_id: self.product_id.clone(),
            serial: self.serial.clone(),
            name: Some(name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SerialPort {
    pub path: PathBuf,
    pub usb: UsbInfo,
}

#[derive(Debug, Clone)]
pub struct Camera {
    pub path: PathBuf,
    pub usb: UsbInfo,
    pub name: String,
    /// MJPEG frame sizes, largest first.
    pub sizes: Vec<(u32, u32)>,
}

fn read_attr(dir: &Path, name: &str) -> Option<String> {
    let s = fs::read_to_string(dir.join(name)).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Walks up from a class device to the USB device that owns it.
fn usb_info(class_dev: &Path) -> Option<UsbInfo> {
    let mut dir = fs::canonicalize(class_dev.join("device")).ok()?;
    loop {
        if dir.join("idVendor").exists() {
            return Some(UsbInfo {
                vendor_id: read_attr(&dir, "idVendor")?,
                product_id: read_attr(&dir, "idProduct")?,
                serial: read_attr(&dir, "serial"),
                manufacturer: read_attr(&dir, "manufacturer"),
                product: read_attr(&dir, "product"),
            });
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn class_entries(class: &str, prefixes: &[&str]) -> Vec<(String, PathBuf)> {
    let mut out: Vec<_> = fs::read_dir(Path::new("/sys/class").join(class))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            prefixes
                .iter()
                .any(|p| name.starts_with(p))
                .then(|| (name, e.path()))
        })
        .collect();
    out.sort_by(|a, b| natural_key(&a.0).cmp(&natural_key(&b.0)));
    out
}

fn natural_key(name: &str) -> (String, u32) {
    let digits = name.trim_start_matches(|c: char| !c.is_ascii_digit());
    let prefix = &name[..name.len() - digits.len()];
    (prefix.to_string(), digits.parse().unwrap_or(0))
}

/// USB serial ports (`ttyACM*`, `ttyUSB*`).
pub fn serial_ports() -> Vec<SerialPort> {
    class_entries("tty", &["ttyACM", "ttyUSB"])
        .into_iter()
        .filter_map(|(name, sys)| {
            Some(SerialPort {
                path: Path::new("/dev").join(&name),
                usb: usb_info(&sys)?,
            })
        })
        .collect()
}

/// USB cameras that can capture MJPEG.
pub fn cameras() -> Vec<Camera> {
    class_entries("video4linux", &["video"])
        .into_iter()
        .filter_map(|(name, sys)| {
            // A UVC camera has a second node for metadata; only index 0 captures video.
            if read_attr(&sys, "index").is_some_and(|i| i != "0") {
                return None;
            }
            let usb = usb_info(&sys)?;
            let path = Path::new("/dev").join(&name);
            let sizes = mjpeg_sizes(&path)?;
            Some(Camera {
                name: read_attr(&sys, "name").unwrap_or_else(|| usb.label()),
                path,
                usb,
                sizes,
            })
        })
        .collect()
}

/// Returns `None` if the device cannot capture MJPEG.
fn mjpeg_sizes(path: &Path) -> Option<Vec<(u32, u32)>> {
    let dev = Device::open(path).ok()?;
    let caps = dev.capabilities().ok()?.device_capabilities();
    if !caps.contains(CapabilityFlags::VIDEO_CAPTURE) {
        return None;
    }
    let has_mjpeg = dev
        .formats(BufType::VIDEO_CAPTURE)
        .flatten()
        .any(|f| f.pixel_format() == PixelFormat::MJPG);
    if !has_mjpeg {
        return None;
    }
    let mut sizes = match dev.frame_sizes(PixelFormat::MJPG) {
        Ok(FrameSizes::Discrete(list)) => list.iter().map(|s| (s.width(), s.height())).collect(),
        Ok(FrameSizes::Stepwise(s) | FrameSizes::Continuous(s)) => {
            vec![(s.max_width(), s.max_height())]
        }
        Err(_) => Vec::new(),
    };
    sizes.sort_by_key(|&(w, h)| std::cmp::Reverse(w * h));
    sizes.dedup();
    Some(sizes)
}

pub fn find_serial(m: &UsbMatch) -> Option<PathBuf> {
    serial_ports()
        .into_iter()
        .find(|p| p.usb.matches(m))
        .map(|p| p.path)
}

/// Finds the camera's capture node without opening it, so this is cheap to call in a loop.
pub fn find_camera(m: &UsbMatch) -> Option<PathBuf> {
    class_entries("video4linux", &["video"])
        .into_iter()
        .find(|(_, sys)| {
            read_attr(sys, "index").is_none_or(|i| i == "0")
                && usb_info(sys).is_some_and(|u| u.matches(m))
        })
        .map(|(name, _)| Path::new("/dev").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_device_names_numerically() {
        let mut names = vec!["ttyACM10", "ttyACM2", "ttyUSB0", "ttyACM0"];
        names.sort_by_key(|n| natural_key(n));
        assert_eq!(names, ["ttyACM0", "ttyACM2", "ttyACM10", "ttyUSB0"]);
    }

    #[test]
    fn matches_serial_only_when_configured() {
        let usb = UsbInfo {
            vendor_id: "303a".into(),
            product_id: "4001".into(),
            serial: Some("ABC".into()),
            manufacturer: None,
            product: None,
        };
        let mut m = usb.to_match("x".into());
        assert!(usb.matches(&m));
        m.serial = Some("OTHER".into());
        assert!(!usb.matches(&m));
        m.serial = None;
        m.vendor_id = "303A".into();
        assert!(usb.matches(&m));
    }
}
