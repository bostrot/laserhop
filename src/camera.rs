//! Captures MJPEG frames from the camera on a thread and hands out the latest one.
//!
//! The camera already compresses to JPEG, so frames are passed through as they are.

use std::{
    os::fd::AsRawFd,
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use linuxvideo::{
    Device, Fract,
    format::{PixFormat, PixelFormat},
};
use serde::Serialize;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::{config::CameraConfig, devices, jpeg};

#[derive(Debug, Clone, Default, Serialize)]
pub struct CameraStatus {
    pub enabled: bool,
    pub connected: bool,
    pub device: Option<String>,
    pub name: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Frames per second actually delivered, over the last second.
    pub fps: f32,
    pub viewers: usize,
    pub last_error: Option<String>,
}

pub type Shared = Arc<Mutex<CameraStatus>>;
pub type Frames = watch::Receiver<Option<Bytes>>;

pub fn spawn(cfg: CameraConfig, status: Shared) -> Result<Frames> {
    let want = cfg
        .device
        .clone()
        .context("no camera selected; run `sudo laserhop setup`")?;
    let (tx, rx) = watch::channel(None);
    {
        let mut s = status.lock().unwrap();
        s.enabled = true;
        s.name = want.name.clone();
    }
    thread::Builder::new()
        .name("camera".into())
        .spawn(move || {
            let mut last_error = String::new();
            loop {
                let err = match devices::find_camera(&want) {
                    None => format!("waiting for camera {}:{}", want.vendor_id, want.product_id),
                    Some(path) => match capture(&path, &cfg, &tx, &status) {
                        Ok(()) => continue,
                        Err(e) => format!("{e:#}"),
                    },
                };
                tx.send_replace(None);
                {
                    let mut s = status.lock().unwrap();
                    s.connected = false;
                    s.fps = 0.0;
                    s.last_error = Some(err.clone());
                }
                if err != last_error {
                    if err.starts_with("waiting") {
                        info!("camera: {err}");
                    } else {
                        warn!("camera: {err}");
                    }
                    last_error = err;
                }
                thread::sleep(Duration::from_secs(2));
            }
        })?;
    Ok(rx)
}

fn capture(
    path: &Path,
    cfg: &CameraConfig,
    tx: &watch::Sender<Option<Bytes>>,
    status: &Shared,
) -> Result<()> {
    let device = Device::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let capture = device
        .video_capture(PixFormat::new(cfg.width, cfg.height, PixelFormat::MJPG))
        .context("cannot set the video format")?;
    let format = capture.format();
    if format.pixel_format() != PixelFormat::MJPG {
        bail!("{} does not deliver MJPEG", path.display());
    }
    let (width, height) = (format.width(), format.height());
    let fps = cfg.fps.max(1);
    // Not every camera accepts every rate; frames are thinned out below anyway.
    capture.set_frame_interval(Fract::new(1, fps)).ok();
    let mut stream = capture.into_stream().context("cannot start streaming")?;

    info!(
        "camera: streaming {} at {width}x{height}, {fps} fps",
        path.display()
    );
    {
        let mut s = status.lock().unwrap();
        s.connected = true;
        s.device = Some(path.display().to_string());
        s.width = width;
        s.height = height;
        s.last_error = None;
    }

    let min_gap = Duration::from_secs_f64(0.9 / fps as f64);
    let mut last_sent = Instant::now() - min_gap;
    let mut window = (Instant::now(), 0u32);
    loop {
        // A camera that stops sending frames without an error gets reopened.
        let mut pfd = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        match unsafe { libc::poll(&mut pfd, 1, 5000) } {
            0 => bail!("no frame for 5 s"),
            n if n < 0 => {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err.into());
            }
            _ => {}
        }

        let due = last_sent.elapsed() >= min_gap;
        let frame = stream
            .dequeue(|buf| Ok((due && !buf.is_error() && !buf.is_empty()).then(|| buf.to_vec())))
            .context("camera disconnected")?;
        let Some(frame) = frame else { continue };
        last_sent = Instant::now();
        tx.send_replace(Some(Bytes::from(jpeg::ensure_huffman(frame))));

        window.1 += 1;
        let elapsed = window.0.elapsed();
        if elapsed >= Duration::from_secs(1) {
            status.lock().unwrap().fps = window.1 as f32 / elapsed.as_secs_f32();
            window = (Instant::now(), 0);
        }
    }
}
