//! Shares the laser's serial port over TCP.
//!
//! The port stays open the whole time, so a controller that resets or complains when the port
//! is opened (the Falcon A1 answers the first line with error:2) only does that once, at startup,
//! and never in front of a client. One client at a time; a new client replaces the old one.

use std::{
    fs::File,
    io::{self, Read, Write},
    net::SocketAddr,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use nix::sys::termios::{self, BaudRate, ControlFlags, SetArg};
use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, unix::AsyncFd},
    net::{TcpListener, TcpStream},
    time::{self, Instant},
};
use tracing::{info, warn};

use crate::{config::LaserConfig, devices, net};

#[derive(Debug, Clone, Default, Serialize)]
pub struct LaserStatus {
    pub enabled: bool,
    pub port: u16,
    pub connected: bool,
    pub device: Option<String>,
    pub name: Option<String>,
    pub firmware: Option<String>,
    /// GRBL machine state, such as Idle, Run, Hold or Alarm.
    pub state: Option<String>,
    pub position: Option<String>,
    pub client: Option<String>,
    pub last_error: Option<String>,
}

pub type Shared = Arc<Mutex<LaserStatus>>;

fn update(status: &Shared, f: impl FnOnce(&mut LaserStatus)) {
    f(&mut status.lock().unwrap());
}

struct Serial {
    fd: AsyncFd<File>,
}

impl Serial {
    fn open(path: &Path, baud: u32) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
            .open(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        let mut t = termios::tcgetattr(&file)?;
        termios::cfmakeraw(&mut t);
        termios::cfsetspeed(&mut t, baud_rate(baud)?)?;
        t.control_flags |= ControlFlags::CLOCAL | ControlFlags::CREAD;
        // Don't drop DTR on close: on many boards that resets the controller.
        t.control_flags &= !ControlFlags::HUPCL;
        termios::tcsetattr(&file, SetArg::TCSANOW, &t)?;
        // SAFETY: the File owns its descriptor, and nothing else closes or replaces it.
        let fd = unsafe { AsyncFd::register(file)? };
        Ok(Self { fd })
    }

    async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.readable().await?;
            if let Ok(res) = guard.try_io(|f| (&mut f.get_ref()).read(buf)) {
                return match res {
                    // A tty that reads 0 bytes while readable has hung up (unplugged).
                    Ok(0) => Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "device closed",
                    )),
                    other => other,
                };
            }
        }
    }

    async fn write_all(&self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let mut guard = self.fd.writable().await?;
            if let Ok(res) = guard.try_io(|f| (&mut f.get_ref()).write(data)) {
                data = &data[res?..];
            }
        }
        Ok(())
    }

    /// Reads and discards everything for `dur`, feeding it to the parser.
    async fn drain(
        &self,
        dur: Duration,
        parser: &mut LineParser,
        status: &Shared,
    ) -> io::Result<()> {
        let end = Instant::now() + dur;
        let mut buf = [0u8; 1024];
        while let Ok(res) = time::timeout_at(end, self.read(&mut buf)).await {
            parser.feed(&buf[..res?], status);
        }
        Ok(())
    }
}

fn baud_rate(baud: u32) -> Result<BaudRate> {
    Ok(match baud {
        9600 => BaudRate::B9600,
        19200 => BaudRate::B19200,
        38400 => BaudRate::B38400,
        57600 => BaudRate::B57600,
        115200 => BaudRate::B115200,
        230400 => BaudRate::B230400,
        460800 => BaudRate::B460800,
        500000 => BaudRate::B500000,
        921600 => BaudRate::B921600,
        1000000 => BaudRate::B1000000,
        other => bail!("unsupported baud rate {other}"),
    })
}

/// Picks GRBL's replies out of the byte stream for the status page.
#[derive(Default)]
struct LineParser {
    line: Vec<u8>,
}

impl LineParser {
    fn feed(&mut self, data: &[u8], status: &Shared) {
        for &b in data {
            if b == b'\n' {
                let line = String::from_utf8_lossy(&self.line).trim().to_string();
                self.line.clear();
                if !line.is_empty() {
                    update(status, |s| apply_line(s, &line));
                }
            } else if self.line.len() < 512 {
                self.line.push(b);
            }
        }
    }
}

fn apply_line(s: &mut LaserStatus, line: &str) {
    if let Some(report) = line.strip_prefix('<').and_then(|l| l.strip_suffix('>')) {
        let mut fields = report.split('|');
        if let Some(state) = fields.next() {
            s.state = Some(state.split(':').next().unwrap_or(state).to_string());
        }
        if let Some(pos) = fields.find(|f| f.starts_with("MPos:") || f.starts_with("WPos:")) {
            s.position = Some(pos.to_string());
        }
    } else if line.starts_with("Grbl ") || line.starts_with("GrblHAL ") {
        s.firmware = Some(
            line.split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" "),
        );
    } else if line.starts_with("ALARM:") {
        s.state = Some("Alarm".into());
        s.last_error = Some(line.to_string());
    } else if line.starts_with("error:") {
        s.last_error = Some(line.to_string());
    }
}

pub async fn run(cfg: LaserConfig, status: Shared) -> Result<()> {
    let Some(want) = cfg.device.clone() else {
        bail!("no laser selected; run `sudo laserhop setup`");
    };
    let listener = net::bind_tcp(cfg.port).await?;
    info!("laser: listening on TCP port {}", cfg.port);
    update(&status, |s| {
        s.enabled = true;
        s.port = cfg.port;
        s.name = want.name.clone();
    });

    let mut parser = LineParser::default();
    let mut missing_logged = false;
    loop {
        let Some(path) = devices::find_serial(&want) else {
            if !missing_logged {
                info!(
                    "laser: waiting for {}:{} to be plugged in and switched on",
                    want.vendor_id, want.product_id
                );
                missing_logged = true;
            }
            // Refuse clients while there is no laser, so they show as disconnected.
            tokio::select! {
                Ok((sock, addr)) = listener.accept() => {
                    info!("laser: refused {addr}, laser not connected");
                    drop(sock);
                }
                _ = time::sleep(Duration::from_secs(2)) => {}
            }
            continue;
        };
        missing_logged = false;

        let serial = match Serial::open(&path, cfg.baud) {
            Ok(s) => s,
            Err(e) => {
                warn!("laser: {e:#}");
                update(&status, |s| s.last_error = Some(format!("{e:#}")));
                time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        info!("laser: opened {}", path.display());

        let result = async {
            serial
                .drain(Duration::from_millis(cfg.settle_ms), &mut parser, &status)
                .await?;
            if cfg.wake_line {
                serial.write_all(b"\n").await?;
                serial
                    .drain(Duration::from_secs(1), &mut parser, &status)
                    .await?;
            }
            update(&status, |s| {
                s.connected = true;
                s.device = Some(path.display().to_string());
                s.last_error = None;
            });
            session(&listener, &serial, &cfg, &mut parser, &status).await
        }
        .await;

        if let Err(e) = result {
            warn!("laser: connection lost ({e})");
        }
        update(&status, |s| {
            s.connected = false;
            s.client = None;
            s.state = None;
            s.position = None;
        });
        time::sleep(Duration::from_secs(1)).await;
    }
}

async fn session(
    listener: &TcpListener,
    serial: &Serial,
    cfg: &LaserConfig,
    parser: &mut LineParser,
    status: &Shared,
) -> io::Result<()> {
    let mut client: Option<(TcpStream, SocketAddr)> = None;
    let mut from_serial = [0u8; 4096];
    let mut from_client = [0u8; 4096];
    let mut poll = time::interval(Duration::from_secs(2));

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((sock, addr)) = accepted else { continue };
                sock.set_nodelay(true).ok();
                if let Some((_, old)) = &client {
                    info!("laser: {addr} replaces {old}");
                }
                client = Some((sock, addr));
                update(status, |s| s.client = Some(addr.ip().to_canonical().to_string()));
                // Drop replies to our own status polls that nobody asked for.
                serial.drain(Duration::from_millis(50), parser, status).await?;
                info!("laser: client {addr} connected");
            }
            n = serial.read(&mut from_serial) => {
                let data = &from_serial[..n?];
                parser.feed(data, status);
                if let Some((sock, addr)) = client.as_mut() {
                    let sent = time::timeout(Duration::from_secs(5), sock.write_all(data)).await;
                    if !matches!(sent, Ok(Ok(()))) {
                        info!("laser: client {addr} stopped reading, dropped");
                        client = None;
                        update(status, |s| s.client = None);
                    }
                }
            }
            n = async { client.as_mut().unwrap().0.read(&mut from_client).await }, if client.is_some() => {
                match n {
                    Ok(n) if n > 0 => serial.write_all(&from_client[..n]).await?,
                    _ => {
                        if let Some((_, addr)) = client.take() {
                            info!("laser: client {addr} disconnected");
                        }
                        update(status, |s| s.client = None);
                    }
                }
            }
            _ = poll.tick(), if client.is_none() && cfg.poll_status => {
                // '?' is a GRBL realtime command: it needs no newline and gets no "ok".
                serial.write_all(b"?").await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(lines: &str) -> LaserStatus {
        let status: Shared = Default::default();
        LineParser::default().feed(lines.as_bytes(), &status);
        status.lock().unwrap().clone()
    }

    #[test]
    fn reads_firmware_and_state() {
        let s = parse("Grbl 1.1f ['$' for help]\r\n<Idle|MPos:0.000,0.000,0.000|FS:0,0>\r\n");
        assert_eq!(s.firmware.as_deref(), Some("Grbl 1.1f"));
        assert_eq!(s.state.as_deref(), Some("Idle"));
        assert_eq!(s.position.as_deref(), Some("MPos:0.000,0.000,0.000"));
    }

    #[test]
    fn strips_substates_and_tracks_errors() {
        let s = parse("<Hold:0|WPos:1,2,3>\nerror:2\n");
        assert_eq!(s.state.as_deref(), Some("Hold"));
        assert_eq!(s.last_error.as_deref(), Some("error:2"));
        assert_eq!(parse("ALARM:1\n").state.as_deref(), Some("Alarm"));
    }

    #[test]
    fn handles_lines_split_across_reads() {
        let status: Shared = Default::default();
        let mut p = LineParser::default();
        p.feed(b"<Ru", &status);
        p.feed(b"n|MPos:1,1,0>\n", &status);
        assert_eq!(status.lock().unwrap().state.as_deref(), Some("Run"));
    }
}
