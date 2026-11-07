#!/usr/bin/env bash
# Installs Logitech Mouse DPI for the current user:
#   app      -> ~/.local/bin/logitech-dpi
#   menu     -> ~/.local/share/applications/logitech-dpi.desktop
#   icon     -> ~/.local/share/icons/hicolor/scalable/apps/logitech-dpi.svg
#   udev     -> /etc/udev/rules.d/70-logitech-hidpp.rules (asks for your password)
#
# Works from a git checkout (builds with cargo), from an extracted release,
# or piped from curl (downloads the latest release).
#   ./install.sh              install or update
#   ./install.sh --uninstall  remove everything again
set -euo pipefail

REPO="S0CloseYetS0Far/Logitech-Mouse-DPI"
TARBALL="logitech-mouse-dpi-x86_64-linux.tar.gz"
APP="logitech-dpi"
BIN_DIR="$HOME/.local/bin"
APPS_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
RULE="/etc/udev/rules.d/70-logitech-hidpp.rules"
OLD_RULES="/etc/udev/rules.d/99-logitech-g402.rules /etc/udev/rules.d/99-logitech-hidpp.rules"

# Run a command as root, from a terminal (sudo) or a desktop (pkexec).
as_root() {
    if [ "$(id -u)" -eq 0 ]; then "$@"
    elif [ -t 0 ] && command -v sudo >/dev/null; then sudo "$@"
    else pkexec "$@"
    fi
}

RELOAD_UDEV="udevadm control --reload-rules && udevadm trigger --subsystem-match=hidraw && udevadm settle"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN_DIR/$APP" "$APPS_DIR/$APP.desktop" "$ICON_DIR/$APP.svg"
    echo "Removing the udev rule (needs your password)…"
    as_root sh -c "rm -f $RULE $OLD_RULES && $RELOAD_UDEV"
    echo "Logitech Mouse DPI removed."
    exit 0
fi

# Find the files: next to this script, or download the latest release.
src=""
if [ -f "${BASH_SOURCE[0]:-}" ]; then
    src="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fi
if [ -n "$src" ] && [ -f "$src/Cargo.toml" ]; then
    if ! command -v cargo >/dev/null; then
        [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    fi
    if command -v cargo >/dev/null; then
        echo "Building (release)…"
        cargo build --release --manifest-path "$src/Cargo.toml"
        bin="$src/target/release/$APP"
    else
        echo "Rust is not installed; downloading the prebuilt release instead."
        src=""
    fi
elif [ -n "$src" ] && [ -x "$src/$APP" ]; then
    bin="$src/$APP"
else
    src=""
fi
if [ -z "$src" ]; then
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    echo "Downloading the latest release…"
    curl -fsSL "https://github.com/$REPO/releases/latest/download/$TARBALL" | tar xz -C "$tmp"
    src="$tmp"
    bin="$src/$APP"
fi

mkdir -p "$BIN_DIR" "$APPS_DIR" "$ICON_DIR"
install -m 755 "$bin" "$BIN_DIR/$APP"
install -m 644 "$src/packaging/$APP.svg" "$ICON_DIR/$APP.svg"
cat > "$APPS_DIR/$APP.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Logitech Mouse DPI
Comment=Change the DPI of your Logitech mouse
Exec=$BIN_DIR/$APP
Icon=$APP
Terminal=false
Categories=Settings;HardwareSettings;
Keywords=mouse;dpi;logitech;sensitivity;
StartupWMClass=$APP
EOF
command -v update-desktop-database >/dev/null && update-desktop-database "$APPS_DIR" 2>/dev/null || true

if ! cmp -s "$src/packaging/70-logitech-hidpp.rules" "$RULE"; then
    echo "Installing the udev rule so the app works without sudo (needs your password)…"
    as_root sh -c "rm -f $OLD_RULES && install -m 644 '$src/packaging/70-logitech-hidpp.rules' '$RULE' && $RELOAD_UDEV"
fi

echo
echo "Done. Open \"Logitech Mouse DPI\" from your app menu."
