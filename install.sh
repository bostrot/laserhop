#!/bin/sh
# Installs laserhop as a systemd service that starts on boot.
#
#   curl -fsSL https://raw.githubusercontent.com/bostrot/laserhop/main/install.sh | sudo sh
#
# Running it again updates the binary and keeps the config.
# Uninstall: curl -fsSL .../install.sh | sudo sh -s -- --uninstall
#
# Environment:
#   LASERHOP_VERSION  release tag to install (default: latest)
#   LASERHOP_BINARY   install this local file instead of downloading
set -eu

REPO="${LASERHOP_REPO:-bostrot/laserhop}"
VERSION="${LASERHOP_VERSION:-latest}"
BIN=/usr/local/bin/laserhop
UNIT=/etc/systemd/system/laserhop.service
CONFIG=/etc/laserhop/config.toml

say() { printf '%s\n' "$*"; }
die() { printf 'laserhop: %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "please run as root, for example: curl -fsSL <url> | sudo sh"
command -v systemctl >/dev/null 2>&1 || die "systemd is required"

if [ "${1:-}" = "--uninstall" ]; then
    systemctl disable --now laserhop 2>/dev/null || true
    rm -f "$UNIT" "$BIN"
    systemctl daemon-reload
    say "laserhop removed. The config in /etc/laserhop was kept."
    exit 0
fi

case "$(uname -m)" in
    armv6l) arch=armv6 ;;
    armv7l | armv8l) arch=armv7 ;;
    aarch64 | arm64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *) die "unsupported CPU: $(uname -m) (supported: armv6, armv7, aarch64, x86_64)" ;;
esac

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

if [ -n "${LASERHOP_BINARY:-}" ]; then
    cp "$LASERHOP_BINARY" "$tmp"
else
    if [ "$VERSION" = latest ]; then
        url="https://github.com/$REPO/releases/latest/download/laserhop-$arch-linux"
    else
        url="https://github.com/$REPO/releases/download/$VERSION/laserhop-$arch-linux"
    fi
    say "Downloading $url"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$tmp" "$url" || die "download failed"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$tmp" "$url" || die "download failed"
    else
        die "curl or wget is required"
    fi
fi
chmod 755 "$tmp"
"$tmp" --version >/dev/null 2>&1 || die "the binary does not run on this machine"
install -m 755 "$tmp" "$BIN"
say "Installed $("$BIN" --version) to $BIN"

cat >"$UNIT" <<'EOF'
[Unit]
Description=laserhop: USB laser and camera over the network
Wants=network-online.target
After=network-online.target

[Service]
ExecStart=/usr/local/bin/laserhop run
Restart=always
RestartSec=2
DynamicUser=yes
SupplementaryGroups=dialout video
ProtectSystem=strict
ProtectHome=yes
NoNewPrivileges=yes

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload

if [ -f "$CONFIG" ]; then
    say "Keeping $CONFIG. Change it with: sudo laserhop setup"
elif (: </dev/tty) 2>/dev/null; then
    # stdin is the downloaded script when piped from curl, so ask on the terminal.
    "$BIN" setup </dev/tty
else
    say "No terminal, so taking the first laser and camera found. Change it with: sudo laserhop setup"
    "$BIN" setup --yes
fi

systemctl enable laserhop >/dev/null 2>&1
systemctl restart laserhop
sleep 2
if ! systemctl is-active --quiet laserhop; then
    journalctl -u laserhop -n 20 --no-pager >&2 || true
    die "the service did not start (see the log above)"
fi

say ""
say "laserhop is running and starts on boot."
"$BIN" info
