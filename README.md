# Maprisma Hue Lights for COSMIC

A native COSMIC panel applet (Rust/libcosmic) for Philips Hue: rooms, zones, lights and
scenes. One of the [Maprisma](https://github.com/maprisma) apps: the COSMIC port of the Omarchy
plugin `ubruckhaus.hue-lights`, which stays as it is. Interface languages: English, Deutsch, Français, Español, Italiano, Português, Nederlands and Polski. Choose Language in the popup/window or tray menu. The default follows LC_ALL, LC_MESSAGES, then LANG (first non-empty value). Preferences are stored in `~/.config/maprisma-hue-lights/config.json` (or under XDG_CONFIG_HOME). Changes apply immediately; other running views refresh on their next UI update. Catalogs are bundled offline; regenerate them with `python3 tools/gen-translations` after editing translations.

- **Panel:** a bulb, in the theme accent while any light is on, dim when all are off or no bridge is
  paired. Left click opens the popup; right click switches every light off. The tooltip
  says how many lights are on.
- **Popup:** the bridge and "n of m on"; one card per room and zone (icon from its name or
  Hue archetype, status, on/off switch, brightness while on). A card opens to its single
  lights (switch, brightness if dimmable) and scenes (the active one highlighted, "+N
  more"). All off, Refresh, and the bridge with Unpair (asks again before it acts).
- **Pairing:** found bridges with Pair, or an IP address by hand; then press the bridge's
  link button.

All bridge traffic goes through `scripts/hue-lights.py`, the same helper as before:
JSON commands on stdin, JSON snapshots on stdout (every 2 s while the popup is open, 20 s
while closed), pinned HTTPS to the bridge on the LAN only. The app key stays in
`~/.config/hue-lights/config.json` (0600) and never reaches the applet.

## F12: a screenshot with a note

While the popup (or the KDE window) is open, **F12** (without Ctrl, Shift, Alt or Super) saves a picture of it,
for telling what is wrong with it:

- **Only on F12.** Nothing is captured at any other time, and only this app's own popup or window: libcosmic draws
  it once more into memory (iced's `window::screenshot`) and the app encodes that as PNG. No screen capture tool
  or portal runs, nothing else on the screen is in it, and nothing is uploaded or sent anywhere (the picture may
  show your room and light names; it stays on this computer).
- **Where:** `~/Pictures/Maprisma Hue Lights/maprisma-hue-lights-<date>-<time>.png` (the pictures folder from
  `~/.config/user-dirs.dirs` when it is set there; that file is only read when it is a small regular file). The
  popup then names the file and asks **What is wrong? (optional)**; **Enter** or **Done** saves the text next to
  the picture as a `.txt` with the same name (at most 4000 characters; no text, no file), **Esc** skips it, and
  closing the popup keeps what was typed.
- **Private files:** the folder is made `0700` when missing and only used when it is a real folder (not a link) of
  yours that no one else can write to; pictures and notes are new `0600` files created inside that folder,
  never written through a link or over an existing file or note (`-2`, `-3` … are added instead), and a file that
  cannot be written completely is removed again.
- **Off:** switch **F12 saves a screenshot** off in the popup (stored as `"screenshot_key": false` in
  `~/.config/maprisma-hue-lights/config.json`, beside the language; the bridge's app key is not in that file).

The KDE tray icon's menu is drawn by Plasma and has no F12.

## Corners

**Corners** in the popup or window: *Like the desktop (… px)* (the default: the COSMIC theme's corner radius, 16,
8 or 2 px by COSMIC Settings › Appearance; on KDE the theme libcosmic loads there, normally 16 px), *Slightly
rounded (6 px)*, *Rounded (12 px)* or *Square*. Room cards and the popup's frame take the full radius; buttons,
scene buttons, drop-downs, the IP and note fields at most 8 px; small parts (the room icon badges, light colour
swatches, switches' and sliders' tracks as libcosmic draws them) at most 4 px; the "n of m on" pill stays round, or
square in *Square*. The panel icon and the KDE window's own frame keep the desktop's corners. Stored as
`"corners"` in `config.json`; *Like the desktop* removes it again. Files without these keys stay valid.

## Install

```sh
./install.sh   # builds, installs into ~/.local
```

Then add **Hue Lights** in COSMIC Settings › Desktop › Panel › Applets.

## Tests

```sh
env -u DISPLAY -u WAYLAND_DISPLAY python3 -m unittest discover -s tests
```

None of the tests talks to a real bridge.

## KDE Plasma

On KDE Plasma 6.8 (Wayland), run `./install.sh`, then open **Maprisma Hue Lights**
from the launcher, or run `~/.local/bin/maprisma-hue-lights --window`. The normal,
resizable window has the same controls as the COSMIC popup, including pairing,
rooms, zones, individual lights, scenes and refresh. Only one window runs per session.

The installer also installs `maprisma-hue-lights-tray` and a KDE-only autostart entry
(`~/.config/autostart/io.github.ubruckhaus.CosmicMaprismaHueLights.Tray.desktop`).
Start the tray manually once, or log out and back in. Left click toggles the window;
the menu offers **Open Maprisma Hue Lights** (brings an open window to the front, also
a minimized one) and **Quit tray**. Quitting the tray leaves an open window running. The
bulb is lit while any light is on, as in the COSMIC panel. The tooltip uses the same
bridge helper and status text as the COSMIC panel, polling at the helper's inexpensive closed-popup rate
(20 seconds). The tray reconnects when Plasma's StatusNotifierWatcher restarts.
COSMIC continues to use the existing panel applet without `--window`.

Rust checks (no bridge required):

```sh
cargo test
cargo build --release --offline
```
