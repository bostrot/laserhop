# laserhop

Puts a USB-only laser engraver on the network, using the oldest, slowest Linux board you have lying around.

Many diode lasers, such as the Creality Falcon A1, connect over USB only. laserhop runs on a board next to the laser and makes the laser and its camera available over Wi-Fi or Ethernet, so the laptop running [Rayforge](https://rayforge.org/) no longer has to sit next to the machine.

```
 ┌──────────┐  USB   ┌─────────────────┐  Wi-Fi / LAN  ┌────────────────────┐
 │  Laser   │◄──────►│    laserhop     │◄─────────────►│ Rayforge, browser  │
 │ + camera │        │ (old board)     │               │                    │
 └──────────┘        └─────────────────┘               └────────────────────┘
                       TCP 2000   serial port (GRBL)
                       HTTP 8080  camera, status page
```

## Built for old hardware

Sharing a serial port and a camera is a small job, so it shouldn't need new hardware. laserhop is written to run on the boards that end up in a drawer:

- **One static binary of about 2 MB.** No Python, no Node, no runtime dependencies, nothing else to install.
- **Installs onto the Linux you already run,** such as Armbian or Raspberry Pi OS, with one command. No image to flash, no disk to give up.
- **ARMv6, ARMv7, AArch64 and x86_64.** That covers a first-generation Raspberry Pi or Pi Zero, an Orange Pi Zero, and any old PC.
- **No video encoding.** The camera already delivers JPEG frames. laserhop passes them through and only drops frames to keep to the configured frame rate.
- **Slow Wi-Fi is fine.** Every viewer gets the newest frame, and a slow viewer skips frames instead of building up a delay.

## Compared with other setups

| | laserhop | ser2net + ustreamer | [LaserBridgeOS](https://github.com/hkiam/LaserBridgeOS) |
|---|---|---|---|
| Runs on ARM boards (Pi, Orange Pi) | yes, ARMv6 and newer | yes | no, x86_64 PCs only |
| Install | one command on your existing Linux | two packages, configured by hand | flash an 800 MB disk image |
| Camera stream | yes | yes | yes |
| Falcon A1 `error:2` on connect | handled | first command fails | not mentioned in its docs |
| Finds the laser after replugging or reboot | by USB ID | by device path | by `/dev/serial/by-id` |
| Safety watchdog for a sender that disappears | no | no | yes |

LaserBridgeOS is the better choice if you have a spare PC and want its safety features. laserhop is for small boards and for keeping the system you already have.

## Features

- **Serial port over TCP** for Rayforge's *GRBL (Telnet)* driver or any other GRBL sender that speaks raw TCP
- **Camera as an MJPEG stream** that Rayforge and browsers can use directly
- **Status page** with machine state, connected client and camera preview
- **Survives unplugging and reboots:** devices are remembered by USB ID, and it waits for the laser if it's off
- **Handles controller quirks,** such as the Falcon A1's `error:2` on connect
- **Found by name** via mDNS as `<hostname>.local`
- **Starts on boot** as a systemd service

## Requirements

- A Linux board with systemd on **armv6**, **armv7**, **aarch64** or **x86_64**
- A GRBL laser that shows up as a USB serial port
- Optionally, a USB camera that delivers MJPEG. Nearly all do.

Tested with a Creality Falcon A1 on an Orange Pi Zero running Armbian.

## Install

On the board next to the laser, with the laser switched on and plugged in:

```sh
curl -fsSL https://raw.githubusercontent.com/bostrot/laserhop/main/install.sh | sudo sh
```

The installer downloads the binary and asks which serial port and camera to share. It then sets up a systemd service that starts on boot, and prints the addresses to use:

```
Status page:  http://192.168.3.23:8080/
Laser:        192.168.3.23 port 2000
Camera:       http://192.168.3.23:8080/stream
```

To update, run the same command again. Your settings are kept.

## Set up Rayforge

- **Laser:** *Machine setup → Configure manually → Device not listed → GRBL (Telnet)*, host = the board's IP address, port `2000`
- **Camera:** add a camera with `http://<board-ip>:8080/stream` as the source

## Usage

| Command | What it does |
|---|---|
| `sudo laserhop setup` | Choose the laser and camera again. This restarts the service. |
| `laserhop devices` | List USB serial ports and cameras. |
| `laserhop status` | Show the running service's state. |
| `laserhop info` | Print the addresses for Rayforge and the browser. |

The settings live in `/etc/laserhop/config.toml`. Logs: `journalctl -u laserhop -f`.

## Security

laserhop has **no authentication**. Anyone who can reach the board can control the laser and watch the camera. Only run it on a network you trust, and never expose it to the internet.

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/bostrot/laserhop/main/install.sh | sudo sh -s -- --uninstall
```

## More

[docs/details.md](docs/details.md) covers the config file, HTTP endpoints, troubleshooting, how it works inside, and building from source.

## Licence

[MIT](LICENSE)
