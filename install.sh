#!/usr/bin/env bash
# Build and install Maprisma Lights for the current user (no root needed):
#   ~/.local/bin/maprisma-hue-lights                   the applet (symlinked as maprisma-lights)
#   ~/.local/share/maprisma-hue-lights/scripts/        the bridge & multi-brand helper
#   ~/.local/share/applications/…desktop             makes it an applet for the panel
#   ~/.local/share/icons/hicolor/scalable/apps/…svg  the bulb icons (lit, lit in the accent, off)
# Then add it in COSMIC Settings › Desktop › Panel › Applets.
set -euo pipefail
cd "$(dirname "$0")"
id=io.github.ubruckhaus.CosmicMaprismaHueLights
data="${XDG_DATA_HOME:-$HOME/.local/share}"
bin="$HOME/.local/bin"

cargo build --release
install -Dm755 target/release/maprisma-hue-lights "$bin/maprisma-hue-lights"
install -Dm755 target/release/maprisma-hue-lights-tray "$bin/maprisma-hue-lights-tray"
ln -sf maprisma-hue-lights "$bin/maprisma-lights"
# Desktop Exec has two escaping layers: argument quoting, then string-value escaping.
desktop_exec() {
    local v=$1
    v=${v//\\/\\\\}
    v=${v//\"/\\\"}
    v=${v//\`/\\\`}
    v=${v//\$/\\\$}
    v=${v//\%/%%}
    v=${v//\\/\\\\}
    printf '"%s"' "$v"
}
mkdir -p "$data/applications" "${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
window_exec="$(desktop_exec "$bin/maprisma-hue-lights") --window"
tray_exec="$(desktop_exec "$bin/maprisma-hue-lights-tray")"
cat > "$data/applications/$id.Window.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Maprisma Lights
Name[de]=Maprisma Lights
Comment[de]=Smarte Beleuchtung für mehrere Marken & Grundriss-Studio in der Leiste
Name[fr]=Maprisma Lights
Comment[fr]=Éclairage connecté multimarque et Studio Plan dans le panneau
Name[es]=Maprisma Lights
Comment[es]=Iluminación inteligente multimarca y estudio de planos en el panel
Name[it]=Maprisma Lights
Comment[it]=Illuminazione smart multimarca e studio planimetrie nel pannello
Name[pt]=Maprisma Lights
Comment[pt]=Iluminação inteligente multimarca e estúdio de plantas no painel
Name[nl]=Maprisma Lights
Comment[nl]=Slimme verlichting voor meerdere merken en plattegrondstudio in het paneel
Name[pl]=Maprisma Lights
Comment[pl]=Inteligentne oświetlenie wielu marek i studio planów w panelu
Comment=Multi-brand smart lighting & Visual Floorplan Studio in the panel
Exec=$window_exec
Icon=$id-symbolic
Terminal=false
NoDisplay=false
Categories=Utility;
StartupWMClass=$id.Window
EOF
cat > "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/$id.Tray.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Maprisma Lights Tray
Name[de]=Maprisma Lights – Tray
Name[fr]=Maprisma Lights – Zone de notification
Name[es]=Maprisma Lights – Bandeja
Name[it]=Maprisma Lights – Area di notifica
Name[pt]=Maprisma Lights – Bandeja
Name[nl]=Maprisma Lights – Systeemvak
Name[pl]=Maprisma Lights – Zasobnik
Exec=$tray_exec
Icon=$id-symbolic
Terminal=false
OnlyShowIn=KDE;Hyprland;
EOF
install -Dm644 scripts/hue-lights.py "$data/maprisma-hue-lights/scripts/hue-lights.py"
if [[ -f scripts/hue_providers.py ]]; then
    install -Dm644 scripts/hue_providers.py "$data/maprisma-hue-lights/scripts/hue_providers.py"
fi
if [[ -f scripts/hue-ai.py ]]; then
    install -Dm644 scripts/hue-ai.py "$data/maprisma-hue-lights/scripts/hue-ai.py"
fi
if [[ -f scripts/hue-mcp.py ]]; then
    install -Dm755 scripts/hue-mcp.py "$data/maprisma-hue-lights/scripts/hue-mcp.py"
    ln -sf "$data/maprisma-hue-lights/scripts/hue-mcp.py" "$bin/maprisma-hue-mcp"
fi
install -Dm644 "data/icons/$id-symbolic.svg" "$data/icons/hicolor/scalable/apps/$id-symbolic.svg"
# The KDE/Hyprland tray's bulb while every light is off.
install -Dm644 data/icons/hue-bulb-off-symbolic.svg "$data/icons/hicolor/scalable/apps/$id-off-symbolic.svg"
install -Dm644 data/icons/hue-bulb-active-symbolic.svg "$data/icons/hicolor/scalable/apps/$id-active-symbolic.svg"
# The panel starts applets without ~/.local/bin in PATH: the full path.
while IFS= read -r line; do
    if [[ $line == Exec=* ]]; then
        printf 'Exec=%s\n' "$(desktop_exec "$bin/maprisma-hue-lights")"
    else
        printf '%s\n' "$line"
    fi
done < "data/$id.desktop" > "$data/applications/$id.desktop"
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
echo "Installed. Add “Maprisma Lights” in COSMIC Settings › Desktop › Panel › Applets."
