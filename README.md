# Maprisma Lights

A native Linux desktop & panel app (Rust/libcosmic) for **multi-brand local smart lighting** on **COSMIC**, **KDE Plasma**, and **Hyprland** *(formerly Maprisma Hue Lights)*. Control **Philips Hue**, **Home Assistant**, **WLED**, **WiZ**, **LIFX**, **Govee LAN**, **Nanoleaf**, **Shelly**, and **IKEA DIRIGERA** directly over your local network with zero cloud account. Design an interactive **Floorplan Studio v2** for your home with polygon rooms, multiple floors, a starter gallery, and a Future Lighting Planner — alongside rooms, zones, individual lights, color, Kelvin warmth, native effects, per-lamp Light Themes, schedules, linked-light rules, and a real-time Audio-Reactive Light Synthesizer.

![Maprisma Lights](preview.png)

## What's New in v0.5.0

- **Renamed to Maprisma Lights:** Clean multi-brand identity across the UI, desktop entries, tray menu, Help, and all 8 languages (while keeping internal IDs, binaries, and `~/.config/hue-lights/` 100% backwards-compatible, plus a new `~/.local/bin/maprisma-lights` convenience symlink).
- **Multi-Brand Local Lighting Providers:** Connect **Philips Hue**, **Home Assistant** (100+ brands via local REST API), **WLED**, **WiZ**, **LIFX**, **Govee LAN**, **Nanoleaf**, **Shelly**, and **IKEA DIRIGERA** side-by-side over your local LAN — with or without a Hue Bridge.
- **Floorplan Studio v2 (`Map` & `Plan` modes):**
  - **Custom Polygon Rooms:** Start from rectangles or L-Shape, Bay Window, T-Shape, Hallway, or Angled Wall shapes; drag midpoints to add corners and right-click corners to remove them.
  - **Multi-Floor Support:** Add, rename, switch, and delete floors with per-floor lit counts and 1-click Undo.
  - **Day / Dusk / Night Ambient Simulation** & **Starter Floorplan Gallery** (with optional pre-populated lighting plan).
  - **Future Lighting Planner (`Plan` mode):** Place virtual fixtures & bendable LED lightstrips with customizable lumens, Kelvin warmth, beam radius, and price; see live estimated room illuminance (Lux) and a shopping list; bind planned fixtures to real paired lights later.
- **Safer UX & macOS Parity:** Confirmation prompts before deleting a room/zone on your Hue Bridge or deleting rules/schedules, Retry action on connection errors, clearer trademark-clean onboarding copy, and improved French (`Map` → `Carte`) & 8-language translations.

## Highlights

- **Floorplan Studio v2 (`Map` / `Plan` / `Rules`):** Place lamps and sensors freely from a searchable **Unplaced / Placed** sidebar onto a 2D multi-floor canvas with live color halos, polygon room shapes, furniture stamps (*Door, Window, Sofa, TV, Desk, Bed, Dining, Kitchen, Stairs*), custom background blueprint images (PNG/JPG/WebP via XDG Desktop Portal), 12 custom vector lamp icons, Room Scenes & 1-click **⏰ Schedule scene**, typed Bezier dependency wires, Future Lighting Planner, and **1-click Sync All** to your Philips Hue Bridge.
- **Multi-Brand Lights, Rooms & Scenes:** Unified cards per room and zone with provider pills, live Hue CLIP v2 SSE updates + local polling for extra providers, brightness sliders, filter pills (*All, On, Rooms, Zones, Favorites*), pinned **Favorites** strip, smart **All On** (with per-room *Exclude from All On* + optional confirmation) and **All Off**.
- **Color, Kelvin Warmth, Effects & Per-Lamp Light Themes:** Expand any light for warmth presets (2000K–6500K), color swatches, native Hue effects (*Candle, Fire, Sparkle, Prism, …*), 8 curated **Light Themes** (*Reading, Relax, Focus, Sunset Glow, Deep Ocean, Cyberpunk, Fireplace, Nightlight*), plus custom themes saved to `themes.json` and *"Save current as theme"*.
- **Schedules (`Bridge` or `Local`):** Create, toggle, and delete per-light, per-room, or **room-scene** schedules with weekday recurrence, brightness, and fade duration. Run either **On Hue Bridge** (`behavior_instance`, 24/7 even when your PC is off, including sunrise/sunset) or **On this PC (live)** (`local_schedules.json`, works across all brands).
- **Visual & Linked-Light Rules:** Link lights, rooms, and sensors across brands — in the Rules tab or visually on the Floorplan Map — with **Follow / Mirror**, **Exclusive**, and **Companion** rules (optional brightness matching, *Only if off*, overnight time windows, static cycle detection, state-aware echo suppression, pause toggle, and activity log).
- **Audio-Reactive Light Synthesizer (`Synth` tab):** Live 3-band Bass / Mids / Treble meters with beat detection driving your lights in real time from **System audio** (`parec @DEFAULT_MONITOR@`), **Microphone** (`parec @DEFAULT_SOURCE@`), or built-in **Demo beat**. Audio is analyzed strictly in memory and never recorded or transmitted.
- **Bridge Pro MotionAware™ & Smart Sensors:** Live presence badges on Hue Bridge Pro MotionAware™ lights, plus physical motion sensors, contact sensors, and wireless switches/buttons — placeable directly on the Floorplan Map and usable as live automation rule triggers.
- **Interactive Animated Help & Feature Guide (`?`):** Built-in 9-card feature tour with live smooth-looping pure-vector canvas previews and step-by-step walkthroughs.
- **Local MCP Server for AI Agents (`--mcp`):** Built-in local stdio [Model Context Protocol](https://modelcontextprotocol.io) server (`maprisma-hue-lights --mcp` or `maprisma-hue-mcp`) exposing rooms, lights, scenes, themes, sensors, MotionAware™, schedules, and rules to external AI coding & desktop agents. Credentials stay strictly inside the local backend and are never exposed.
- **Ask AI (Optional, Off by Default):** Opt-in natural-language control for lamps, rooms, scenes, themes, rules, and schedules — **disabled by default** (zero AI UI until enabled in **Settings → Ask AI (Optional)**). Works 100% locally offline (English & German) with optional local-only Ollama fallback (`http://127.0.0.1:11434`).
- **6 Skins, 8 Selectable Panel Icons & 8 Languages:** *System Native, Neon Synthwave, Cozy Cabin, Retro Terminal, Disco Lava*, or *Minimal Zen*, 8 symbolic panel icons (with optional lit-count badge), **Corners**, **Text size**, and 8 languages (*English, Deutsch, Français, Español, Italiano, Português, Nederlands, Polski*).
- **Saved Window Size & Screen Position:** Resizing or moving the `--window` window persists its size and (on Hyprland) screen position cleanly across restarts and across all 5 tabs.

## Architecture & Privacy

All local device traffic goes through `scripts/hue-lights.py` + `scripts/hue_providers.py` (Python 3 standard library only): JSON commands on stdin, JSON snapshots on stdout over live CLIP v2 SSE with REST/UDP/HTTP local LAN protocols, pinned HTTPS (SHA-256 fingerprint + CN == bridge ID for Philips Hue) on your LAN only.

- Bridge & provider credentials stay in `~/.config/hue-lights/` (`0600`) and never reach the UI process.
- Floorplan layouts (`floorplan.json` + optional copied `floorplan_bg.<ext>`), custom themes, rules, and local schedules live in `~/.config/hue-lights/` (`0600`).
- UI preferences (language, favorites, exclusions, skin, panel icon, corners, window size & position) live in `~/.config/maprisma-hue-lights/config.json`.

## F12: Screenshot with Optional Note

While the popup or `--window` window is open, **F12** (without modifiers) renders the app's own window in memory via iced's `window::screenshot` and saves a private `0600` PNG to `~/Pictures/Maprisma Lights/` with an optional `.txt` note. Switch it off anytime in **Settings → F12 saves a screenshot**.

## Install

```sh
./install.sh   # builds release binaries and installs into ~/.local
```

- **COSMIC:** Add **Maprisma Lights** in *COSMIC Settings → Desktop → Panel → Applets*.
- **KDE Plasma / Hyprland:** Open **Maprisma Lights** from your launcher or run `~/.local/bin/maprisma-lights --window` (or `maprisma-hue-lights --window`; also installs the `maprisma-hue-lights-tray` indicator).

## Tests

```sh
env -u DISPLAY -u WAYLAND_DISPLAY python3 -m unittest discover -s tests
env -u DISPLAY -u WAYLAND_DISPLAY cargo test
```

None of the tests talks to a real bridge or smart light.

## License

MIT
