# Maprisma Hue Lights for COSMIC

A native COSMIC panel applet (Rust/libcosmic) for Philips Hue: rooms, zones, lights and
scenes. One of the [Maprisma](https://github.com/maprisma) apps: the COSMIC port of the Omarchy
plugin `ubruckhaus.hue-lights`, which stays as it is. English only.

- **Panel:** a bulb, green while any light is on, dim when all are off or no bridge is
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
