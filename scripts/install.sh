#!/usr/bin/env bash
# Build Penumbra and install it for the current user (no root needed).
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release
install -Dm755 target/release/penumbra "$HOME/.local/bin/penumbra"
"$HOME/.local/bin/penumbra" install-launcher
echo
echo "Installed ~/.local/bin/penumbra"
echo "Start it:            penumbra            (it lives in the tray)"
echo "Start with session:  penumbra autostart on"
command -v ddcutil >/dev/null || echo "NOTE: ddcutil is missing; install it for monitor control:  sudo pacman -S ddcutil"
groups | grep -qw i2c || echo "NOTE: if monitors show no controls, allow I2C access:  sudo usermod -aG i2c \$USER  (then log in again)"
