//! `laserhop setup`: pick the laser and camera in the terminal and write the config.

use std::{
    cell::Cell,
    io::{self, BufRead, Write},
    path::Path,
    process::Command,
    str::FromStr,
};

use anyhow::{Result, bail};

use crate::{
    config::{Config, UsbMatch},
    devices::{self, Camera, SerialPort, UsbInfo},
};

struct Prompt {
    /// Take every default without asking: `--yes`, no terminal, or input ran out.
    auto: Cell<bool>,
}

impl Prompt {
    fn line(&self, question: &str, default: &str) -> Result<String> {
        if self.auto.get() {
            return Ok(default.to_string());
        }
        if default.is_empty() {
            print!("{question}: ");
        } else {
            print!("{question} [{default}]: ");
        }
        io::stdout().flush()?;
        let mut answer = String::new();
        if io::stdin().lock().read_line(&mut answer)? == 0 {
            println!();
            self.auto.set(true);
            return Ok(default.to_string());
        }
        let answer = answer.trim();
        Ok(if answer.is_empty() { default } else { answer }.to_string())
    }

    fn number<T: FromStr + ToString>(&self, question: &str, default: T) -> Result<T> {
        let default = default.to_string();
        loop {
            match self.line(question, &default)?.parse() {
                Ok(v) => return Ok(v),
                Err(_) => println!("  Please enter a number."),
            }
        }
    }

    /// Returns 0 for "none", otherwise the 1-based option number.
    fn choose(&self, options: &[String], default: usize) -> Result<usize> {
        for (i, o) in options.iter().enumerate() {
            println!("  {}) {o}", i + 1);
        }
        println!("  0) none");
        loop {
            match self.line("Select", &default.to_string())?.parse::<usize>() {
                Ok(n) if n <= options.len() => return Ok(n),
                _ => println!("  Please enter a number from 0 to {}.", options.len()),
            }
        }
    }
}

fn describe(path: &Path, usb: &UsbInfo, name: &str) -> String {
    let serial = usb
        .serial
        .as_deref()
        .map(|s| format!(", serial {s}"))
        .unwrap_or_default();
    format!("{name} ({}{serial}) at {}", usb.id(), path.display())
}

/// What the user picked from a device list.
enum Pick {
    None,
    Found(usize),
    /// The configured device, which is not plugged in right now.
    Keep,
}

/// Lists the found devices, plus the configured one if it is missing, so that running setup
/// while the laser is off does not throw its settings away. The default is the configured
/// device, else the first one found.
fn pick_device(
    p: &Prompt,
    mut options: Vec<String>,
    usbs: &[&UsbInfo],
    current: Option<&UsbMatch>,
) -> Result<Pick> {
    let found = usbs.len();
    let current_idx = current.and_then(|m| usbs.iter().position(|u| u.matches(m)));
    let keep = current.filter(|_| current_idx.is_none());
    if let Some(m) = keep {
        let name = m
            .name
            .clone()
            .unwrap_or_else(|| format!("{}:{}", m.vendor_id, m.product_id));
        options.push(format!("{name} (current setting, not connected right now)"));
    }
    let default = match (current_idx, keep) {
        (Some(i), _) => i + 1,
        (None, Some(_)) => found + 1,
        (None, None) if found > 0 => 1,
        _ => 0,
    };
    Ok(match p.choose(&options, default)? {
        0 => Pick::None,
        n if n <= found => Pick::Found(n - 1),
        _ => Pick::Keep,
    })
}

pub fn run(path: &Path, yes: bool) -> Result<()> {
    let p = Prompt {
        auto: Cell::new(yes),
    };
    let mut cfg = if path.exists() {
        Config::load(path)?
    } else {
        Config::default()
    };
    let first_time = !path.exists();

    println!("laserhop setup. Press Enter to keep the value in brackets.\n");

    let (ports, cameras) = loop {
        let ports = devices::serial_ports();
        let cameras = devices::cameras();
        if !ports.is_empty() || !cameras.is_empty() || p.auto.get() {
            break (ports, cameras);
        }
        println!("No USB serial ports or cameras found. Is the laser switched on and plugged in?");
        if p.line("Press Enter to scan again, or type s to skip", "")? == "s" {
            break (ports, cameras);
        }
    };

    choose_laser(&p, &mut cfg, &ports)?;
    choose_camera(&p, &mut cfg, &cameras)?;

    println!("\nStatus page and camera stream");
    cfg.web.port = p.number("Web port", cfg.web.port)?;

    if cfg.laser.enabled && cfg.camera.enabled && cfg.laser.port == cfg.web.port {
        bail!(
            "the laser and the web page cannot share port {}",
            cfg.web.port
        );
    }

    cfg.save(path)?;
    println!("\nSaved {}", path.display());

    // Only the default config belongs to the service.
    if path != Path::new(crate::config::DEFAULT_PATH) {
        return Ok(());
    }
    let active = Command::new("systemctl")
        .args(["is-active", "--quiet", "laserhop"])
        .status()
        .is_ok_and(|s| s.success());
    if active {
        Command::new("systemctl")
            .args(["restart", "laserhop"])
            .status()?;
        println!("Restarted the laserhop service.");
    } else if !first_time {
        println!("Start it with: sudo systemctl start laserhop");
    }
    Ok(())
}

fn choose_laser(p: &Prompt, cfg: &mut Config, ports: &[SerialPort]) -> Result<()> {
    println!("Laser (USB serial port to share over TCP)");
    let options: Vec<String> = ports
        .iter()
        .map(|s| describe(&s.path, &s.usb, &s.usb.label()))
        .collect();
    let usbs: Vec<&UsbInfo> = ports.iter().map(|s| &s.usb).collect();
    match pick_device(p, options, &usbs, cfg.laser.device.as_ref())? {
        Pick::None => {
            cfg.laser.enabled = false;
            return Ok(());
        }
        Pick::Found(i) => cfg.laser.device = Some(ports[i].usb.to_match(ports[i].usb.label())),
        Pick::Keep => {}
    }
    cfg.laser.enabled = true;
    cfg.laser.port = p.number("TCP port", cfg.laser.port)?;
    cfg.laser.baud = p.number("Baud rate", cfg.laser.baud)?;
    Ok(())
}

fn choose_camera(p: &Prompt, cfg: &mut Config, cameras: &[Camera]) -> Result<()> {
    println!("\nCamera (MJPEG stream over HTTP)");
    let options: Vec<String> = cameras
        .iter()
        .map(|c| describe(&c.path, &c.usb, &c.name))
        .collect();
    let usbs: Vec<&UsbInfo> = cameras.iter().map(|c| &c.usb).collect();
    let cam = match pick_device(p, options, &usbs, cfg.camera.device.as_ref())? {
        Pick::None => {
            cfg.camera.enabled = false;
            return Ok(());
        }
        Pick::Found(i) => Some(&cameras[i]),
        Pick::Keep => None,
    };
    cfg.camera.enabled = true;

    // Resolutions can only be listed while the camera is plugged in.
    if let Some(cam) = cam.filter(|c| !c.sizes.is_empty()) {
        cfg.camera.device = Some(cam.usb.to_match(cam.name.clone()));
        println!("Resolution (higher needs more Wi-Fi bandwidth)");
        let current = (cfg.camera.width, cfg.camera.height);
        let default = cam
            .sizes
            .iter()
            .position(|&s| s == current)
            .or_else(|| cam.sizes.iter().position(|&(w, _)| w <= 1280))
            .unwrap_or(0);
        for (i, (w, h)) in cam.sizes.iter().enumerate() {
            println!("  {}) {w}x{h}", i + 1);
        }
        let pick = loop {
            match p
                .line("Select", &(default + 1).to_string())?
                .parse::<usize>()
            {
                Ok(n) if (1..=cam.sizes.len()).contains(&n) => break n - 1,
                _ => println!("  Please enter a number from 1 to {}.", cam.sizes.len()),
            }
        };
        (cfg.camera.width, cfg.camera.height) = cam.sizes[pick];
    } else if let Some(cam) = cam {
        cfg.camera.device = Some(cam.usb.to_match(cam.name.clone()));
    }
    cfg.camera.fps = p.number("Frames per second", cfg.camera.fps)?.clamp(1, 60);
    Ok(())
}
