# laserhop in detail

- [Config file](#config-file)
- [Commands and service](#commands-and-service)
- [HTTP endpoints](#http-endpoints)
- [Troubleshooting](#troubleshooting)
- [How it works](#how-it-works)
- [Build from source](#build-from-source)

## Config file

`sudo laserhop setup` writes `/etc/laserhop/config.toml`. It shows the current values as defaults. If the laser is off while you run it, the saved device is offered as *current setting, not connected right now*, so nothing is lost.

You can also edit the file by hand and run `sudo systemctl restart laserhop`.

```toml
[laser]
enabled = true
port = 2000          # TCP port clients connect to
baud = 115200        # serial speed; GRBL boards almost always use 115200
settle_ms = 1500     # how long to wait for the controller's banner after opening the port
wake_line = true     # send one empty line after opening and drop the reply (Falcon A1 error:2 fix)
poll_status = true   # send GRBL "?" every 2 s while no client is connected, for the status page

[laser.device]       # which USB device is the laser
vendor_id = "303a"
product_id = "4001"
serial = "..."       # optional; set when two identical devices are connected
name = "Creality Falcon A1"   # only a label

[camera]
enabled = true
width = 1280
height = 960
fps = 10             # frames per second sent to clients

[camera.device]
vendor_id = "1bcf"
product_id = "0b19"
name = "Creality Falcon Camera"

[web]
port = 8080          # status page, /stream, /snapshot, /api/status

[mdns]
enabled = true       # announce <hostname>.local and the service
# name = "Workshop laser"   # service name shown to browsers; defaults to the hostname
```

`laserhop devices` shows the vendor and product ids of what's connected.

## Commands and service

| Command | What it does |
|---|---|
| `sudo laserhop setup` | Choose devices and save the config. `--yes` takes the defaults without asking. |
| `laserhop devices` | List USB serial ports and MJPEG cameras with their ids and resolutions. |
| `laserhop status` | Show the running service's state as JSON. |
| `laserhop info` | Print the addresses. This is the default when no command is given. |
| `laserhop run` | Run the bridge in the foreground. The service does this. |

Every command takes `--config <path>` (or `LASERHOP_CONFIG`). For detailed logs, set `LASERHOP_LOG=debug`.

```sh
sudo systemctl status laserhop
sudo systemctl restart laserhop
journalctl -u laserhop -f
```

Installer options:

- `LASERHOP_VERSION=v0.1.0` installs a specific release.
- `LASERHOP_BINARY=./laserhop` installs a local file instead of downloading one.

The service runs as an unprivileged, temporary user (`DynamicUser`). That user is only in the `dialout` and `video` groups, and the file system is read-only for it.

## HTTP endpoints

| Path | Content |
|---|---|
| `/` | Status page |
| `/stream` | MJPEG stream (`multipart/x-mixed-replace`) |
| `/snapshot` | Latest camera frame as a JPEG |
| `/api/status` | Laser and camera state as JSON |

`/stream` and `/snapshot` use the same paths as [ustreamer](https://github.com/pikvm/ustreamer), so clients set up for ustreamer work unchanged.

```json
{
  "hostname": "orangepizero",
  "version": "0.1.0",
  "laser": {
    "enabled": true, "connected": true, "port": 2000,
    "device": "/dev/ttyACM0", "name": "Creality Falcon A1",
    "firmware": "Grbl 1.1f", "state": "Idle",
    "position": "MPos:0.000,0.000,0.000",
    "client": "192.168.3.126", "last_error": null
  },
  "camera": {
    "enabled": true, "connected": true, "device": "/dev/video0",
    "name": "Creality Falcon Camera", "width": 1280, "height": 960,
    "fps": 10.0, "viewers": 1, "last_error": null
  }
}
```

## Troubleshooting

**The laser shows "Not connected".**
Run `laserhop devices`. If the serial port isn't listed, the board can't see the laser at all. Check that:

- the laser is switched on
- the cable is in the board's USB host port, not a power-only port
- the cable carries data, since some USB cables only charge

A weak power supply can also stop the board from powering its USB port. Use at least 5 V / 2 A for an Orange Pi or Raspberry Pi. `dmesg -w` shows whether the board notices the device being plugged in.

**Rayforge connects but the laser doesn't respond, or reports *Invalid Number Format*.**
Leave `wake_line = true`. If your controller needs more time after the port opens, raise `settle_ms`. If it stops answering completely, switch the laser off and on. Restarting laserhop does not reset a hung controller.

**`cannot listen on port 2000` in the log.**
Another program is using the port, for example an older ser2net setup. Stop it, or choose another port with `sudo laserhop setup`. `sudo ss -ltnp` shows which program holds which port.

**The camera stream is slow or stutters.**
The bottleneck is almost always Wi-Fi. Lower the resolution or `fps`, move the board closer to the router, or use Ethernet. The status page shows the frame rate actually delivered.

**`<hostname>.local` doesn't resolve.**
mDNS only works within one network segment. If your computer and the board are on different subnets, use the IP address.

**The connection drops in the middle of a job.**
That is the network, not the laser. A job only continues as long as the sender can stream G-code, so use Ethernet or a strong Wi-Fi signal for long jobs.

## How it works

**Serial bridge.** laserhop opens the serial port once and keeps it open. Many controllers misbehave every time the port is opened. Some reset, and the Falcon A1 prints a banner and answers the next line with `error:2`. A sender that opens the port itself sees that error on its first command, which in Rayforge is the homing command, so nothing moves. With the port held open, it only happens once. laserhop sends one empty line itself and drops the reply. It also doesn't drop DTR when the port closes, because that resets many boards.

One client is connected at a time. A new connection replaces the old one, so a client that crashed or lost Wi-Fi can't lock you out. While no client is connected, laserhop sends GRBL's realtime `?` every two seconds for the status page. While a client is connected, it only reads the replies passing through and never sends anything of its own. While the laser is missing, connections are refused, so senders show the laser as disconnected instead of hanging.

**Camera.** UVC cameras compress to JPEG in hardware. laserhop takes the frames as they come, thins them to the configured frame rate, and serves the newest one to every viewer. A slow viewer skips frames instead of falling behind. Many cameras leave the Huffman tables out of their frames. laserhop adds the standard ones, so browsers and OpenCV can decode every frame. A camera that stops delivering frames for 5 seconds is reopened.

**Device lookup.** Devices are found through sysfs by USB vendor id, product id and, if set, serial number. Moving the laser to another USB port, or the kernel numbering it differently after a reboot, doesn't matter.

## Build from source

laserhop cross-compiles to static musl binaries with [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild), on macOS or Linux:

```sh
cargo install cargo-zigbuild        # also needs zig, e.g. `brew install zig`
rustup target add arm-unknown-linux-musleabihf armv7-unknown-linux-musleabihf aarch64-unknown-linux-musl x86_64-unknown-linux-musl
cargo zigbuild --release --target armv7-unknown-linux-musleabihf
```

| Board | Target |
|---|---|
| Raspberry Pi 1, Pi Zero, Pi Zero W | `arm-unknown-linux-musleabihf` (ARMv6) |
| Raspberry Pi 2, Orange Pi Zero and other 32-bit boards | `armv7-unknown-linux-musleabihf` |
| 64-bit ARM boards running a 64-bit OS | `aarch64-unknown-linux-musl` |
| PCs | `x86_64-unknown-linux-musl` |

Install a local build with the same installer:

```sh
scp target/armv7-unknown-linux-musleabihf/release/laserhop install.sh root@<board>:/tmp/
ssh root@<board> 'cd /tmp && LASERHOP_BINARY=/tmp/laserhop sh install.sh'
```

GitHub Actions builds the release binaries when a `v*` tag is pushed (`.github/workflows/release.yml`).

| Source | Responsibility |
|---|---|
| `src/laser.rs` | Serial-to-TCP bridge and GRBL status parsing |
| `src/camera.rs` | V4L2 capture thread |
| `src/jpeg.rs` | Adds missing Huffman tables |
| `src/web.rs` | Status page, stream, snapshot, JSON |
| `src/devices.rs` | USB device discovery through sysfs |
| `src/setup.rs` | Interactive setup |
| `src/mdns.rs` | mDNS announcement |
| `install.sh` | Installer and systemd unit |
