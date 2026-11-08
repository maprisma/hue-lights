#!/usr/bin/env bash
# Build and install Maprisma Hue Lights for the current user (no root needed):
#   ~/.local/bin/maprisma-hue-lights                   the applet
#   ~/.local/share/maprisma-hue-lights/scripts/        the bridge helper
#   ~/.local/share/applications/…desktop             makes it an applet for the panel
#   ~/.local/share/icons/hicolor/scalable/apps/…svg  the bulb icon
# Then add it in COSMIC Settings › Desktop › Panel › Applets.
set -euo pipefail
cd "$(dirname "$0")"
id=io.github.ubruckhaus.CosmicMaprismaHueLights
data="${XDG_DATA_HOME:-$HOME/.local/share}"
bin="$HOME/.local/bin"

cargo build --release
install -Dm755 target/release/maprisma-hue-lights "$bin/maprisma-hue-lights"
install -Dm644 scripts/hue-lights.py "$data/maprisma-hue-lights/scripts/hue-lights.py"
install -Dm644 "data/icons/$id-symbolic.svg" "$data/icons/hicolor/scalable/apps/$id-symbolic.svg"
# The panel starts applets without ~/.local/bin in PATH: the full path.
sed "s|^Exec=.*|Exec=$bin/maprisma-hue-lights|" "data/$id.desktop" > "$data/applications/$id.desktop"
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
echo "Installed. Add “Maprisma Hue Lights” in COSMIC Settings › Desktop › Panel › Applets."
