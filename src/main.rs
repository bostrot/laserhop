mod camera;
mod config;
mod devices;
mod jpeg;
mod laser;
mod mdns;
mod net;
mod setup;
mod web;

use std::{
    io::{IsTerminal, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::signal::unix::{SignalKind, signal};
use tracing::{error, info};

use crate::config::Config;

/// Share a USB laser (GRBL over serial) and its camera over the network.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Config file.
    #[arg(long, global = true, env = "LASERHOP_CONFIG", default_value = config::DEFAULT_PATH)]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the bridge (what the service does).
    Run,
    /// Choose the laser and camera and write the config.
    Setup {
        /// Take the defaults without asking.
        #[arg(long)]
        yes: bool,
    },
    /// List USB serial ports and cameras.
    Devices,
    /// Show the running bridge's status.
    Status,
    /// Show the addresses to use in Rayforge and the browser.
    Info,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Cmd::Info) {
        Cmd::Run => run(&cli.config),
        Cmd::Setup { yes } => setup::run(&cli.config, yes || !std::io::stdin().is_terminal()),
        Cmd::Devices => {
            list_devices();
            Ok(())
        }
        Cmd::Status => status(&cli.config),
        Cmd::Info => info(&cli.config),
    }
}

#[tokio::main]
async fn run(path: &Path) -> Result<()> {
    // systemd's journal adds its own timestamps.
    let under_systemd = std::env::var_os("INVOCATION_ID").is_some();
    let filter = tracing_subscriber::EnvFilter::try_from_env("LASERHOP_LOG")
        .unwrap_or_else(|_| "info".into());
    let fmt = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    if under_systemd {
        fmt.without_time().init();
    } else {
        fmt.init();
    }

    let cfg = Config::load(path).context("no usable config; run `sudo laserhop setup`")?;
    let hostname = net::hostname();
    info!("laserhop {} on {hostname}", env!("CARGO_PKG_VERSION"));

    let laser_status = laser::Shared::default();
    let camera_status = camera::Shared::default();

    let frames = if cfg.camera.enabled {
        Some(camera::spawn(cfg.camera.clone(), camera_status.clone())?)
    } else {
        None
    };

    let mut tasks = tokio::task::JoinSet::new();
    if cfg.laser.enabled {
        tasks.spawn(laser::run(cfg.laser.clone(), laser_status.clone()));
    }
    tasks.spawn(web::run(
        cfg.web.port,
        web::AppState {
            laser: laser_status,
            camera: camera_status,
            frames,
            hostname: Arc::from(hostname.as_str()),
        },
    ));

    let _mdns = if cfg.mdns.enabled {
        mdns::announce(&cfg, &hostname)
            .inspect_err(|e| error!("mdns: {e:#}"))
            .ok()
    } else {
        None
    };

    let mut term = signal(SignalKind::terminate())?;
    tokio::select! {
        Some(res) = tasks.join_next() => {
            // The laser and web tasks only return on a fatal error, such as a port in use.
            res??;
        }
        _ = tokio::signal::ctrl_c() => info!("stopping"),
        _ = term.recv() => info!("stopping"),
    }
    Ok(())
}

fn list_devices() {
    let ports = devices::serial_ports();
    println!("Serial ports:");
    if ports.is_empty() {
        println!("  none (is the laser switched on and plugged in?)");
    }
    for p in &ports {
        println!("  {}  {}  {}", p.path.display(), p.usb.id(), p.usb.label());
    }
    let cams = devices::cameras();
    println!("Cameras:");
    if cams.is_empty() {
        println!("  none");
    }
    for c in &cams {
        let sizes: Vec<String> = c.sizes.iter().map(|(w, h)| format!("{w}x{h}")).collect();
        println!(
            "  {}  {}  {}  [{}]",
            c.path.display(),
            c.usb.id(),
            c.name,
            sizes.join(", ")
        );
    }
}

fn status(path: &Path) -> Result<()> {
    let cfg = Config::load(path)?;
    let mut sock = TcpStream::connect(("127.0.0.1", cfg.web.port))
        .context("the bridge is not running (sudo systemctl status laserhop)")?;
    sock.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(sock, "GET /api/status HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut resp = String::new();
    sock.read_to_string(&mut resp)?;
    let body = resp.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    let v: serde_json::Value = serde_json::from_str(body).context("unexpected reply")?;
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

fn info(path: &Path) -> Result<()> {
    let cfg = Config::load(path).context("no config yet; run `sudo laserhop setup`")?;
    let host = net::primary_ip().map_or_else(net::hostname, |ip| ip.to_string());
    let local = format!("{}.local", net::hostname());
    println!("Status page:  http://{host}:{}/", cfg.web.port);
    if cfg.laser.enabled {
        println!(
            "Laser:        {host} port {} (Rayforge: Machine setup > Configure manually > \
             Device not listed > GRBL (Telnet))",
            cfg.laser.port
        );
    }
    if cfg.camera.enabled {
        println!("Camera:       http://{host}:{}/stream", cfg.web.port);
    }
    if cfg.mdns.enabled {
        println!("Also reachable as {local}");
    }
    println!("Config:       {}", path.display());
    Ok(())
}
