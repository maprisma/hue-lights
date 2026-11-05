#!/usr/bin/env bash
# Build and install Hue Lights for the current user (no root needed):
#   ~/.local/bin/cosmic-hue-lights                   the applet
#   ~/.local/share/cosmic-hue-lights/scripts/        the bridge helper
#   ~/.local/share/applications/…desktop             makes it an applet for the panel
#   ~/.local/share/icons/hicolor/scalable/apps/…svg  the bulb icon
# Then add it in COSMIC Settings › Desktop › Panel › Applets.
set -euo pipefail
cd "$(dirname "$0")"
id=io.github.ubruckhaus.CosmicHueLights
data="${XDG_DATA_HOME:-$HOME/.local/share}"
bin="$HOME/.local/bin"

cargo build --release
install -Dm755 target/release/cosmic-hue-lights "$bin/cosmic-hue-lights"
install -Dm644 scripts/hue-lights.py "$data/cosmic-hue-lights/scripts/hue-lights.py"
install -Dm644 "data/icons/$id-symbolic.svg" "$data/icons/hicolor/scalable/apps/$id-symbolic.svg"
# The panel starts applets without ~/.local/bin in PATH: the full path.
sed "s|^Exec=.*|Exec=$bin/cosmic-hue-lights|" "data/$id.desktop" > "$data/applications/$id.desktop"
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
echo "Installed. Add “Hue Lights” in COSMIC Settings › Desktop › Panel › Applets."
