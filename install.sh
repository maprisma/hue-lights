#!/usr/bin/env bash
# Build and install Maprisma Hue Lights for the current user (no root needed):
#   ~/.local/bin/maprisma-hue-lights                   the applet
#   ~/.local/share/maprisma-hue-lights/scripts/        the bridge helper
#   ~/.local/share/applications/…desktop             makes it an applet for the panel
#   ~/.local/share/icons/hicolor/scalable/apps/…svg  the bulb icons (lit, off)
# Then add it in COSMIC Settings › Desktop › Panel › Applets.
set -euo pipefail
cd "$(dirname "$0")"
id=io.github.ubruckhaus.CosmicMaprismaHueLights
data="${XDG_DATA_HOME:-$HOME/.local/share}"
bin="$HOME/.local/bin"

cargo build --release
install -Dm755 target/release/maprisma-hue-lights "$bin/maprisma-hue-lights"
install -Dm755 target/release/maprisma-hue-lights-tray "$bin/maprisma-hue-lights-tray"
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
Name=Maprisma Hue Lights
Name[de]=Maprisma Hue Lights
Comment[de]=Philips-Hue-Räume, Zonen, Lampen und Szenen in der Leiste
Name[fr]=Maprisma Hue Lights
Comment[fr]=Pièces, zones, lampes et scènes Philips Hue dans le panneau
Name[es]=Maprisma Hue Lights
Comment[es]=Habitaciones, zonas, luces y escenas Philips Hue en el panel
Name[it]=Maprisma Hue Lights
Comment[it]=Stanze, zone, luci e scene Philips Hue nel pannello
Name[pt]=Maprisma Hue Lights
Comment[pt]=Divisões, zonas, luzes e cenas Philips Hue no painel
Name[nl]=Maprisma Hue Lights
Comment[nl]=Philips Hue-kamers, zones, lampen en scènes in het paneel
Name[pl]=Maprisma Hue Lights
Comment[pl]=Pokoje, strefy, lampy i sceny Philips Hue w panelu
Comment=Philips Hue rooms, zones, lights and scenes in the panel
Exec=$window_exec
Icon=$id-symbolic
Terminal=false
NoDisplay=false
Categories=Utility;
StartupWMClass=$id
EOF
cat > "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/$id.Tray.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Maprisma Hue Lights Tray
Name[de]=Maprisma Hue Lights – Tray
Name[fr]=Maprisma Hue Lights – Zone de notification
Name[es]=Maprisma Hue Lights – Bandeja
Name[it]=Maprisma Hue Lights – Area di notifica
Name[pt]=Maprisma Hue Lights – Bandeja
Name[nl]=Maprisma Hue Lights – Systeemvak
Name[pl]=Maprisma Hue Lights – Zasobnik
Exec=$tray_exec
Icon=$id-symbolic
Terminal=false
OnlyShowIn=KDE;Hyprland;
EOF
install -Dm644 scripts/hue-lights.py "$data/maprisma-hue-lights/scripts/hue-lights.py"
install -Dm644 "data/icons/$id-symbolic.svg" "$data/icons/hicolor/scalable/apps/$id-symbolic.svg"
# The KDE/Hyprland tray's bulb while every light is off.
install -Dm644 data/icons/hue-bulb-off-symbolic.svg "$data/icons/hicolor/scalable/apps/$id-off-symbolic.svg"
# The panel starts applets without ~/.local/bin in PATH: the full path.
while IFS= read -r line; do
    if [[ $line == Exec=* ]]; then
        printf 'Exec=%s\n' "$(desktop_exec "$bin/maprisma-hue-lights")"
    else
        printf '%s\n' "$line"
    fi
done < "data/$id.desktop" > "$data/applications/$id.desktop"
gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
echo "Installed. Add “Maprisma Hue Lights” in COSMIC Settings › Desktop › Panel › Applets."
