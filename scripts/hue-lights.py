#!/usr/bin/env python3
"""Philips Hue bridge helper for the Hue Lights GNOME Shell extension.

Long-running process: reads one JSON command per line on stdin and prints one
JSON snapshot per line on stdout. Talks to the bridge's local CLIP v2 API over
HTTPS only. The bridge certificate is self-signed by Philips, so it is pinned
(SHA-256 fingerprint + CN == bridge id) when pairing and checked on every
connection afterwards. The application key lives in
$XDG_CONFIG_HOME/hue-lights/config.json (~/.config/hue-lights, mode 0600) and
never leaves this process.
"""

import atexit
import fcntl
import hashlib
import http.client
import ipaddress
import json
import math
import os
import re
import select
import shutil
import signal
import socket
import ssl
import stat
import struct
import tempfile
import subprocess
import sys
import threading
import time
import uuid

def _config_home():
    """$XDG_CONFIG_HOME when it is an absolute path (the spec ignores relative ones), else ~/.config."""
    home = os.environ.get("XDG_CONFIG_HOME", "")
    return home if os.path.isabs(home) else os.path.join(os.path.expanduser("~"), ".config")


CONFIG_DIR = os.path.join(_config_home(), "hue-lights")
CONFIG_PATH = os.path.join(CONFIG_DIR, "config.json")

POLL_OPEN = 2.0       # seconds between refreshes while the panel is open
POLL_CLOSED = 20.0    # seconds between refreshes while it is closed
PAIR_WINDOW = 35.0    # how long to keep asking for a key after "pair"
LIGHT_INTERVAL = 0.12 # Hue guidance: ~10 light commands per second
GROUP_INTERVAL = 0.6  # ... and ~1 group command per second
HTTP_TIMEOUT = 4.0     # overall deadline per request: connect, TLS, headers and body
# The bridge always listens on 443. HUE_LIGHTS_BRIDGE_PORT exists for the test harness's
# mock bridge (tools/mock-bridge.py), which cannot bind 443 without root.
try:
    BRIDGE_PORT = int(os.environ.get("HUE_LIGHTS_BRIDGE_PORT", "443"))
except ValueError:
    BRIDGE_PORT = 443
if not 1 <= BRIDGE_PORT <= 65535:
    BRIDGE_PORT = 443
MAX_BODY = 4 * 1024 * 1024
MAX_CONFIG = 64 * 1024
MAX_PENDING = 256     # queued light/group/scene commands at most (one per resource)
MAX_COMMAND = 4096    # bytes per panel command line; longer lines are dropped unread
# Rooms and zones name their lights directly or through devices, and nothing stops
# a bridge from repeating them: each group keeps a light once and at most
# MAX_GROUP_LIGHTS of them, all groups together at most MAX_MEMBERSHIPS. The lights
# in no room ("Other lights") are a group like the others: the same two limits.
MAX_GROUP_LIGHTS = 150
MAX_MEMBERSHIPS = 1000
# Each room or zone ID is shown once. Scenes are kept once per ID, at most
# MAX_GROUP_SCENES per group and MAX_SCENES on the whole bridge, and every scene
# goes to the one group it names, so the model never holds more than MAX_SCENES.
MAX_GROUP_SCENES = 100
MAX_SCENES = 1000
MAX_SCHEDULES = 100
MAX_LOCAL_SCHEDULES = 50
MAX_THEMES = 40
MAX_RULES = 100
MAX_RULE_TARGETS = 16
MAX_RULE_LOG = 20
MAX_API_QUEUE = 32
MAX_EFFECTS = 64
RULE_ECHO_WINDOW = 1.5
RULE_COOLDOWN = 2.0
MAX_CASCADE_DEPTH = 3
SSE_BACKOFF = (2.0, 5.0, 15.0, 30.0)
SSE_READ_TIMEOUT = 15.0
LOCAL_SCHEDULE_POLL_CAP = 15.0
DEFAULT_SCHEDULE_SCRIPT_ID = "7e571ac6-f363-42e1-809a-49656328a7a7"

UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
BRIDGE_ID_RE = re.compile(r"^[0-9A-F]{16}$")
APP_KEY_RE = re.compile(r"[A-Za-z0-9_-]{1,100}")     # the bridge's application key ("username")
CLIENT_KEY_RE = re.compile(r"[0-9A-Fa-f]{1,64}")    # the entertainment client key
EFFECT_RE = re.compile(r"^[a-z0-9_]{1,32}$")
TIME_HHMM_RE = re.compile(r"^([01]\d|2[0-3]):([0-5]\d)$")
THEME_ID_RE = re.compile(r"^[a-zA-Z0-9_-]{1,64}$")
HEX_COLOR_RE = re.compile(r"^#?[0-9a-fA-F]{6}$")
DAY_SHORT_TO_FULL = {
    "mon": "monday", "tue": "tuesday", "wed": "wednesday",
    "thu": "thursday", "fri": "friday", "sat": "saturday", "sun": "sunday",
}
DAY_FULL_TO_SHORT = {v: k for k, v in DAY_SHORT_TO_FULL.items()}
DAY_ORDER = ("mon", "tue", "wed", "thu", "fri", "sat", "sun")
RULE_KINDS = ("follow", "exclusive", "companion", "desktop")
SCHEDULE_MODES = ("bridge", "local")
MAX_SECRETS = 8

BUILTIN_THEMES = [
    {"id": "reading", "name": "Reading", "hex": "#ffd9a0", "mirek": 320, "bri": 85, "effect": "no_effect", "built_in": True},
    {"id": "relax", "name": "Relax", "hex": "#ffb86b", "mirek": 420, "bri": 55, "effect": "no_effect", "built_in": True},
    {"id": "focus", "name": "Focus", "hex": "#e8f2ff", "mirek": 210, "bri": 100, "effect": "no_effect", "built_in": True},
    {"id": "sunset", "name": "Sunset Glow", "hex": "#ff6b35", "mirek": None, "bri": 70, "effect": "no_effect", "built_in": True},
    {"id": "ocean", "name": "Deep Ocean", "hex": "#1e88e5", "mirek": None, "bri": 65, "effect": "no_effect", "built_in": True},
    {"id": "cyberpunk", "name": "Cyberpunk", "hex": "#ff2e93", "mirek": None, "bri": 85, "effect": "no_effect", "built_in": True},
    {"id": "fireplace", "name": "Fireplace", "hex": "#ff7b29", "mirek": 450, "bri": 60, "effect": "candle", "built_in": True},
    {"id": "nightlight", "name": "Nightlight", "hex": "#ff9e44", "mirek": 500, "bri": 12, "effect": "no_effect", "built_in": True},
]
BUILTIN_THEME_IDS = frozenset(t["id"] for t in BUILTIN_THEMES)

SYNTH_SOURCES = ("system", "mic", "demo")
SYNTH_MODES = ("club", "chill", "ambient", "strobe")
SYNTH_BANDS = ("all", "bass", "mids", "treble")
SYNTH_PALETTES = {
    "neon": ("#ff007f", "#00f5ff", "#8a2be2", "#39ff14"),
    "sunset": ("#ff4e50", "#fc913a", "#f9d423", "#c94b4b"),
    "ocean": ("#005c97", "#00b4db", "#38ef7d", "#1e3c72"),
    "aurora": ("#00f260", "#0575e6", "#7f00ff", "#00d2ff"),
    "fire": ("#ff1e00", "#ff6a00", "#ffb300", "#ff3d00"),
    "pastel": ("#ffb3ba", "#bae1ff", "#baffc9", "#e0bbff"),
}
SYNTH_LIGHT_INTERVAL = 0.22
SYNTH_EMIT_OPEN = 0.10
SYNTH_EMIT_CLOSED = 0.50
SYNTH_BASS_FREQS = (60.0, 100.0, 150.0)
SYNTH_MIDS_FREQS = (400.0, 800.0, 1500.0)
SYNTH_TREBLE_FREQS = (3000.0, 5000.0, 7000.0)


# ---------------------------------------------------------------------------
# Pure helpers (unit tested)

def valid_uuid(value):
    return isinstance(value, str) and UUID_RE.match(value) is not None


def valid_bridge_ip(value):
    """Only private / link-local IPv4 literals: the bridge is always on the LAN."""
    try:
        ip = ipaddress.IPv4Address(str(value))
    except (ipaddress.AddressValueError, ValueError):
        return False
    return ip.is_private and not ip.is_loopback and not ip.is_unspecified and not ip.is_multicast


def clamp(value, low, high):
    return max(low, min(high, value))


def finite(value, default, low, high):
    """A bridge or panel number as a float in [low, high]; `default` for NaN, infinities and junk
    (json.loads accepts NaN/Infinity, which the panel's JSON.parse would reject)."""
    try:
        number = float(value)
    except (TypeError, ValueError, OverflowError):
        return default
    return clamp(number, low, high) if math.isfinite(number) else default


def clean(text, limit=60):
    """Names come from the bridge or mDNS: no markup or control characters ever reach Qt."""
    text = "".join(ch for ch in str(text or "") if ch.isprintable() and ch not in "<>&")
    return text.strip()[:limit]


def fingerprint(der):
    return hashlib.sha256(der).hexdigest()


def _der_items(buf, pos, end):
    """Yield (tag, content_start, content_end) for the DER items in buf[pos:end]."""
    while pos < end:
        tag = buf[pos]
        length = buf[pos + 1]
        pos += 2
        if length & 0x80:
            count = length & 0x7F
            length = int.from_bytes(buf[pos:pos + count], "big")
            pos += count
        yield tag, pos, pos + length
        pos += length


def cert_common_name(der):
    """Subject CN of a DER certificate (Certificate > TBSCertificate > subject)."""
    try:
        _, cert_start, cert_end = next(_der_items(der, 0, len(der)))
        _, tbs_start, tbs_end = next(_der_items(der, cert_start, cert_end))
        fields = list(_der_items(der, tbs_start, tbs_end))
        if fields and fields[0][0] == 0xA0:  # explicit version
            fields = fields[1:]
        _, subj_start, subj_end = fields[4]   # serial, sigAlg, issuer, validity, subject
        for _, set_start, set_end in _der_items(der, subj_start, subj_end):
            for _, atv_start, atv_end in _der_items(der, set_start, set_end):
                (_, oid_s, oid_e), (_, val_s, val_e) = list(_der_items(der, atv_start, atv_end))[:2]
                if der[oid_s:oid_e] == b"\x55\x04\x03":
                    return der[val_s:val_e].decode("utf-8", "replace")
    except (StopIteration, IndexError, ValueError):
        pass
    return ""


def _gamma(c):
    c = clamp(c, 0.0, 1.0)
    return 12.92 * c if c <= 0.0031308 else 1.055 * (c ** (1 / 2.4)) - 0.055


def xy_to_hex(x, y):
    """CIE xy (Hue colour) to an sRGB hex string at full brightness, for a swatch."""
    x, y = finite(x, None, 0.0, 1.0), finite(y, None, 0.0, 1.0)
    if x is None or y is None or y <= 0:
        return "#ffd27a"
    z = 1.0 - x - y
    Y = 1.0
    X = (Y / y) * x
    Z = (Y / y) * z
    r = X * 1.656492 - Y * 0.354851 - Z * 0.255038
    g = -X * 0.707196 + Y * 1.655397 + Z * 0.036152
    b = X * 0.051713 - Y * 0.121364 + Z * 1.011530
    peak = max(r, g, b)
    if peak > 1:
        r, g, b = r / peak, g / peak, b / peak
    return "#%02x%02x%02x" % tuple(int(round(_gamma(c) * 255)) for c in (r, g, b))


def _inv_gamma(c):
    c = clamp(c, 0.0, 1.0)
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def hex_to_xy(hex_str):
    """Convert '#rrggbb' (or 'rrggbb') sRGB hex to Hue CIE [x, y] clamped to [0, 1], or None."""
    if not isinstance(hex_str, str):
        return None
    s = hex_str.strip()
    if not HEX_COLOR_RE.fullmatch(s):
        return None
    if s.startswith("#"):
        s = s[1:]
    r = _inv_gamma(int(s[0:2], 16) / 255.0)
    g = _inv_gamma(int(s[2:4], 16) / 255.0)
    b = _inv_gamma(int(s[4:6], 16) / 255.0)
    # Wide RGB D65 matrix (inverse of xy_to_hex)
    X = r * 0.664511 + g * 0.154324 + b * 0.162028
    Y = r * 0.283881 + g * 0.668433 + b * 0.047685
    Z = r * 0.000088 + g * 0.072310 + b * 0.986039
    total = X + Y + Z
    if total <= 0.0:
        return [0.3127, 0.329]
    x = clamp(X / total, 0.0, 1.0)
    y = clamp(Y / total, 0.0, 1.0)
    return [round(x, 4), round(y, 4)]


def mirek_to_hex(mirek):
    """Colour temperature (mirek) to an approximate sRGB hex (Tanner Helland fit)."""
    mirek = finite(mirek, None, 1.0, 1000000.0)
    if mirek is None:
        return "#ffd27a"
    kelvin = 1000000.0 / mirek
    t = clamp(kelvin, 1000, 40000) / 100.0
    import math
    if t <= 66:
        r = 255
        g = 99.4708025861 * math.log(t) - 161.1195681661
        b = 0 if t <= 19 else 138.5177312231 * math.log(t - 10) - 305.0447927307
    else:
        r = 329.698727446 * ((t - 60) ** -0.1332047592)
        g = 288.1221695283 * ((t - 60) ** -0.0755148492)
        b = 255
    return "#%02x%02x%02x" % tuple(int(clamp(c, 0, 255)) for c in (r, g, b))


def light_color(light):
    ct = light.get("color_temperature") or {}
    if ct.get("mirek_valid") and ct.get("mirek"):
        return mirek_to_hex(ct["mirek"])
    xy = (light.get("color") or {}).get("xy")
    if xy:
        return xy_to_hex(xy.get("x"), xy.get("y"))
    if ct.get("mirek"):
        return mirek_to_hex(ct["mirek"])
    return "#ffd27a"


def brightness(value):
    """A bridge brightness in 0..100; anything else (a string, NaN, an infinity) makes the whole
    reply one this app does not understand, rather than a value the panel cannot parse."""
    number = finite(value, None, 0.0, 100.0)
    if number is None:
        raise ValueError("unexpected brightness")
    return number


def blend_colors(colors):
    """Average of '#rrggbb' strings (the room's swatch), warm white when empty."""
    rgb = [tuple(int(c[i:i + 2], 16) for i in (1, 3, 5)) for c in colors if re.fullmatch(r"#[0-9a-f]{6}", c or "")]
    if not rgb:
        return "#ffd27a"
    return "#%02x%02x%02x" % tuple(int(round(sum(ch[i] for ch in rgb) / len(rgb))) for i in range(3))


def build_model(resources):
    """Turn the flat CLIP v2 /resource list into rooms -> lights / scenes."""
    by_type = {}
    for res in resources if isinstance(resources, list) else []:
        if isinstance(res, dict):
            by_type.setdefault(res.get("type"), []).append(res)
    lights = {r["id"]: r for r in by_type.get("light", []) if valid_uuid(r.get("id"))}
    grouped = {r["id"]: r for r in by_type.get("grouped_light", []) if valid_uuid(r.get("id"))}
    devices = {r["id"]: r for r in by_type.get("device", []) if valid_uuid(r.get("id"))}
    connectivity = {}
    for zc in by_type.get("zigbee_connectivity", []):
        owner = (zc.get("owner") or {}).get("rid")
        if owner:
            connectivity[owner] = zc.get("status") == "connected"

    def light_entry(light):
        dimming = light.get("dimming") if isinstance(light.get("dimming"), dict) else {}
        owner = (light.get("owner") or {}).get("rid")
        ct = light.get("color_temperature") if isinstance(light.get("color_temperature"), dict) else {}
        schema = ct.get("mirek_schema") if isinstance(ct.get("mirek_schema"), dict) else {}
        mirek_min = int(round(finite(schema.get("mirek_minimum"), 153.0, 153.0, 500.0)))
        mirek_max = int(round(finite(schema.get("mirek_maximum"), 500.0, 153.0, 500.0)))
        if mirek_min > mirek_max:
            mirek_min, mirek_max = 153, 500
        raw_mirek = ct.get("mirek")
        mirek = None
        if ct.get("mirek_valid", True) and raw_mirek is not None and not isinstance(raw_mirek, bool):
            m = finite(raw_mirek, None, 153.0, 500.0)
            if m is not None:
                mirek = int(round(m))
        effects_obj = light.get("effects") if isinstance(light.get("effects"), dict) else {}
        raw_effect = effects_obj.get("status")
        effect = raw_effect if isinstance(raw_effect, str) and EFFECT_RE.match(raw_effect) else "no_effect"
        avail_effects = []
        seen_eff = set()
        raw_vals = effects_obj.get("effect_values")
        if isinstance(raw_vals, list):
            for ev in raw_vals:
                if len(avail_effects) >= MAX_EFFECTS:
                    break
                if isinstance(ev, str) and EFFECT_RE.match(ev) and ev not in seen_eff:
                    seen_eff.add(ev)
                    avail_effects.append(ev)
        return {
            "id": light["id"],
            "name": clean((light.get("metadata") or {}).get("name") or "Light"),
            "on": bool((light.get("on") or {}).get("on")),
            "bri": round(brightness(dimming.get("brightness", 100.0)), 1),
            "dimmable": "dimming" in light,
            "color": light_color(light),
            "reachable": connectivity.get(owner, True),
            "supports_color": "color" in light,
            "supports_ct": "color_temperature" in light,
            "mirek": mirek,
            "mirek_min": mirek_min,
            "mirek_max": mirek_max,
            "effect": effect,
            "available_effects": avail_effects,
        }

    device_lights = {}   # each device's light services, once and capped
    for did, device in devices.items():
        found = {}
        for svc in device.get("services") or []:
            if isinstance(svc, dict) and svc.get("rtype") == "light" and svc.get("rid") in lights:
                found[svc["rid"]] = None
                if len(found) >= MAX_GROUP_LIGHTS:
                    break
        device_lights[did] = list(found)
    budget = [MAX_MEMBERSHIPS]

    def lights_of(group):
        ids = {}   # insertion-ordered set
        limit = min(MAX_GROUP_LIGHTS, budget[0])

        def add(rid):
            if rid in lights and rid not in ids and len(ids) < limit:
                ids[rid] = None

        for child in group.get("children") or []:
            if len(ids) >= limit:
                break
            if not isinstance(child, dict):
                continue
            rid, rtype = child.get("rid"), child.get("rtype")
            if rtype == "light":
                add(rid)
            elif rtype == "device":
                for lid in device_lights.get(rid, ()):
                    add(lid)
        budget[0] -= len(ids)
        return list(ids)

    scenes_by_group = {}
    seen_scenes = set()
    for scene in by_type.get("scene", []):
        if len(seen_scenes) >= MAX_SCENES:
            break
        gid = ((scene.get("group") if isinstance(scene.get("group"), dict) else None) or {}).get("rid")
        if not valid_uuid(scene.get("id")) or not valid_uuid(gid) or scene["id"] in seen_scenes:
            continue
        group_scenes = scenes_by_group.setdefault(gid, [])
        if len(group_scenes) >= MAX_GROUP_SCENES:
            continue
        seen_scenes.add(scene["id"])
        group_scenes.append({
            "id": scene["id"],
            "name": clean((scene.get("metadata") or {}).get("name") or "Scene", 30),
            "active": (scene.get("status") or {}).get("active", "inactive") != "inactive",
        })
    for group_scenes in scenes_by_group.values():
        group_scenes.sort(key=lambda s: s["name"].lower())

    rooms = []
    assigned = set()
    seen_groups = set()
    for kind in ("room", "zone"):
        for group in by_type.get(kind, []):
            if not valid_uuid(group.get("id")) or group["id"] in seen_groups:
                continue
            seen_groups.add(group["id"])
            gl_id = None
            for svc in group.get("services") or []:
                if svc.get("rtype") == "grouped_light" and svc.get("rid") in grouped:
                    gl_id = svc["rid"]
            ids = lights_of(group)
            if not ids:
                continue
            if kind == "room":
                assigned.update(ids)
            members = sorted((light_entry(lights[i]) for i in ids), key=lambda l: l["name"].lower())
            on_members = [l for l in members if l["on"]]
            gl = grouped.get(gl_id) or {}
            bri = (gl.get("dimming") or {}).get("brightness")
            bri = None if bri is None else brightness(bri)
            if bri is None:
                bri = sum(l["bri"] for l in on_members) / len(on_members) if on_members else 0
            rooms.append({
                "id": group["id"],
                "kind": kind,
                "name": clean((group.get("metadata") or {}).get("name") or kind.title()),
                "grouped_light": gl_id or "",
                "on": any(l["on"] for l in members),
                "bri": round(float(bri), 1),
                "on_count": len(on_members),
                "archetype": clean((group.get("metadata") or {}).get("archetype") or "", 30),
                "color": blend_colors([l["color"] for l in on_members]),
                "lights": members,
                "scenes": scenes_by_group.pop(group["id"], []),
            })
    loose = []
    for lid, light in lights.items():
        if len(loose) >= min(MAX_GROUP_LIGHTS, budget[0]):
            break
        if lid not in assigned:
            loose.append(light_entry(light))
    budget[0] -= len(loose)
    if loose:
        loose.sort(key=lambda l: l["name"].lower())
        on_members = [l for l in loose if l["on"]]
        rooms.append({
            "id": "", "kind": "other", "name": "Other lights", "grouped_light": "",
            "on": bool(on_members),
            "bri": round(sum(l["bri"] for l in on_members) / len(on_members), 1) if on_members else 0,
            "on_count": len(on_members), "archetype": "", "color": blend_colors([l["color"] for l in on_members]),
            "lights": loose, "scenes": [],
        })
    rooms.sort(key=lambda r: (r["kind"] != "room", r["name"].lower()))
    return rooms, len(lights), sum(1 for l in lights.values() if (l.get("on") or {}).get("on"))


def extract_bridge_home_gl(resources):
    """Extract the bridge_home grouped_light UUID from a CLIP v2 /resource list."""
    if not isinstance(resources, list):
        return ""
    for res in resources:
        if not isinstance(res, dict) or res.get("type") != "bridge_home":
            continue
        for svc in res.get("services") or []:
            if isinstance(svc, dict) and svc.get("rtype") == "grouped_light" and valid_uuid(svc.get("rid")):
                return svc["rid"]
    return ""


def _parse_hhmm(value):
    if not isinstance(value, str):
        return None
    m = TIME_HHMM_RE.match(value.strip())
    if not m:
        return None
    return int(m.group(1)), int(m.group(2))


def _normalize_days(raw_days):
    if not isinstance(raw_days, list):
        return []
    found = set()
    for d in raw_days:
        if not isinstance(d, str):
            continue
        low = d.strip().lower()
        if low in DAY_SHORT_TO_FULL:
            found.add(low)
        elif low in DAY_FULL_TO_SHORT:
            found.add(DAY_FULL_TO_SHORT[low])
    return [d for d in DAY_ORDER if d in found]


def _pick_schedule_script_id(resources):
    """Find the best behavior_script UUID for schedules, falling back to Hue's standard schedule script."""
    best = None
    best_rank = 99
    for res in resources if isinstance(resources, list) else []:
        if not isinstance(res, dict) or res.get("type") != "behavior_script":
            continue
        sid = res.get("id")
        if not valid_uuid(sid):
            continue
        if sid == DEFAULT_SCHEDULE_SCRIPT_ID:
            return sid
        meta = res.get("metadata") if isinstance(res.get("metadata"), dict) else {}
        cat = str(meta.get("category") or "").lower()
        name = str(meta.get("name") or "").lower()
        text = cat + " " + name
        if "schedule" in text:
            rank = 1
        elif any(k in text for k in ("timer", "wake_up", "wake up", "go_to_sleep", "go to sleep", "automation")):
            rank = 2
        else:
            continue
        if rank < best_rank:
            best_rank, best = rank, sid
    return best or DEFAULT_SCHEDULE_SCRIPT_ID


def parse_schedules(resources, rooms=None):
    """Parse CLIP v2 behavior_script + behavior_instance entries into (schedules, script_id)."""
    if not isinstance(resources, list):
        return [], DEFAULT_SCHEDULE_SCRIPT_ID
    script_id = _pick_schedule_script_id(resources)
    names = {}
    for res in resources:
        if not isinstance(res, dict) or not valid_uuid(res.get("id")):
            continue
        rtype = res.get("type")
        if rtype in ("light", "room", "zone"):
            names[(rtype, res["id"])] = clean((res.get("metadata") or {}).get("name") or "")
    if isinstance(rooms, list):
        for r in rooms:
            if isinstance(r, dict):
                if r.get("kind") in ("room", "zone") and valid_uuid(r.get("id")):
                    names.setdefault((r["kind"], r["id"]), clean(r.get("name") or ""))
                for l in r.get("lights") or []:
                    if isinstance(l, dict) and valid_uuid(l.get("id")):
                        names.setdefault(("light", l["id"]), clean(l.get("name") or ""))

    schedules = []
    seen = set()
    for res in resources:
        if len(schedules) >= MAX_SCHEDULES:
            break
        if not isinstance(res, dict) or res.get("type") != "behavior_instance":
            continue
        sid = res.get("id")
        if not valid_uuid(sid) or sid in seen:
            continue
        cfg = res.get("configuration") if isinstance(res.get("configuration"), dict) else {}
        # Trigger
        when = cfg.get("when") if isinstance(cfg.get("when"), dict) else {}
        when_ext = cfg.get("when_extended") if isinstance(cfg.get("when_extended"), dict) else {}
        start_at = when_ext.get("start_at") if isinstance(when_ext.get("start_at"), dict) else {}
        tp = when.get("time_point") if isinstance(when.get("time_point"), dict) else start_at.get("time_point")
        if not isinstance(tp, dict):
            continue
        tp_type = tp.get("type")
        if tp_type == "time":
            trigger_kind = "time"
            t = tp.get("time") if isinstance(tp.get("time"), dict) else {}
            hh = int(round(finite(t.get("hour", 0), 0.0, 0.0, 23.0)))
            mm = int(round(finite(t.get("minute", 0), 0.0, 0.0, 59.0)))
            time_str = "%02d:%02d" % (hh, mm)
            offset_min = 0
        elif tp_type in ("sunrise", "sunset"):
            trigger_kind = tp_type
            time_str = ""
            off = tp.get("offset") if isinstance(tp.get("offset"), dict) else {}
            offset_min = int(round(finite(off.get("minutes", 0), 0.0, -120.0, 120.0)))
        else:
            continue
        raw_days = when.get("recurrence_days") if isinstance(when.get("recurrence_days"), list) else when_ext.get("recurrence_days")
        days = _normalize_days(raw_days)

        # Target
        target_kind, target_id = "", ""
        where = cfg.get("where")
        if isinstance(where, list):
            for w in where:
                if not isinstance(w, dict):
                    continue
                grp = w.get("group")
                if isinstance(grp, dict) and grp.get("rtype") in ("light", "room", "zone") and valid_uuid(grp.get("rid")):
                    target_kind, target_id = grp["rtype"], grp["rid"]
                    break
                items = w.get("items")
                if isinstance(items, list):
                    for it in items:
                        if isinstance(it, dict) and it.get("rtype") in ("light", "room", "zone") and valid_uuid(it.get("rid")):
                            target_kind, target_id = it["rtype"], it["rid"]
                            break
                    if target_id:
                        break
                if w.get("rtype") in ("light", "room", "zone") and valid_uuid(w.get("rid")):
                    target_kind, target_id = w["rtype"], w["rid"]
                    break
        if not target_id and isinstance(cfg.get("group"), dict):
            grp = cfg["group"]
            if grp.get("rtype") in ("light", "room", "zone") and valid_uuid(grp.get("rid")):
                target_kind, target_id = grp["rtype"], grp["rid"]
        if not target_id:
            continue

        # Action & fade
        action, bri, scene_id = "on", None, ""
        fade_min = 0
        for fkey in ("fade_in_duration", "fade_out_duration", "duration"):
            fobj = cfg.get(fkey)
            if isinstance(fobj, dict):
                if "seconds" in fobj and not isinstance(fobj.get("seconds"), bool):
                    fade_min = int(round(finite(fobj.get("seconds"), 0.0, 0.0, 3600.0) / 60.0))
                    break
                if "minutes" in fobj and not isinstance(fobj.get("minutes"), bool):
                    fade_min = int(round(finite(fobj.get("minutes"), 0.0, 0.0, 60.0)))
                    break
        what = cfg.get("what")
        if isinstance(what, dict):
            what = [what]
        elif not isinstance(what, list):
            what = cfg.get("what_at_start")
            if isinstance(what, dict):
                what = [what]
            elif not isinstance(what, list):
                what = []
        matched_what = False
        for entry in what:
            if not isinstance(entry, dict):
                continue
            recall = entry.get("recall")
            if isinstance(recall, dict) and valid_uuid(recall.get("rid")):
                action, scene_id, matched_what = "scene", recall["rid"], True
                break
            on_obj = entry.get("on")
            dim_obj = entry.get("dimming")
            if isinstance(on_obj, dict) and isinstance(on_obj.get("on"), bool):
                action = "on" if on_obj["on"] else "off"
                matched_what = True
                if action == "on" and isinstance(dim_obj, dict) and dim_obj.get("brightness") is not None and not isinstance(dim_obj.get("brightness"), bool):
                    b = finite(dim_obj.get("brightness"), None, 1.0, 100.0)
                    if b is not None:
                        bri = int(round(b))
                break
            if isinstance(dim_obj, dict) and dim_obj.get("brightness") is not None and not isinstance(dim_obj.get("brightness"), bool):
                b = finite(dim_obj.get("brightness"), None, 1.0, 100.0)
                if b is not None:
                    action, bri, matched_what = "on", int(round(b)), True
                    break
        if not matched_what and cfg.get("end_state") == "turn_off":
            action = "off"

        seen.add(sid)
        meta = res.get("metadata") if isinstance(res.get("metadata"), dict) else {}
        schedules.append({
            "id": sid,
            "name": clean(meta.get("name") or "Schedule", 60) or "Schedule",
            "enabled": bool(res.get("enabled", False)),
            "target_kind": target_kind,
            "target_id": target_id,
            "target_name": names.get((target_kind, target_id), ""),
            "trigger_kind": trigger_kind,
            "time": time_str,
            "offset_min": offset_min,
            "days": days,
            "action": action,
            "bri": bri,
            "scene_id": scene_id,
            "fade_min": fade_min,
            "source": "bridge",
        })
    schedules.sort(key=lambda s: (s["name"].lower(), s["id"]))
    return schedules, script_id


def build_schedule_payload(cmd, script_id=DEFAULT_SCHEDULE_SCRIPT_ID):
    """Validate schedule_create inputs and build a CLIP v2 behavior_instance POST payload, or None."""
    if not isinstance(cmd, dict):
        return None
    if not valid_uuid(script_id):
        script_id = DEFAULT_SCHEDULE_SCRIPT_ID
    target_kind = cmd.get("target_kind")
    target_id = cmd.get("target_id")
    if target_kind not in ("light", "room", "zone") or not valid_uuid(target_id):
        return None
    trigger_kind = cmd.get("trigger_kind")
    if trigger_kind not in ("time", "sunrise", "sunset"):
        return None
    if trigger_kind == "time":
        hm = _parse_hhmm(cmd.get("time"))
        if hm is None:
            return None
        time_point = {"type": "time", "time": {"hour": hm[0], "minute": hm[1]}}
    else:
        raw_off = cmd.get("offset_min", 0)
        if raw_off is None:
            raw_off = 0
        if isinstance(raw_off, bool) or not isinstance(raw_off, (int, float)):
            return None
        off = finite(raw_off, None, -120.0, 120.0)
        if off is None:
            return None
        time_point = {"type": trigger_kind, "offset": {"minutes": int(round(off))}}

    raw_days = cmd.get("days", [])
    if raw_days is None:
        raw_days = []
    if not isinstance(raw_days, list):
        return None
    days = []
    seen_days = set()
    for d in raw_days:
        if not isinstance(d, str):
            return None
        low = d.strip().lower()
        if low in DAY_SHORT_TO_FULL:
            short = low
        elif low in DAY_FULL_TO_SHORT:
            short = DAY_FULL_TO_SHORT[low]
        else:
            return None
        if short not in seen_days:
            seen_days.add(short)
            days.append(short)
    days = [d for d in DAY_ORDER if d in seen_days]

    action = cmd.get("action")
    if action not in ("on", "off", "scene"):
        return None
    bri = None
    scene_id = ""
    if action == "scene":
        scene_id = cmd.get("scene_id")
        if not valid_uuid(scene_id):
            return None
        what = [{"recall": {"rid": scene_id, "rtype": "scene"}}]
    elif action == "off":
        what = [{"on": {"on": False}}]
    else:
        entry = {"on": {"on": True}}
        raw_bri = cmd.get("bri")
        if raw_bri is not None:
            if isinstance(raw_bri, bool) or not isinstance(raw_bri, (int, float)):
                return None
            b = finite(raw_bri, None, 1.0, 100.0)
            if b is None:
                return None
            bri = int(round(b))
            entry["dimming"] = {"brightness": bri}
        what = [entry]

    raw_fade = cmd.get("fade_min", 0)
    if raw_fade is None:
        raw_fade = 0
    if isinstance(raw_fade, bool) or not isinstance(raw_fade, (int, float)):
        return None
    fade = finite(raw_fade, None, 0.0, 60.0)
    if fade is None:
        return None
    fade_min = int(round(fade))

    enabled = cmd.get("enabled", True)
    if not isinstance(enabled, bool):
        return None
    name = clean(cmd.get("name") or "", 60) or "Schedule"

    when = {"time_point": time_point}
    if days:
        when["recurrence_days"] = [DAY_SHORT_TO_FULL[d] for d in days]
    if target_kind == "light":
        where = [{"items": [{"rid": target_id, "rtype": "light"}]}]
    else:
        where = [{"group": {"rid": target_id, "rtype": target_kind}}]
    config = {"where": where, "when": when, "what": what}
    if fade_min > 0:
        if action == "off":
            config["fade_out_duration"] = {"seconds": fade_min * 60}
        else:
            config["fade_in_duration"] = {"seconds": fade_min * 60}
    return {
        "type": "behavior_instance",
        "script_id": script_id,
        "enabled": enabled,
        "metadata": {"name": name},
        "configuration": config,
    }


def parse_sse_events(payload):
    """Parse one CLIP v2 SSE JSON array payload into [(kind, rid, changes), ...]."""
    if not payload or len(payload) > MAX_BODY:
        return []
    try:
        events = json.loads(payload)
    except (ValueError, TypeError, RecursionError):
        return []
    if not isinstance(events, list):
        return []
    updates = []
    for ev in events:
        if not isinstance(ev, dict) or ev.get("type") != "update":
            continue
        items = ev.get("data")
        if not isinstance(items, list):
            continue
        for item in items:
            if not isinstance(item, dict):
                continue
            kind = item.get("type")
            rid = item.get("id")
            if kind not in ("light", "grouped_light") or not valid_uuid(rid):
                continue
            changes = {}
            on_obj = item.get("on")
            if isinstance(on_obj, dict) and isinstance(on_obj.get("on"), bool):
                changes["on"] = on_obj["on"]
            dim_obj = item.get("dimming")
            if isinstance(dim_obj, dict) and dim_obj.get("brightness") is not None and not isinstance(dim_obj.get("brightness"), bool):
                b = finite(dim_obj.get("brightness"), None, 0.0, 100.0)
                if b is not None:
                    changes["bri"] = round(b, 1)
            ct_obj = item.get("color_temperature")
            if isinstance(ct_obj, dict):
                if ct_obj.get("mirek_valid") is False:
                    changes["mirek"] = None
                elif ct_obj.get("mirek") is not None and not isinstance(ct_obj.get("mirek"), bool):
                    m = finite(ct_obj.get("mirek"), None, 153.0, 500.0)
                    if m is not None:
                        changes["mirek"] = int(round(m))
            col_obj = item.get("color")
            if isinstance(col_obj, dict) and isinstance(col_obj.get("xy"), dict):
                xy = col_obj["xy"]
                rx, ry = xy.get("x"), xy.get("y")
                if not isinstance(rx, bool) and not isinstance(ry, bool):
                    x, y = finite(rx, None, 0.0, 1.0), finite(ry, None, 0.0, 1.0)
                    if x is not None and y is not None and y > 0:
                        changes["xy"] = (round(x, 4), round(y, 4))
            eff_obj = item.get("effects")
            if isinstance(eff_obj, dict):
                st = eff_obj.get("status")
                if isinstance(st, str) and EFFECT_RE.match(st):
                    changes["effect"] = st
            if changes:
                updates.append((kind, rid, changes))
    return updates


def parse_sse_data_line(line):
    """Parse one raw SSE line; returns list of updates when line starts with `data:`, else []."""
    if isinstance(line, bytes):
        if len(line) > MAX_BODY:
            return []
        line = line.decode("utf-8", "replace")
    if not isinstance(line, str) or len(line) > MAX_BODY:
        return []
    line = line.rstrip("\r\n")
    if not line.startswith("data:"):
        return []
    return parse_sse_events(line[5:].lstrip())


def validate_rule(raw):
    """Validate and normalize a Dependency / Linked-Light Rule dict, or return None."""
    if not isinstance(raw, dict):
        return None
    rid = raw.get("id")
    if rid is None or rid == "":
        rid = str(uuid.uuid4())
    elif not valid_uuid(rid):
        return None
    name = clean(raw.get("name") or "", 60) or "Rule"
    enabled = raw.get("enabled", True)
    if not isinstance(enabled, bool):
        return None
    kind = raw.get("kind")
    if kind not in RULE_KINDS:
        return None
    trigger_kind = raw.get("trigger_kind")
    trigger_id = raw.get("trigger_id")
    trigger_state = raw.get("trigger_state", "")
    if trigger_kind == "desktop":
        if trigger_id not in ("wake", "sleep"):
            return None
        if trigger_state not in ("", "on", "off"):
            return None
        trigger_state = ""
    elif trigger_kind in ("light", "room"):
        if not valid_uuid(trigger_id):
            return None
        if trigger_state not in ("on", "off"):
            return None
    else:
        return None

    raw_targets = raw.get("targets")
    if not isinstance(raw_targets, list) or not (1 <= len(raw_targets) <= MAX_RULE_TARGETS):
        return None
    targets = []
    seen_targets = set()
    for t in raw_targets:
        if not isinstance(t, dict):
            return None
        tkind = t.get("kind")
        tid = t.get("id")
        action = t.get("action")
        if tkind not in ("light", "room") or not valid_uuid(tid) or action not in ("on", "off"):
            return None
        if tkind == trigger_kind and tid == trigger_id:
            return None
        if (tkind, tid) in seen_targets:
            continue
        seen_targets.add((tkind, tid))
        raw_bri = t.get("bri")
        bri = None
        if raw_bri is not None:
            if isinstance(raw_bri, bool) or not isinstance(raw_bri, (int, float)):
                return None
            b = finite(raw_bri, None, 1.0, 100.0)
            if b is None:
                return None
            bri = int(round(b))
        scene_id = t.get("scene_id", "")
        if scene_id is None:
            scene_id = ""
        if not isinstance(scene_id, str) or (scene_id and not valid_uuid(scene_id)):
            return None
        targets.append({"kind": tkind, "id": tid, "action": action, "bri": bri, "scene_id": scene_id})
    if not targets:
        return None

    mirror_off = raw.get("mirror_off", False)
    match_bri = raw.get("match_bri", False)
    only_if_off = raw.get("only_if_off", False)
    if not (isinstance(mirror_off, bool) and isinstance(match_bri, bool) and isinstance(only_if_off, bool)):
        return None

    time_start = raw.get("time_start", "")
    time_end = raw.get("time_end", "")
    if time_start is None:
        time_start = ""
    if time_end is None:
        time_end = ""
    if not isinstance(time_start, str) or not isinstance(time_end, str):
        return None
    time_start, time_end = time_start.strip(), time_end.strip()
    if bool(time_start) != bool(time_end):
        return None
    if time_start:
        if _parse_hhmm(time_start) is None or _parse_hhmm(time_end) is None:
            return None

    return {
        "id": rid,
        "name": name,
        "enabled": enabled,
        "kind": kind,
        "trigger_kind": trigger_kind,
        "trigger_id": trigger_id,
        "trigger_state": trigger_state,
        "targets": targets,
        "mirror_off": mirror_off,
        "match_bri": match_bri,
        "time_start": time_start,
        "time_end": time_end,
        "only_if_off": only_if_off,
    }


def detect_rule_cycles(rules):
    """Check enabled rules for contradictory cycles or chains deeper than MAX_CASCADE_DEPTH (3).

    Returns an error reason string if invalid, or '' if safe.
    Safe symmetric pairs (A ON -> B ON and B ON -> A ON) are allowed because echo suppression
    prevents infinite loops.
    """
    adj = {}  # (kind, id, state) -> set of (kind, id, state)
    for r in rules if isinstance(rules, list) else []:
        if not isinstance(r, dict) or not r.get("enabled", True):
            continue
        tk = r.get("trigger_kind")
        tid = r.get("trigger_id")
        ts = r.get("trigger_state")
        if tk not in ("light", "room") or ts not in ("on", "off"):
            continue
        src = (tk, tid, ts)
        for t in r.get("targets") or []:
            dst = (t["kind"], t["id"], t["action"])
            adj.setdefault(src, set()).add(dst)
            if r.get("mirror_off") and ts == "on":
                adj.setdefault((tk, tid, "off"), set()).add((t["kind"], t["id"], "off"))

    def dfs(node, res_states, depth):
        for nxt in adj.get(node, ()):
            nxt_res = (nxt[0], nxt[1])
            nxt_state = nxt[2]
            if nxt_res in res_states:
                if res_states[nxt_res] != nxt_state:
                    return "contradictory rule cycle"
                # Consistent cycle (e.g. A ON -> B ON -> A ON): allowed via echo suppression
                continue
            if depth + 1 > MAX_CASCADE_DEPTH:
                return "rule chain depth exceeds %d" % MAX_CASCADE_DEPTH
            res_states[nxt_res] = nxt_state
            err = dfs(nxt, res_states, depth + 1)
            del res_states[nxt_res]
            if err:
                return err
        return ""

    for start in list(adj):
        err = dfs(start, {(start[0], start[1]): start[2]}, 0)
        if err:
            return err
    return ""


def in_time_window(time_start, time_end, now=None):
    """True when `now` (tuple (hour, minute) or None for localtime) falls in [time_start, time_end)."""
    if not time_start or not time_end:
        return True
    s = _parse_hhmm(time_start)
    e = _parse_hhmm(time_end)
    if s is None or e is None:
        return False
    if now is None:
        lt = time.localtime()
        cur = lt.tm_hour * 60 + lt.tm_min
    else:
        cur = int(now[0]) * 60 + int(now[1])
    s_min = s[0] * 60 + s[1]
    e_min = e[0] * 60 + e[1]
    if s_min == e_min:
        return True
    if s_min < e_min:
        return s_min <= cur < e_min
    return cur >= s_min or cur < e_min


def _echo_entry(entry):
    """Decode echo_suppress entry (expires, expected_on) or legacy bare float timestamp."""
    if isinstance(entry, tuple) and len(entry) == 2:
        return float(entry[0]), entry[1]
    return float(entry), None


def validate_theme(raw):
    """Validate and normalize a custom per-lamp light theme dict, or return None."""
    if not isinstance(raw, dict):
        return None
    tid = raw.get("id")
    if tid is None or tid == "":
        tid = str(uuid.uuid4())
    elif not isinstance(tid, str) or THEME_ID_RE.match(tid) is None or tid in BUILTIN_THEME_IDS:
        return None
    name = clean(raw.get("name") or "", 60) or "Theme"
    raw_mirek = raw.get("mirek")
    mirek = None
    if raw_mirek is not None:
        if isinstance(raw_mirek, bool) or not isinstance(raw_mirek, (int, float)):
            return None
        m = finite(raw_mirek, None, 153.0, 500.0)
        if m is None:
            return None
        mirek = int(round(m))
    raw_hex = raw.get("hex")
    if raw_hex is None or raw_hex == "":
        if mirek is None:
            return None
        hex_val = mirek_to_hex(mirek)
    elif isinstance(raw_hex, str) and HEX_COLOR_RE.fullmatch(raw_hex.strip()):
        s = raw_hex.strip()
        hex_val = (s if s.startswith("#") else "#" + s).lower()
    else:
        return None
    raw_bri = raw.get("bri", 80)
    if isinstance(raw_bri, bool) or not isinstance(raw_bri, (int, float)):
        return None
    b = finite(raw_bri, None, 1.0, 100.0)
    if b is None:
        return None
    bri = int(round(b))
    raw_eff = raw.get("effect", "no_effect")
    if raw_eff is None or raw_eff == "":
        effect = "no_effect"
    elif isinstance(raw_eff, str) and EFFECT_RE.match(raw_eff):
        effect = raw_eff
    else:
        return None
    return {
        "id": tid,
        "name": name,
        "hex": hex_val,
        "mirek": mirek,
        "bri": bri,
        "effect": effect,
        "built_in": False,
    }


def validate_local_schedule(raw):
    """Validate and normalize a local schedule dict, or return None."""
    if not isinstance(raw, dict):
        return None
    sid = raw.get("id")
    if sid is None or sid == "":
        sid = str(uuid.uuid4())
    elif not valid_uuid(sid):
        return None
    name = clean(raw.get("name") or "", 60) or "Schedule"
    enabled = raw.get("enabled", True)
    if not isinstance(enabled, bool):
        return None
    target_kind = raw.get("target_kind")
    target_id = raw.get("target_id")
    if target_kind not in ("light", "room", "zone") or not valid_uuid(target_id):
        return None
    target_name = clean(raw.get("target_name") or "", 60)
    trigger_kind = raw.get("trigger_kind", "time")
    if trigger_kind != "time":
        return None
    hm = _parse_hhmm(raw.get("time"))
    if hm is None:
        return None
    raw_days = raw.get("days", [])
    if raw_days is None:
        raw_days = []
    if not isinstance(raw_days, list):
        return None
    seen_days = set()
    for d in raw_days:
        if not isinstance(d, str):
            return None
        low = d.strip().lower()
        if low in DAY_SHORT_TO_FULL:
            seen_days.add(low)
        elif low in DAY_FULL_TO_SHORT:
            seen_days.add(DAY_FULL_TO_SHORT[low])
        else:
            return None
    days = [d for d in DAY_ORDER if d in seen_days]
    action = raw.get("action")
    if action not in ("on", "off", "scene"):
        return None
    bri = None
    scene_id = ""
    if action == "scene":
        scene_id = raw.get("scene_id")
        if not valid_uuid(scene_id):
            return None
    elif action == "on":
        raw_bri = raw.get("bri")
        if raw_bri is not None:
            if isinstance(raw_bri, bool) or not isinstance(raw_bri, (int, float)):
                return None
            b = finite(raw_bri, None, 1.0, 100.0)
            if b is None:
                return None
            bri = int(round(b))
    raw_fade = raw.get("fade_min", 0)
    if raw_fade is None:
        raw_fade = 0
    if isinstance(raw_fade, bool) or not isinstance(raw_fade, (int, float)):
        return None
    fade = finite(raw_fade, None, 0.0, 60.0)
    if fade is None:
        return None
    last_fired = raw.get("last_fired")
    if not isinstance(last_fired, str) or len(last_fired) > 16:
        last_fired = ""
    out = {
        "id": sid,
        "name": name,
        "enabled": enabled,
        "target_kind": target_kind,
        "target_id": target_id,
        "target_name": target_name,
        "trigger_kind": "time",
        "time": "%02d:%02d" % hm,
        "offset_min": 0,
        "days": days,
        "action": action,
        "bri": bri,
        "scene_id": scene_id,
        "fade_min": int(round(fade)),
        "source": "local",
    }
    if last_fired:
        out["last_fired"] = last_fired
    return out


_HANN_CACHE = {}


def _hann_window(length):
    win = _HANN_CACHE.get(length)
    if win is None:
        denom = float(length - 1) if length > 1 else 1.0
        win = tuple(0.5 * (1.0 - math.cos(2.0 * math.pi * i / denom)) for i in range(length))
        if len(_HANN_CACHE) < 32:
            _HANN_CACHE[length] = win
    return win


def _goertzel_mag(samples, freq, sample_rate, block_len=None):
    """Hann-windowed Goertzel RMS magnitude normalized to ~1.0 for a unit-amplitude sine."""
    n = len(samples)
    if n == 0 or sample_rate <= 0 or freq <= 0 or freq >= sample_rate * 0.5:
        return 0.0
    m = n if block_len is None else min(n, max(8, int(block_len)))
    if m < 4:
        return 0.0
    omega = 2.0 * math.pi * freq / sample_rate
    coeff = 2.0 * math.cos(omega)
    win = _hann_window(m)
    norm = m * 0.25
    norm_sq = norm * norm
    acc = 0.0
    blocks = 0
    for start in range(0, n - m + 1, m):
        s1 = 0.0
        s2 = 0.0
        for i in range(m):
            s0 = samples[start + i] * win[i] + coeff * s1 - s2
            s2 = s1
            s1 = s0
        power = s1 * s1 + s2 * s2 - coeff * s1 * s2
        if power > 0.0:
            acc += power / norm_sq
        blocks += 1
    return math.sqrt(acc / blocks) if blocks else 0.0


def analyze_pcm_bands(samples, sample_rate=16000):
    """Pure-Python constant-Q Goertzel energy across Bass, Mids, and Treble -> (bass, mids, treble) in 0..1."""
    if not samples or sample_rate <= 0:
        return (0.0, 0.0, 0.0)
    if isinstance(samples, (bytes, bytearray, memoryview)):
        count = len(samples) // 2
        if count == 0:
            return (0.0, 0.0, 0.0)
        raw = struct.unpack("<%dh" % count, samples[:count * 2])
        norm = [v / 32768.0 for v in raw]
    else:
        norm = [float(v) for v in samples]
        if not norm:
            return (0.0, 0.0, 0.0)
        peak = max(abs(v) for v in norm)
        if peak > 1.5:
            norm = [v / 32768.0 for v in norm]

    bass_block = max(64, int(round(sample_rate * 0.032)))   # 512 @ 16 kHz -> covers 30..200 Hz
    mids_block = max(32, int(round(sample_rate * 0.006)))   #  96 @ 16 kHz -> covers 250..1800 Hz
    treble_block = max(8, int(round(sample_rate * 0.001)))  #  16 @ 16 kHz -> covers 2000..8000 Hz

    def band_energy(freqs, block_len):
        total_sq = 0.0
        for f in freqs:
            m = _goertzel_mag(norm, f, sample_rate, block_len=block_len)
            total_sq += m * m
        return round(clamp(math.sqrt(total_sq), 0.0, 1.0), 4)

    return (
        band_energy(SYNTH_BASS_FREQS, bass_block),
        band_energy(SYNTH_MIDS_FREQS, mids_block),
        band_energy(SYNTH_TREBLE_FREQS, treble_block),
    )


class AudioBeatTracker:
    """Rolling energy history + beat detector + exponential band smoother."""

    def __init__(self, history_size=24, beat_ratio=1.35, beat_floor=0.12, min_beat_gap=0.22):
        self.history = []
        self.history_size = max(4, int(history_size))
        self.beat_ratio = float(beat_ratio)
        self.beat_floor = float(beat_floor)
        self.min_beat_gap = float(min_beat_gap)
        self.bass = 0.0
        self.mids = 0.0
        self.treble = 0.0
        self.last_beat = -1e9

    def reset(self):
        self.history.clear()
        self.bass = 0.0
        self.mids = 0.0
        self.treble = 0.0
        self.last_beat = -1e9

    def update(self, bass, mids, treble, sensitivity=1.0, smoothness=0.45, now_mono=None):
        sens = finite(sensitivity, 1.0, 0.2, 3.0)
        smooth = finite(smoothness, 0.45, 0.0, 0.95)
        now_mono = time.monotonic() if now_mono is None else float(now_mono)
        b_in = clamp(finite(bass, 0.0, 0.0, 1.0) * sens, 0.0, 1.0)
        m_in = clamp(finite(mids, 0.0, 0.0, 1.0) * sens, 0.0, 1.0)
        t_in = clamp(finite(treble, 0.0, 0.0, 1.0) * sens, 0.0, 1.0)
        energy = b_in * 0.65 + m_in * 0.25 + t_in * 0.10
        avg = (sum(self.history) / len(self.history)) if self.history else energy
        is_beat = (
            len(self.history) >= 4
            and energy >= self.beat_floor
            and energy > avg * self.beat_ratio
            and (now_mono - self.last_beat) >= self.min_beat_gap
        )
        if is_beat:
            self.last_beat = now_mono
        self.history.append(energy)
        if len(self.history) > self.history_size:
            self.history.pop(0)
        alpha = 1.0 - smooth
        self.bass = self.bass * smooth + b_in * alpha
        self.mids = self.mids * smooth + m_in * alpha
        self.treble = self.treble * smooth + t_in * alpha
        return (
            round(clamp(self.bass, 0.0, 1.0), 4),
            round(clamp(self.mids, 0.0, 1.0), 4),
            round(clamp(self.treble, 0.0, 1.0), 4),
            bool(is_beat),
        )


def demo_beat_levels(elapsed_sec, bpm=120.0):
    """Synthesize musical (bass, mids, treble, is_beat) levels with zero audio hardware."""
    t = max(0.0, finite(elapsed_sec, 0.0, 0.0, 1e9))
    bpm = finite(bpm, 120.0, 40.0, 240.0)
    beat_period = 60.0 / bpm
    beat_pos = (t % beat_period) / beat_period
    bar_pos = (t % (beat_period * 4.0)) / (beat_period * 4.0)
    bass = clamp(math.exp(-beat_pos * 5.5) * 0.92 + 0.08, 0.0, 1.0)
    snare_pos = ((bar_pos * 4.0) - 1.0) % 2.0
    snare = math.exp(-snare_pos * 6.0) * 0.45
    mids = clamp(0.32 + 0.28 * math.sin(bar_pos * 2.0 * math.pi) + snare, 0.0, 1.0)
    hat_pos = (beat_pos * 2.0) % 1.0
    treble = clamp(math.exp(-hat_pos * 6.5) * 0.65 + 0.12 + 0.15 * abs(math.sin(t * 3.1)), 0.0, 1.0)
    is_beat = beat_pos < 0.18
    return (round(bass, 4), round(mids, 4), round(treble, 4), is_beat)


def _lerp_hex(c1, c2, t):
    t = clamp(float(t), 0.0, 1.0)
    r1, g1, b1 = int(c1[1:3], 16), int(c1[3:5], 16), int(c1[5:7], 16)
    r2, g2, b2 = int(c2[1:3], 16), int(c2[3:5], 16), int(c2[5:7], 16)
    r = int(round(r1 + (r2 - r1) * t))
    g = int(round(g1 + (g2 - g1) * t))
    b = int(round(b1 + (b2 - b1) * t))
    return "#%02x%02x%02x" % (clamp(r, 0, 255), clamp(g, 0, 255), clamp(b, 0, 255))


def map_synth_frame(light_index, levels, mode, palette, band, sensitivity, smoothness, phase):
    """Map audio band levels + light index into {bri: 1..100, xy: [x, y], hex: '#rrggbb'}."""
    if isinstance(levels, dict):
        bass = finite(levels.get("bass"), 0.0, 0.0, 1.0)
        mids = finite(levels.get("mids"), 0.0, 0.0, 1.0)
        treble = finite(levels.get("treble"), 0.0, 0.0, 1.0)
        is_beat = bool(levels.get("beat", False))
    elif isinstance(levels, (list, tuple)) and len(levels) >= 3:
        bass = finite(levels[0], 0.0, 0.0, 1.0)
        mids = finite(levels[1], 0.0, 0.0, 1.0)
        treble = finite(levels[2], 0.0, 0.0, 1.0)
        is_beat = bool(levels[3]) if len(levels) >= 4 else False
    else:
        bass = mids = treble = 0.0
        is_beat = False

    idx = max(0, int(light_index)) if isinstance(light_index, (int, float)) else 0
    mode = mode if mode in SYNTH_MODES else "club"
    colors = SYNTH_PALETTES.get(palette, SYNTH_PALETTES["neon"])
    band = band if band in SYNTH_BANDS else "all"
    sens = finite(sensitivity, 1.0, 0.2, 3.0)
    smooth = finite(smoothness, 0.45, 0.0, 0.95)
    ph = finite(phase, 0.0, -1e9, 1e9)

    if band == "bass":
        energy = bass
    elif band == "mids":
        energy = mids
    elif band == "treble":
        energy = treble
    else:
        energy = 0.50 * bass + 0.30 * mids + 0.20 * treble
    level = clamp(energy * sens, 0.0, 1.0)

    if mode == "club":
        raw_bri = 18.0 + level * 68.0 + (22.0 if is_beat else 0.0)
        mid_bri = 55.0
        pos = ph * 0.75 + idx * 0.85 + (0.65 if is_beat else 0.0) + treble * 0.4
    elif mode == "chill":
        raw_bri = 25.0 + level * 52.0 + (8.0 if is_beat else 0.0)
        mid_bri = 52.0
        pos = ph * 0.35 + idx * 0.40 + mids * 0.35
    elif mode == "ambient":
        raw_bri = 30.0 + level * 42.0 + (4.0 if is_beat else 0.0)
        mid_bri = 50.0
        pos = ph * 0.18 + idx * 0.28
    else:  # strobe
        raw_bri = (96.0 + level * 4.0) if (is_beat or level > 0.65) else (8.0 + level * 22.0)
        mid_bri = 50.0
        pos = ph * 1.4 + idx * 1.1 + (1.5 if is_beat else 0.0)

    bri_f = raw_bri * (1.0 - smooth * 0.35) + mid_bri * (smooth * 0.35)
    bri = int(round(clamp(bri_f, 1.0, 100.0)))

    n_col = len(colors)
    wrapped = pos % n_col
    i0 = int(wrapped) % n_col
    i1 = (i0 + 1) % n_col
    frac = wrapped - int(wrapped)
    hex_color = _lerp_hex(colors[i0], colors[i1], frac)
    xy = hex_to_xy(hex_color) or [0.3127, 0.329]
    return {"bri": bri, "xy": xy, "hex": hex_color}


def parse_avahi_hue(lines):
    """Parse `avahi-browse -rtpk _hue._tcp` output into [{ip, bridge_id, name}]."""
    found = {}
    for line in lines:
        parts = line.split(";")
        if len(parts) < 10 or parts[0] != "=" or parts[2] != "IPv4":
            continue
        ip = parts[7]
        if not valid_bridge_ip(ip):
            continue
        bridge_id = ""
        m = re.search(r'"bridgeid=([0-9a-fA-F]{16})"', ";".join(parts[9:]))
        if m:
            bridge_id = m.group(1).upper()
        name = parts[3].replace("\\032", " ")
        name = re.sub(r"\\(\d{3})", lambda mm: chr(int(mm.group(1))), name)
        found[ip] = {"ip": ip, "bridge_id": bridge_id, "name": clean(name)}
    return sorted(found.values(), key=lambda b: b["ip"])


def parse_command(line):
    try:
        cmd = json.loads(line)
    except (ValueError, TypeError, RecursionError):
        return None
    return cmd if isinstance(cmd, dict) and isinstance(cmd.get("cmd"), str) else None


def read_commands(stream):
    """Commands from a binary stream, one JSON object per line. A line longer than MAX_COMMAND
    is skipped through its newline without being kept in memory."""
    while True:
        line = stream.readline(MAX_COMMAND + 1)
        if not line:
            return
        if len(line) > MAX_COMMAND:
            while not line.endswith(b"\n"):
                line = stream.readline(MAX_COMMAND + 1)
                if not line:
                    return
            continue
        cmd = parse_command(line)
        if cmd:
            yield cmd


# ---------------------------------------------------------------------------
# Config

def private_config_dir():
    """Create the config directory as 0700 and refuse it unless it is our own real directory."""
    os.makedirs(CONFIG_DIR, mode=0o700, exist_ok=True)
    info = os.lstat(CONFIG_DIR)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        raise OSError("config directory is not a private directory owned by this user")
    if info.st_mode & 0o077:
        os.chmod(CONFIG_DIR, 0o700)
    return CONFIG_DIR


def load_config():
    try:
        fd = os.open(CONFIG_PATH, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)  # a FIFO must not block
    except OSError:
        return {}
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_size > MAX_CONFIG:
            return {}
        raw = os.read(fd, MAX_CONFIG)
    except OSError:
        return {}
    finally:
        os.close(fd)
    try:
        data = json.loads(raw.decode("utf-8"))
    except ValueError:
        return {}
    if not isinstance(data, dict):
        return {}
    if not (valid_bridge_ip(data.get("ip")) and isinstance(data.get("app_key"), str)
            and re.fullmatch(r"[A-Za-z0-9_-]{1,100}", data["app_key"])
            and re.fullmatch(r"[0-9a-f]{64}", str(data.get("fingerprint", "")))
            and BRIDGE_ID_RE.match(str(data.get("bridge_id", "")))):
        return {}
    return data


def save_config(data):
    """Atomic 0600 write via a unique temp file (never a predictable name, never a followed link)."""
    directory = private_config_dir()
    fd, tmp = tempfile.mkstemp(dir=directory, prefix=".config.", suffix=".tmp")
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(data, fh, indent=2)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, CONFIG_PATH)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def remove_config():
    """Delete the saved pairing. True when no pairing file is left (a missing one counts),
    False when one is still there (not ours, or the folder is read-only)."""
    try:
        info = os.lstat(CONFIG_PATH)
    except FileNotFoundError:
        return True
    except OSError:
        return False
    if not ((stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode)) and info.st_uid == os.getuid()):
        return False
    try:
        os.unlink(CONFIG_PATH)
    except FileNotFoundError:
        return True
    except OSError:
        return False
    return True


def _rules_path():
    return os.path.join(CONFIG_DIR, "rules.json")


def load_rules():
    """Load persisted dependency rules from CONFIG_DIR/rules.json (rejecting symlinks, FIFOs, >64 KiB)."""
    default = {"paused": False, "rules": []}
    path = _rules_path()
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError:
        return default
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_size > MAX_CONFIG:
            return default
        raw = os.read(fd, MAX_CONFIG)
    except OSError:
        return default
    finally:
        os.close(fd)
    try:
        data = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError, RecursionError):
        return default
    if not isinstance(data, dict):
        return default
    paused = bool(data.get("paused", False))
    raw_rules = data.get("rules")
    if not isinstance(raw_rules, list):
        return {"paused": paused, "rules": []}
    rules = []
    seen = set()
    for item in raw_rules:
        if len(rules) >= MAX_RULES:
            break
        validated = validate_rule(item)
        if validated and validated["id"] not in seen:
            seen.add(validated["id"])
            rules.append(validated)
    if detect_rule_cycles(rules):
        return {"paused": paused, "rules": []}
    return {"paused": paused, "rules": rules}


def save_rules(data):
    """Atomic 0600 write of rules.json via unique temp file, capped at MAX_CONFIG (64 KiB)."""
    payload = json.dumps(data, indent=2, allow_nan=False).encode("utf-8")
    if len(payload) > MAX_CONFIG:
        raise ValueError("rules file exceeds maximum size")
    directory = private_config_dir()
    path = _rules_path()
    fd, tmp = tempfile.mkstemp(dir=directory, prefix=".rules.", suffix=".tmp")
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb") as fh:
            fh.write(payload)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def _themes_path():
    return os.path.join(CONFIG_DIR, "themes.json")


def load_themes():
    """Load persisted custom themes from CONFIG_DIR/themes.json (rejecting symlinks, FIFOs, >64 KiB)."""
    path = _themes_path()
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError:
        return []
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_size > MAX_CONFIG:
            return []
        raw = os.read(fd, MAX_CONFIG)
    except OSError:
        return []
    finally:
        os.close(fd)
    try:
        data = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError, RecursionError):
        return []
    raw_items = data.get("themes") if isinstance(data, dict) else data
    if not isinstance(raw_items, list):
        return []
    themes = []
    seen = set()
    for item in raw_items:
        if len(themes) >= MAX_THEMES:
            break
        validated = validate_theme(item)
        if validated and validated["id"] not in seen:
            seen.add(validated["id"])
            themes.append(validated)
    return themes


def save_themes(items):
    """Atomic 0600 write of themes.json via unique temp file, capped at MAX_CONFIG (64 KiB)."""
    payload = json.dumps(items, indent=2, allow_nan=False).encode("utf-8")
    if len(payload) > MAX_CONFIG:
        raise ValueError("themes file exceeds maximum size")
    directory = private_config_dir()
    path = _themes_path()
    fd, tmp = tempfile.mkstemp(dir=directory, prefix=".themes.", suffix=".tmp")
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb") as fh:
            fh.write(payload)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def _local_schedules_path():
    return os.path.join(CONFIG_DIR, "local_schedules.json")


def load_local_schedules():
    """Load persisted schedule mode & local schedules from CONFIG_DIR/local_schedules.json."""
    default = {"mode": "bridge", "schedules": []}
    path = _local_schedules_path()
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError:
        return default
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_size > MAX_CONFIG:
            return default
        raw = os.read(fd, MAX_CONFIG)
    except OSError:
        return default
    finally:
        os.close(fd)
    try:
        data = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError, RecursionError):
        return default
    if not isinstance(data, dict):
        return default
    mode = data.get("mode") if data.get("mode") in SCHEDULE_MODES else "bridge"
    raw_scheds = data.get("schedules")
    if not isinstance(raw_scheds, list):
        return {"mode": mode, "schedules": []}
    schedules = []
    seen = set()
    for item in raw_scheds:
        if len(schedules) >= MAX_LOCAL_SCHEDULES:
            break
        validated = validate_local_schedule(item)
        if validated and validated["id"] not in seen:
            seen.add(validated["id"])
            schedules.append(validated)
    return {"mode": mode, "schedules": schedules}


def save_local_schedules(data):
    """Atomic 0600 write of local_schedules.json via unique temp file, capped at MAX_CONFIG (64 KiB)."""
    payload = json.dumps(data, indent=2, allow_nan=False).encode("utf-8")
    if len(payload) > MAX_CONFIG:
        raise ValueError("local schedules file exceeds maximum size")
    directory = private_config_dir()
    path = _local_schedules_path()
    fd, tmp = tempfile.mkstemp(dir=directory, prefix=".local_schedules.", suffix=".tmp")
    try:
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb") as fh:
            fh.write(payload)
            fh.flush()
            os.fsync(fh.fileno())
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def claim_local_schedule_bucket(sid, bucket):
    """Cross-process deduplication for local schedules using an exclusive flock + persisted last_fired.
    Returns True if this process claimed `(sid, bucket)` (or when no local_schedules.json exists yet),
    False if another helper instance or prior run already fired this schedule in `bucket`."""
    path = _local_schedules_path()
    if not os.path.exists(path):
        return True
    try:
        directory = private_config_dir()
        lock_path = os.path.join(directory, ".local_schedules.lock")
        fd = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    except OSError:
        return True
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        data = load_local_schedules()
        for sched in data["schedules"]:
            if sched["id"] == sid:
                if sched.get("last_fired") == bucket:
                    return False
                sched["last_fired"] = bucket
                try:
                    save_local_schedules(data)
                except (OSError, ValueError):
                    pass
                return True
        return True
    finally:
        try:
            fcntl.flock(fd, fcntl.LOCK_UN)
        except OSError:
            pass
        os.close(fd)


# Children the helper started and has not reaped yet: ended when the helper exits or gets SIGTERM.
_CHILDREN = set()
_CHILDREN_LOCK = threading.Lock()
_SETPRIV = None   # path of util-linux setpriv, "" when it is not installed; looked up once


def kill_children():
    with _CHILDREN_LOCK:
        children = list(_CHILDREN)
    for proc in children:
        try:
            if proc.poll() is None:
                proc.kill()
        except OSError:
            pass


def _parent_death_prefix():
    """`setpriv --pdeathsig KILL --` so a child ends even when the helper is SIGKILLed. No Python
    preexec_fn: running Python between fork and exec in this threaded process can deadlock."""
    global _SETPRIV
    if _SETPRIV is None:
        _SETPRIV = shutil.which("setpriv") or ""
    return [_SETPRIV, "--pdeathsig", "KILL", "--"] if _SETPRIV else []


def run_bounded(argv, timeout=6.0, max_bytes=262144):
    """Run a fixed argv (no shell) and read at most `max_bytes` of stdout before `timeout`."""
    try:
        with _CHILDREN_LOCK:
            proc = subprocess.Popen(_parent_death_prefix() + list(argv), stdin=subprocess.DEVNULL,
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            _CHILDREN.add(proc)
    except OSError:
        return ""
    chunks, size = [], 0
    deadline = time.monotonic() + timeout
    fd = proc.stdout.fileno()
    try:
        while size < max_bytes:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            ready, _, _ = select.select([fd], [], [], remaining)
            if not ready:
                break
            chunk = os.read(fd, min(65536, max_bytes - size))
            if not chunk:
                break
            chunks.append(chunk)
            size += len(chunk)
    except OSError:
        pass
    finally:
        proc.stdout.close()
        if proc.poll() is None:
            proc.kill()
        try:
            proc.wait(timeout=0.5)
        except subprocess.SubprocessError:
            pass
        with _CHILDREN_LOCK:
            _CHILDREN.discard(proc)
    return b"".join(chunks).decode("utf-8", "replace")


# ---------------------------------------------------------------------------
# HTTPS with certificate pinning

class PinError(Exception):
    pass


class BridgeError(Exception):
    """The bridge answered with CLIP "errors": a fixed message, never the bridge's own text."""


def https_request(ip, method, path, body=None, headers=None, pin=None, timeout=HTTP_TIMEOUT, port=None):
    """One HTTPS request to the bridge. Returns (status, parsed_json, (fingerprint, cn)).

    With `pin`, the peer certificate must match pin["fingerprint"] and have
    CN == pin["bridge_id"], otherwise PinError is raised before anything is sent.
    """
    port = port or BRIDGE_PORT
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE  # self-signed Philips cert; pinned below instead
    conn = http.client.HTTPSConnection(ip, port, timeout=timeout, context=ctx)
    # One overall deadline: a responder that drips headers or body byte by byte is
    # cut off when the timer shuts the socket down, not just per recv(). The socket is
    # kept here because a "Connection: close" reply takes it away from conn.sock.
    expired = threading.Event()
    connected = []
    resp = None

    def cut():
        expired.set()
        sock = connected[0] if connected else conn.sock
        if sock is not None:
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
    watchdog = threading.Timer(timeout, cut)
    watchdog.daemon = True
    watchdog.start()
    try:
        conn.connect()
        connected.append(conn.sock)
        if expired.is_set():
            raise OSError("bridge reply timed out")
        der = conn.sock.getpeercert(binary_form=True) or b""
        peer = (fingerprint(der), cert_common_name(der).upper())
        if pin is not None:
            if peer[0] != pin.get("fingerprint") or (pin.get("bridge_id") and peer[1] != pin["bridge_id"]):
                raise PinError("bridge certificate changed")
        data = json.dumps(body).encode() if body is not None else None
        hdrs = {"Accept": "application/json"}
        if data is not None:
            hdrs["Content-Type"] = "application/json"
        hdrs.update(headers or {})
        conn.request(method, path, body=data, headers=hdrs)
        resp = conn.getresponse()
        length = resp.getheader("Content-Length")
        if length is not None and (not length.strip().isdigit() or int(length) > MAX_BODY):
            raise OSError("reply too large")
        chunks, size = [], 0
        while size <= MAX_BODY:
            if expired.is_set():
                raise OSError("bridge reply timed out")
            chunk = resp.read1(65536)
            if not chunk:
                break
            chunks.append(chunk)
            size += len(chunk)
        if size > MAX_BODY:
            raise OSError("reply too large")
        if expired.is_set():
            raise OSError("bridge reply timed out")
        raw = b"".join(chunks)
        try:
            parsed = json.loads(raw.decode("utf-8")) if raw else None
        except (ValueError, RecursionError):
            parsed = None
        return resp.status, parsed, peer
    finally:
        watchdog.cancel()
        if resp is not None:
            resp.close()
        conn.close()
        for sock in connected:
            sock.close()


def open_sse_stream(ip, app_key, pin, timeout=HTTP_TIMEOUT, read_timeout=SSE_READ_TIMEOUT, port=None):
    """Open a pinned-TLS GET /eventstream/clip/v2 connection. Returns (conn, resp, sock).

    Verifies SHA-256 fingerprint + CN == bridge_id before sending hue-application-key.
    Raises PinError on certificate mismatch, PermissionError on 401/403, or OSError on failure.
    """
    port = port or BRIDGE_PORT
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    conn = http.client.HTTPSConnection(ip, port, timeout=timeout, context=ctx)
    resp = None
    sock = None
    try:
        conn.connect()
        sock = conn.sock
        der = sock.getpeercert(binary_form=True) or b""
        peer = (fingerprint(der), cert_common_name(der).upper())
        if pin is None or peer[0] != pin.get("fingerprint") or (pin.get("bridge_id") and peer[1] != pin["bridge_id"]):
            raise PinError("bridge certificate changed")
        conn.request("GET", "/eventstream/clip/v2", headers={
            "Accept": "text/event-stream",
            "hue-application-key": app_key,
        })
        resp = conn.getresponse()
        if resp.status in (401, 403):
            raise PermissionError("bridge rejected the app key")
        if resp.status != 200:
            raise OSError("eventstream returned HTTP %d" % resp.status)
        if sock is not None:
            sock.settimeout(read_timeout)
        return conn, resp, sock
    except BaseException:
        if resp is not None:
            try:
                resp.close()
            except OSError:
                pass
        try:
            conn.close()
        except OSError:
            pass
        if sock is not None:
            try:
                sock.close()
            except OSError:
                pass
        raise


def iter_bounded_lines(read_chunk, max_line=MAX_BODY):
    """Yield complete lines (bytes) from `read_chunk()`, dropping any line longer than `max_line`."""
    buf = bytearray()
    dropping = False
    while True:
        chunk = read_chunk()
        if not chunk:
            if not dropping and buf:
                yield bytes(buf)
            return
        start = 0
        while start < len(chunk):
            nl = chunk.find(b"\n", start)
            if nl == -1:
                seg = chunk[start:]
                if not dropping:
                    if len(buf) + len(seg) > max_line:
                        buf.clear()
                        dropping = True
                    else:
                        buf.extend(seg)
                break
            seg = chunk[start:nl + 1]
            start = nl + 1
            if dropping:
                dropping = False
                continue
            if len(buf) + len(seg) > max_line:
                buf.clear()
                continue
            buf.extend(seg)
            yield bytes(buf)
            buf.clear()


# ---------------------------------------------------------------------------
# Audio-reactive light synthesizer

class SynthController:
    """Streams audio-reactive (parec or Demo Beat) updates to Hue lights in a background thread."""

    def __init__(self, hue_obj):
        self.hue = hue_obj
        self.running = False
        self.source = "system"
        self.mode = "club"
        self.palette = "neon"
        self.room_id = ""
        self.sensitivity = 1.0
        self.smoothness = 0.45
        self.light_bands = {}
        self.bass = 0.0
        self.mids = 0.0
        self.treble = 0.0
        self.beat = False
        self.error = ""
        self._stop_ev = None
        self._thread = None
        self._proc = None

    def snapshot(self):
        """Return JSON-safe synth state dict (call under hue.lock)."""
        return {
            "running": bool(self.running),
            "source": self.source,
            "mode": self.mode,
            "palette": self.palette,
            "room_id": self.room_id,
            "sensitivity": round(self.sensitivity, 2),
            "smoothness": round(self.smoothness, 2),
            "light_bands": dict(self.light_bands),
            "bass": round(self.bass, 3),
            "mids": round(self.mids, 3),
            "treble": round(self.treble, 3),
            "beat": bool(self.beat),
            "error": self.error,
        }

    def configure(self, cmd):
        """Validate and update synth settings from a `synth_config` command (call under hue.lock)."""
        if not isinstance(cmd, dict):
            return False
        changed = False
        if "source" in cmd and cmd["source"] in SYNTH_SOURCES:
            self.source = cmd["source"]
            self.error = ""
            changed = True
        if "mode" in cmd and cmd["mode"] in SYNTH_MODES:
            self.mode = cmd["mode"]
            changed = True
        if "palette" in cmd and cmd["palette"] in SYNTH_PALETTES:
            self.palette = cmd["palette"]
            changed = True
        if "room_id" in cmd:
            rid = cmd["room_id"]
            if rid == "" or rid is None:
                self.room_id = ""
                changed = True
            elif valid_uuid(rid):
                self.room_id = rid
                changed = True
        if "sensitivity" in cmd and not isinstance(cmd["sensitivity"], bool):
            s = finite(cmd["sensitivity"], None, 0.2, 3.0)
            if s is not None:
                self.sensitivity = round(s, 2)
                changed = True
        if "smoothness" in cmd and not isinstance(cmd["smoothness"], bool):
            sm = finite(cmd["smoothness"], None, 0.0, 0.95)
            if sm is not None:
                self.smoothness = round(sm, 2)
                changed = True
        if isinstance(cmd.get("light_bands"), dict):
            cleaned = {}
            for lid, bnd in cmd["light_bands"].items():
                if len(cleaned) >= MAX_GROUP_LIGHTS:
                    break
                if valid_uuid(lid) and bnd in SYNTH_BANDS:
                    cleaned[lid] = bnd
            self.light_bands = cleaned
            changed = True
        if valid_uuid(cmd.get("light_id")) and cmd.get("band") in SYNTH_BANDS:
            if cmd["light_id"] in self.light_bands or len(self.light_bands) < MAX_GROUP_LIGHTS:
                self.light_bands[cmd["light_id"]] = cmd["band"]
                changed = True
        return changed

    def start(self):
        with self.hue.lock:
            if not self.hue.config:
                return False
            if self.running and self._thread is not None and self._thread.is_alive():
                return False
            self.running = True
            self.error = ""
            self.bass = self.mids = self.treble = 0.0
            self.beat = False
            stop_ev = threading.Event()
            self._stop_ev = stop_ev
            thread = threading.Thread(target=self._run, args=(stop_ev,), daemon=True)
            self._thread = thread
            thread.start()
        self.hue.emit()
        return True

    def stop(self, emit=True):
        with self.hue.lock:
            was_active = self.running or self._thread is not None or self._proc is not None
            self.running = False
            self.bass = self.mids = self.treble = 0.0
            self.beat = False
            stop_ev = self._stop_ev
            self._stop_ev = None
            thread = self._thread
            self._thread = None
            proc = self._proc
            self._proc = None
        if stop_ev is not None:
            stop_ev.set()
        if proc is not None:
            self._terminate_proc(proc)
        if thread is not None and thread is not threading.current_thread():
            thread.join(timeout=1.5)
        if emit and was_active:
            self.hue.emit()
        return was_active

    def _terminate_proc(self, proc):
        try:
            if proc.stdout is not None:
                proc.stdout.close()
        except OSError:
            pass
        try:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=0.3)
                except subprocess.SubprocessError:
                    proc.kill()
                    try:
                        proc.wait(timeout=0.3)
                    except subprocess.SubprocessError:
                        pass
        except OSError:
            pass
        with _CHILDREN_LOCK:
            _CHILDREN.discard(proc)

    def _target_lights(self):
        """Return target light dicts for current room_id (call under hue.lock)."""
        rid = self.room_id
        if rid:
            for room in self.hue.rooms:
                if room["id"] == rid:
                    return list(room["lights"])
            return []
        seen = set()
        lights = []
        for room in self.hue.rooms:
            if room["kind"] == "zone":
                continue
            for light in room["lights"]:
                if light["id"] not in seen:
                    seen.add(light["id"])
                    lights.append(light)
        return lights

    def _apply_step(self, bass, mids, treble, is_beat, now_mono, phase, last_light_push, last_emit):
        with self.hue.lock:
            self.bass = bass
            self.mids = mids
            self.treble = treble
            self.beat = is_beat
            panel_open = self.hue.panel_open
            should_push = (now_mono - last_light_push) >= SYNTH_LIGHT_INTERVAL
            pushed = False
            if should_push and self.hue.config:
                lights = self._target_lights()
                mode = self.mode
                palette = self.palette
                sensitivity = self.sensitivity
                smoothness = self.smoothness
                light_bands = dict(self.light_bands)
                for idx, light in enumerate(lights):
                    lid = light["id"]
                    band = light_bands.get(lid, "all")
                    frame = map_synth_frame(
                        idx, (bass, mids, treble, is_beat),
                        mode, palette, band, sensitivity, smoothness, phase,
                    )
                    body = {
                        "on": {"on": True},
                        "dimming": {"brightness": float(frame["bri"])},
                        "dynamics": {"duration": 200},
                    }
                    if light.get("supports_color"):
                        body["color"] = {"xy": {"x": frame["xy"][0], "y": frame["xy"][1]}}
                    if self.hue.queue("light", lid, body, from_rule=True):
                        self.hue._suppress_target("light", lid, now_mono + RULE_ECHO_WINDOW, expected_on=True)
                        pushed = True
                last_light_push = now_mono
        if pushed:
            self.hue.wake.set()
        emit_interval = SYNTH_EMIT_OPEN if panel_open else SYNTH_EMIT_CLOSED
        if now_mono - last_emit >= emit_interval:
            self.hue.emit()
            last_emit = now_mono
        return last_light_push, last_emit

    def _run(self, stop_ev):
        tracker = AudioBeatTracker()
        start_t = time.monotonic()
        last_light_push = 0.0
        last_emit = 0.0
        phase = 0.0
        try:
            while not stop_ev.is_set() and not self.hue._stop.is_set():
                with self.hue.lock:
                    if not self.running or not self.hue.config:
                        break
                    source = self.source
                    sensitivity = self.sensitivity
                    smoothness = self.smoothness
                if source == "demo":
                    now_mono = time.monotonic()
                    bass, mids, treble, is_beat = demo_beat_levels(now_mono - start_t)
                    phase += 0.14 + (0.25 if is_beat else 0.0)
                    last_light_push, last_emit = self._apply_step(
                        bass, mids, treble, is_beat, now_mono, phase, last_light_push, last_emit
                    )
                    stop_ev.wait(0.05)
                    continue

                # System or microphone capture via PulseAudio/PipeWire `parec`
                parec = shutil.which("parec")
                if not parec:
                    with self.hue.lock:
                        self.error = "parec not available"
                        self.running = False
                        self.bass = self.mids = self.treble = 0.0
                        self.beat = False
                    self.hue.emit()
                    return
                device = "@DEFAULT_MONITOR@" if source == "system" else "@DEFAULT_SOURCE@"
                argv = _parent_death_prefix() + [
                    parec, "--raw", "--format=s16le", "--channels=1", "--rate=16000",
                    "--device=" + device,
                ]
                proc = None
                try:
                    with _CHILDREN_LOCK:
                        proc = subprocess.Popen(
                            argv, shell=False, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                        )
                        _CHILDREN.add(proc)
                    with self.hue.lock:
                        if stop_ev.is_set() or not self.running:
                            break
                        self._proc = proc
                    tracker.reset()
                    fd = proc.stdout.fileno()
                    pcm_buf = bytearray()
                    chunk_bytes = 1024  # 512 s16le samples = 32ms @ 16kHz
                    while not stop_ev.is_set() and not self.hue._stop.is_set():
                        with self.hue.lock:
                            if not self.running or not self.hue.config or self.source != source:
                                break
                            sensitivity = self.sensitivity
                            smoothness = self.smoothness
                        try:
                            ready, _, _ = select.select([fd], [], [], 0.1)
                        except (OSError, ValueError):
                            break
                        if not ready:
                            continue
                        try:
                            chunk = os.read(fd, 4096)
                        except OSError:
                            chunk = b""
                        if not chunk:
                            if not stop_ev.is_set():
                                with self.hue.lock:
                                    if self.running and self.source == source:
                                        self.error = "audio capture exited"
                                        self.running = False
                                        self.bass = self.mids = self.treble = 0.0
                                        self.beat = False
                                self.hue.emit()
                            return
                        pcm_buf.extend(chunk)
                        if len(pcm_buf) < chunk_bytes:
                            continue
                        frame_bytes = bytes(pcm_buf[-chunk_bytes:])
                        pcm_buf.clear()
                        raw_b, raw_m, raw_t = analyze_pcm_bands(frame_bytes, sample_rate=16000)
                        now_mono = time.monotonic()
                        bass, mids, treble, is_beat = tracker.update(
                            raw_b, raw_m, raw_t,
                            sensitivity=sensitivity, smoothness=smoothness, now_mono=now_mono,
                        )
                        phase += 0.10 + (0.25 if is_beat else 0.0)
                        last_light_push, last_emit = self._apply_step(
                            bass, mids, treble, is_beat, now_mono, phase, last_light_push, last_emit
                        )
                except OSError as exc:
                    if not stop_ev.is_set():
                        with self.hue.lock:
                            self.error = clean(str(exc), 80) or "audio capture failed"
                            self.running = False
                            self.bass = self.mids = self.treble = 0.0
                            self.beat = False
                        self.hue.emit()
                    return
                finally:
                    if proc is not None:
                        with self.hue.lock:
                            if self._proc is proc:
                                self._proc = None
                        self._terminate_proc(proc)
        finally:
            with self.hue.lock:
                if self._thread is threading.current_thread():
                    self._thread = None
                if self._stop_ev is stop_ev:
                    self._stop_ev = None
                    self.running = False


# ---------------------------------------------------------------------------
# Main state machine

class Hue:
    def __init__(self):
        self.lock = threading.RLock()
        self.wake = threading.Event()
        self.sse_wake = threading.Event()
        self._stop = threading.Event()
        self._sse_sock = None
        self.config = load_config()
        self.panel_open = False
        self.rooms = []
        self.light_count = 0
        self.on_count = 0
        self.bridge_home_gl = ""
        self.custom_themes = load_themes()
        local_sched_data = load_local_schedules()
        self.schedule_mode = local_sched_data["mode"]
        self.bridge_schedules = []
        self.local_schedules = local_sched_data["schedules"]
        self.schedules = list(self.local_schedules)
        self._local_sched_fired = {}  # schedule_id -> "YYYY-MM-DD HH:MM"
        self.schedule_script_id = DEFAULT_SCHEDULE_SCRIPT_ID
        rules_data = load_rules()
        self.rules = rules_data["rules"]
        self.rules_paused = rules_data["paused"]
        self.rule_log = []
        self.echo_suppress = {}       # (kind, rid) -> (expires_monotonic, expected_on)
        self.rule_last_fired = {}     # rule_id -> last_fired_monotonic
        self.api_queue = []           # [(method, path, body), ...]
        self.synth = SynthController(self)
        self.phase = "connecting" if self.config else "unpaired"
        self.error = ""
        # error_code/error_args let the panel show the message in its language.
        self.error_code = ""
        self.error_args = []
        self.candidates = []
        self.discovering = False
        self.pair_ip = ""
        self.pair_until = 0.0
        self.pair_generation = 0   # bumped by pair, cancel_pair and unpair
        # Bumped by pair, a saved pairing and unpair: a refresh, relocation or command that
        # started under an older connection drops its result.
        self.conn_generation = 0
        # Keys this process has held (application key, client key): never part of an error text.
        # Only well-formed keys from a pairing that is still current, at most MAX_SECRETS.
        self.secrets = {self.config["app_key"]} if self.config.get("app_key") else set()
        self.pending = {}          # (kind, id) -> body, coalesced
        self.last_sent = {"light": 0.0, "grouped_light": 0.0, "scene": 0.0}
        self.last_refresh = 0.0
        self.refresh_hold_until = 0.0

    def _reset_sse(self):
        """Close any active SSE socket and wake the SSE listener thread."""
        sock = None
        with self.lock:
            sock = getattr(self, "_sse_sock", None)
            self._sse_sock = None
        if sock is not None:
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            try:
                sock.close()
            except OSError:
                pass
        sse_wake = getattr(self, "sse_wake", None)
        if sse_wake is not None:
            sse_wake.set()

    def remember_secrets(self, *keys):
        """Under self.lock: keep `keys` for redaction. The set stays small: when full, only the
        current application key survives."""
        keys = [k for k in keys if k]
        if len(self.secrets) + len(keys) > MAX_SECRETS:
            current = self.config.get("app_key")
            self.secrets = {current} if current else set()
        self.secrets.update(keys)

    def redact(self, text):
        text = str(text)
        for secret in self.secrets | {self.config.get("app_key") or ""}:
            if len(secret) >= 6:
                text = text.replace(secret, "***")
        return text

    def safe(self, exc, limit):
        """An exception's text for the panel: keys removed first, then cleaned and cut."""
        return clean(self.redact(exc), limit)

    def set_error(self, code, text, *args):
        """English message for logs and old panels, plus a code and arguments the panel translates.
        Keys are removed from every part as a backstop."""
        args = [self.redact(a) for a in args]
        self.error = self.redact(text % tuple(args) if args else text)
        self.error_code = code
        self.error_args = args

    # -- output
    def snapshot(self):
        with self.lock:
            bridge = {}
            if self.config:
                bridge = {"ip": self.config.get("ip", ""), "name": self.config.get("name", "Hue Bridge"),
                          "model": self.config.get("model", "")}
            return {
                "phase": self.phase, "error": self.error, "bridge": bridge,
                "error_code": self.error_code if self.error else "", "error_args": self.error_args if self.error else [],
                "rooms": self.rooms, "light_count": self.light_count, "on_count": self.on_count,
                "candidates": self.candidates, "discovering": self.discovering,
                "pair_ip": self.pair_ip,
                "pair_remaining": max(0, int(self.pair_until - time.time())) if self.pair_ip else 0,
                "schedules": getattr(self, "schedules", []),
                "schedule_mode": getattr(self, "schedule_mode", "bridge"),
                "themes": [dict(t) for t in BUILTIN_THEMES] + list(getattr(self, "custom_themes", [])),
                "synth": self.synth.snapshot() if hasattr(self, "synth") else {
                    "running": False, "source": "system", "mode": "club", "palette": "neon",
                    "room_id": "", "sensitivity": 1.0, "smoothness": 0.45, "light_bands": {},
                    "bass": 0.0, "mids": 0.0, "treble": 0.0, "beat": False, "error": "",
                },
                "rules": getattr(self, "rules", []),
                "rules_paused": getattr(self, "rules_paused", False),
                "rule_log": getattr(self, "rule_log", []),
            }

    def emit(self):
        # Taken, serialised and written under one lock, so snapshots leave in the order they were
        # taken (an older one can never overwrite the panel's view after Unpair).
        with self.lock:
            snap = self.snapshot()
            try:
                line = json.dumps(snap, separators=(",", ":"), allow_nan=False)
            except ValueError:   # backstop: a non-finite number the parsers missed
                self.rooms, self.light_count, self.on_count, self.phase = [], 0, 0, "error"
                self.set_error("errReply", "The bridge sent a reply this app does not understand.")
                line = json.dumps(self.snapshot(), separators=(",", ":"), allow_nan=False)
            sys.stdout.write(line + "\n")
            sys.stdout.flush()

    # -- bridge access
    def connection(self):
        """(config snapshot, generation) for network work outside the lock; no config while a
        pairing runs, so nothing started then can change what the pairing shows."""
        with self.lock:
            return (dict(self.config) if not self.pair_ip else {}), self.conn_generation

    def connection_current(self, generation):
        """Still the connection the work started under, and no pairing running (lock held)."""
        return bool(self.config) and not self.pair_ip and self.conn_generation == generation

    def known(self, kind, rid):
        """A light, grouped light or scene of the current model (call with the lock held)."""
        if kind == "grouped_light" and rid and rid == getattr(self, "bridge_home_gl", ""):
            return True
        for room in self.rooms:
            if kind == "grouped_light" and room["grouped_light"] == rid:
                return True
            if kind == "light" and any(light["id"] == rid for light in room["lights"]):
                return True
            if kind == "scene" and any(scene["id"] == rid for scene in room["scenes"]):
                return True
        return False

    def known_target(self, target_kind, target_id):
        """A light, room or zone UUID of the current model (call with the lock held)."""
        if not valid_uuid(target_id):
            return False
        if target_kind == "light":
            return self.known("light", target_id)
        if target_kind in ("room", "zone"):
            return any(r["kind"] == target_kind and r["id"] == target_id for r in self.rooms)
        return False

    def may_queue(self, kind, rid):
        """Only resources the panel can show, and at most MAX_PENDING queued (lock held)."""
        return self.known(kind, rid) and ((kind, rid) in self.pending or len(self.pending) < MAX_PENDING)

    def api(self, method, path, body=None, config=None):
        config = config if config is not None else self.connection()[0]
        pin = {"fingerprint": config.get("fingerprint"), "bridge_id": config.get("bridge_id", "")}
        status, data, _ = https_request(config["ip"], method, path, body,
                                        {"hue-application-key": config["app_key"]}, pin=pin)
        if status in (401, 403):
            raise PermissionError("bridge rejected the app key")
        if status >= 400:
            raise OSError("bridge returned HTTP %d" % status)
        errors = data.get("errors") if isinstance(data, dict) else None
        if isinstance(errors, list) and errors:
            raise BridgeError("bridge reported an error")
        return data

    def refresh(self):
        config, generation = self.connection()
        if not config:
            return
        failure = None
        try:
            data = self.api("GET", "/clip/v2/resource", config=config)
            if not isinstance(data, dict) or not isinstance(data.get("data"), list):
                raise ValueError("unexpected reply")
            rooms, total, on = build_model(data["data"])
            bridge_home_gl = extract_bridge_home_gl(data["data"])
            schedules, script_id = parse_schedules(data["data"], rooms)
            with self.lock:
                if self.connection_current(generation):
                    if time.time() >= self.refresh_hold_until:
                        self.rooms, self.light_count, self.on_count = rooms, total, on
                    self.bridge_home_gl = bridge_home_gl
                    self.bridge_schedules = schedules
                    self._rebuild_schedules()
                    if script_id:
                        self.schedule_script_id = script_id
                    self.phase, self.error = "ok", ""
        except (ValueError, TypeError, KeyError, AttributeError, BridgeError):
            failure = ("errReply", "The bridge sent a reply this app does not understand.")
        except PinError:
            failure = ("errCert", "The bridge certificate changed. Unpair and pair again if you replaced the bridge.")
        except PermissionError:
            failure = ("errRejected", "The bridge no longer accepts this app. Unpair and pair again.")
        except (OSError, socket.timeout, http.client.HTTPException) as exc:
            failure = ("errUnreachable", "Bridge not reachable (%s)", self.safe(exc, 80))
        if failure:
            with self.lock:
                current = self.connection_current(generation)
                if current:
                    self.phase = "error"
                    self.set_error(*failure)
            if current and failure[0] == "errUnreachable":
                self.relocate(config, generation)
        self.last_refresh = time.time()

    def relocate(self, config, generation):
        """The bridge got a new DHCP address: find it again by bridge id via mDNS. The new address
        is saved only if Unpair or another Pair did not replace the connection meanwhile."""
        bridge_id = config.get("bridge_id")
        if not bridge_id:
            return
        for cand in self.discover_now():
            if cand["bridge_id"] == bridge_id and cand["ip"] != config.get("ip"):
                with self.lock:
                    if not self.connection_current(generation):
                        return
                    moved = dict(self.config, ip=cand["ip"])
                    save_config(moved)
                    self.config = moved
                    self.conn_generation += 1
                self._reset_sse()
                self.wake.set()
                return

    def discover_now(self):
        return parse_avahi_hue(run_bounded(["avahi-browse", "-rtpk", "_hue._tcp"]).splitlines())

    def claim_discovery(self):
        """One discovery at a time: True when this caller got the slot."""
        with self.lock:
            if self.discovering:
                return False
            self.discovering = True
            return True

    def start_discovery(self):
        if self.claim_discovery():
            threading.Thread(target=self.run_discovery, daemon=True).start()

    def discover(self):
        if self.claim_discovery():
            self.run_discovery()

    def run_discovery(self):
        """With the slot claimed: browse, publish the candidates, release the slot."""
        found = []
        try:
            self.emit()
            found = self.discover_now()
        finally:
            with self.lock:
                self.candidates, self.discovering = found, False
            self.emit()

    # -- pairing
    def pairing_current(self, ip, generation):
        """Still the pairing this attempt started for (call with the lock held)."""
        return self.pair_ip == ip and self.pair_generation == generation

    def try_pair(self):
        with self.lock:
            ip, generation = self.pair_ip, self.pair_generation
        if not ip:
            return
        try:
            _, cfg, peer = https_request(ip, "GET", "/api/0/config")
            cfg = cfg if isinstance(cfg, dict) else {}
            bridge_id = str((cfg or {}).get("bridgeid", "")).upper()
            if not BRIDGE_ID_RE.match(bridge_id) or peer[1] != bridge_id:
                with self.lock:
                    if not self.pairing_current(ip, generation):
                        return
                    self.set_error("errNotHue", "%s does not look like a Hue bridge", ip)
                    self.pair_ip = ""
                    self.phase = "unpaired"
                return
            with self.lock:
                if not self.pairing_current(ip, generation):
                    return   # Cancel, Unpair or a new Pair during the first request: no key request
            host = clean(socket.gethostname(), 19) or "pc"
            _, resp, peer2 = https_request(ip, "POST", "/api",
                                           {"devicetype": "gnome_hue_lights#" + host, "generateclientkey": True})
            if peer2 != peer:
                raise PinError("certificate changed during pairing")
            entry = resp[0] if isinstance(resp, list) and resp and isinstance(resp[0], dict) else {}
            if not isinstance(entry.get("success", {}), dict) or not isinstance(entry.get("error", {}), dict):
                entry = {}
            key = (entry.get("success") or {}).get("username")
            client_key = (entry.get("success") or {}).get("clientkey")
            key = key if isinstance(key, str) and APP_KEY_RE.fullmatch(key) else None
            client_key = client_key if isinstance(client_key, str) and CLIENT_KEY_RE.fullmatch(client_key) else None
            with self.lock:
                if not self.pairing_current(ip, generation):
                    return   # a reply for an attempt that was cancelled or replaced: nothing kept
                self.remember_secrets(key, client_key)
            if key:
                config = {
                    "ip": ip, "bridge_id": bridge_id, "fingerprint": peer[0],
                    "app_key": key,
                    "name": clean(cfg.get("name") or "Hue Bridge"), "model": clean(cfg.get("modelid") or ""),
                }
                with self.lock:
                    if not self.pairing_current(ip, generation):
                        return
                    save_config(config)
                    self.conn_generation += 1
                    self.rooms, self.light_count, self.on_count, self.pending = [], 0, 0, {}
                    self.bridge_home_gl, self.bridge_schedules, self.api_queue = "", [], []
                    self._rebuild_schedules()
                    self.config, self.pair_ip, self.phase, self.error = config, "", "connecting", ""
                self._reset_sse()
                self.wake.set()
            elif (entry.get("error") or {}).get("type") == 101:
                with self.lock:
                    if self.pairing_current(ip, generation):
                        self.error = ""  # link button not pressed yet: keep asking
            else:
                with self.lock:
                    if not self.pairing_current(ip, generation):
                        return
                    kind = (entry.get("error") or {}).get("type")
                    self.set_error("errPairFailed", "Pairing failed: %s",
                                   "bridge error %d" % kind if type(kind) is int and 0 <= kind < 100000 else "bridge error")
        except (OSError, socket.timeout, http.client.HTTPException, PinError, ValueError) as exc:
            with self.lock:
                if not self.pairing_current(ip, generation):
                    return
                self.set_error("errCannotReach", "Cannot reach %s (%s)", ip, self.safe(exc, 60))

    # -- control commands
    def _capture_states(self, kind, rid):
        """Capture current light and room states before an optimistic update (lock held)."""
        light_state = None
        room_states = {}
        for room in self.rooms:
            if room["kind"] == "room" and valid_uuid(room["id"]):
                room_states[room["id"]] = {"on": room["on"], "bri": room["bri"]}
            if kind == "light" and light_state is None:
                for light in room["lights"]:
                    if light["id"] == rid:
                        light_state = {"on": light["on"], "bri": light["bri"]}
                        break
        return light_state, room_states

    def _collect_transitions(self, kind, rid, old_light, old_rooms):
        """Compare current states against pre-mutation snapshots (lock held)."""
        transitions = []
        if kind == "light" and old_light is not None:
            for room in self.rooms:
                for light in room["lights"]:
                    if light["id"] == rid:
                        new_light = {"on": light["on"], "bri": light["bri"]}
                        if new_light != old_light:
                            transitions.append(("light", rid, old_light, new_light))
                        break
                else:
                    continue
                break
        for room in self.rooms:
            if room["kind"] == "room" and room["id"] in old_rooms:
                new_rs = {"on": room["on"], "bri": room["bri"]}
                if new_rs != old_rooms[room["id"]]:
                    transitions.append(("room", room["id"], old_rooms[room["id"]], new_rs))
        return transitions

    def queue(self, kind, rid, body, from_rule=False):
        with self.lock:
            if not self.may_queue(kind, rid):
                return False
            old_light, old_rooms = (None, {})
            if not from_rule and getattr(self, "rules", None):
                old_light, old_rooms = self._capture_states(kind, rid)
            key = (kind, rid)
            merged = dict(self.pending.get(key, {}))
            merged.update(body)
            self.pending[key] = merged
            self.apply_optimistic(kind, rid, body)
            self.refresh_hold_until = time.time() + 1.0
            if not from_rule and getattr(self, "rules", None):
                transitions = self._collect_transitions(kind, rid, old_light, old_rooms)
                if transitions:
                    self.evaluate_rules(transitions)
        if not from_rule:
            self.emit()
            self.wake.set()
        return True

    def all_off(self):
        """Every room that is on, and every light in no room that is on: all commands queued at
        once, the model updated in one pass, one snapshot and one wake-up (not one per light)."""
        with self.lock:
            items = []
            for room in self.rooms:
                if room["kind"] == "room" and room["on"] and valid_uuid(room["grouped_light"]):
                    items.append(("grouped_light", room["grouped_light"]))
                elif room["kind"] == "other":
                    items += [("light", light["id"]) for light in room["lights"] if light["on"]]
            if not items:
                return
            for key in items:
                self.pending[key] = {**self.pending.get(key, {}), "on": {"on": False}}
            groups = {rid for kind, rid in items if kind == "grouped_light"}
            off = {rid for kind, rid in items if kind == "light"}
            for room in self.rooms:
                if room["grouped_light"] in groups:
                    off.update(light["id"] for light in room["lights"])
            for room in self.rooms:
                for light in room["lights"]:
                    if light["id"] in off:
                        light["on"] = False
                lit = sum(1 for light in room["lights"] if light["on"])
                room["on_count"] = lit
                room["on"] = bool(lit) and room["grouped_light"] not in groups
            self.on_count = sum(1 for r in self.rooms if r["kind"] != "zone" for l in r["lights"] if l["on"])
            self.refresh_hold_until = time.time() + 1.0
        self.emit()
        self.wake.set()

    def all_on(self, exclude_rooms=None):
        """Turn on all rooms (or via bridge_home when nothing is excluded) and unassigned lights."""
        excluded = set()
        if isinstance(exclude_rooms, list):
            excluded = {r for r in exclude_rooms if valid_uuid(r)}
        with self.lock:
            home_gl = getattr(self, "bridge_home_gl", "")
            items = []
            on_ids = set()
            if not excluded and valid_uuid(home_gl):
                items.append(("grouped_light", home_gl))
                for room in self.rooms:
                    on_ids.update(light["id"] for light in room["lights"])
            else:
                for room in self.rooms:
                    if room["kind"] == "room" and room["id"] not in excluded:
                        if valid_uuid(room["grouped_light"]):
                            items.append(("grouped_light", room["grouped_light"]))
                        on_ids.update(light["id"] for light in room["lights"])
                    elif room["kind"] == "other":
                        for light in room["lights"]:
                            items.append(("light", light["id"]))
                            on_ids.add(light["id"])
            if not items:
                return
            for key in items:
                if key in self.pending or len(self.pending) < MAX_PENDING:
                    self.pending[key] = {**self.pending.get(key, {}), "on": {"on": True}}
            for room in self.rooms:
                for light in room["lights"]:
                    if light["id"] in on_ids:
                        light["on"] = True
                lit = [light for light in room["lights"] if light["on"]]
                room["on_count"] = len(lit)
                room["on"] = bool(lit)
                room["color"] = blend_colors([l["color"] for l in lit])
            self.on_count = sum(1 for r in self.rooms if r["kind"] != "zone" for l in r["lights"] if l["on"])
            self.refresh_hold_until = time.time() + 1.0
        self.emit()
        self.wake.set()

    def apply_optimistic(self, kind, rid, body):
        on = body.get("on", {}).get("on") if "on" in body else None
        bri = body.get("dimming", {}).get("brightness") if "dimming" in body else None
        mirek = body.get("color_temperature", {}).get("mirek") if "color_temperature" in body else None
        xy = body.get("color", {}).get("xy") if "color" in body else None
        effect = body.get("effects", {}).get("effect") if "effects" in body else None
        for room in self.rooms:
            targets = []
            if kind == "grouped_light" and room["grouped_light"] == rid:
                targets = room["lights"]
                if on is not None:
                    room["on"] = on
                if bri is not None:
                    room["bri"], room["on"] = bri, True
                if mirek is not None or xy is not None or (effect is not None and effect != "no_effect"):
                    room["on"] = True
            elif kind == "light":
                targets = [l for l in room["lights"] if l["id"] == rid]
            for light in targets:
                if on is not None:
                    light["on"] = on
                if bri is not None:
                    light["bri"], light["on"] = bri, True
                if mirek is not None:
                    light["mirek"] = mirek
                    light["color"] = mirek_to_hex(mirek)
                    light["on"] = True
                if xy is not None:
                    light["color"] = xy_to_hex(xy.get("x"), xy.get("y"))
                    light["mirek"] = None
                    light["on"] = True
                if effect is not None:
                    light["effect"] = effect
                    if effect != "no_effect":
                        light["on"] = True
            if kind == "light" and targets:
                lit = [l for l in room["lights"] if l["on"]]
                room["on"], room["on_count"] = bool(lit), len(lit)
                room["color"] = blend_colors([l["color"] for l in lit])
                if lit and room["grouped_light"] == "":
                    room["bri"] = round(sum(l["bri"] for l in lit) / len(lit), 1)
            elif kind == "grouped_light" and targets:
                lit = [l for l in room["lights"] if l["on"]]
                room["on_count"] = len(lit)
                room["color"] = blend_colors([l["color"] for l in lit])
        self.on_count = sum(1 for r in self.rooms if r["kind"] != "zone" for l in r["lights"] if l["on"])

    # -- schedules & theme helpers
    def _rebuild_schedules(self):
        """Keep self.schedules as bridge_schedules + local_schedules (call with lock held)."""
        self.schedules = list(getattr(self, "bridge_schedules", [])) + list(getattr(self, "local_schedules", []))

    def _find_light(self, light_id):
        """Return light dict for light_id in current model (call with lock held), or None."""
        for room in self.rooms:
            for light in room["lights"]:
                if light["id"] == light_id:
                    return light
        return None

    def _target_name(self, target_kind, target_id):
        """Resolve display name for a light, room, or zone UUID (call with lock held)."""
        if target_kind == "light":
            light = self._find_light(target_id)
            return light.get("name", "") if light else ""
        if target_kind in ("room", "zone"):
            for room in self.rooms:
                if room["kind"] == target_kind and room["id"] == target_id:
                    return room.get("name", "")
        return ""

    def check_local_schedules(self, now=None):
        """Check enabled local schedules against local (weekday, HH:MM), firing at most once per minute."""
        if now is None:
            lt = time.localtime()
            year, month, day, hour, minute, wday = lt.tm_year, lt.tm_mon, lt.tm_mday, lt.tm_hour, lt.tm_min, lt.tm_wday
        elif hasattr(now, "tm_year"):
            year, month, day, hour, minute, wday = now.tm_year, now.tm_mon, now.tm_mday, now.tm_hour, now.tm_min, now.tm_wday
        elif hasattr(now, "weekday"):
            year, month, day, hour, minute, wday = now.year, now.month, now.day, now.hour, now.minute, now.weekday()
        else:
            year, month, day, hour, minute, wday = now
        day_short = DAY_ORDER[int(wday) % 7]
        hhmm = "%02d:%02d" % (int(hour), int(minute))
        bucket = "%04d-%02d-%02d %02d:%02d" % (int(year), int(month), int(day), int(hour), int(minute))

        fired = 0
        with self.lock:
            if not self.config or not self.local_schedules:
                return 0
            # Prune previous-minute buckets so dict stays tiny
            self._local_sched_fired = {sid: b for sid, b in self._local_sched_fired.items() if b == bucket}
            for sched in self.local_schedules:
                if not sched.get("enabled"):
                    continue
                if sched.get("trigger_kind", "time") != "time" or sched.get("time") != hhmm:
                    continue
                days = sched.get("days") or []
                if days and day_short not in days:
                    continue
                sid = sched["id"]
                if self._local_sched_fired.get(sid) == bucket or sched.get("last_fired") == bucket:
                    self._local_sched_fired[sid] = bucket
                    continue
                if not claim_local_schedule_bucket(sid, bucket):
                    sched["last_fired"] = bucket
                    self._local_sched_fired[sid] = bucket
                    continue
                action = sched.get("action")
                ok = False
                if action == "scene":
                    scene_id = sched.get("scene_id", "")
                    if valid_uuid(scene_id) and self.may_queue("scene", scene_id):
                        recall = {"action": "active"}
                        if sched.get("fade_min", 0) > 0:
                            recall["duration"] = int(sched["fade_min"]) * 60000
                        self.pending[("scene", scene_id)] = {"recall": recall}
                        self.refresh_hold_until = 0
                        ok = True
                else:
                    body = {"on": {"on": action == "on"}}
                    if action == "on" and sched.get("bri") is not None:
                        body["dimming"] = {"brightness": round(clamp(float(sched["bri"]), 1.0, 100.0), 1)}
                    if sched.get("fade_min", 0) > 0:
                        body["dynamics"] = {"duration": int(sched["fade_min"]) * 60000}
                    tkind = sched.get("target_kind")
                    tid = sched.get("target_id")
                    if tkind == "light":
                        ok = self.queue("light", tid, body, from_rule=True)
                    elif tkind in ("room", "zone"):
                        for room in self.rooms:
                            if room["kind"] == tkind and room["id"] == tid:
                                if valid_uuid(room.get("grouped_light")):
                                    ok = self.queue("grouped_light", room["grouped_light"], body, from_rule=True)
                                else:
                                    for light in room["lights"]:
                                        if self.queue("light", light["id"], body, from_rule=True):
                                            ok = True
                                break
                if ok:
                    sched["last_fired"] = bucket
                    self._local_sched_fired[sid] = bucket
                    fired += 1
        if fired:
            self.emit()
            self.wake.set()
        return fired

    # -- dependency / linked-light rule engine
    def _target_is_on(self, tkind, tid):
        if tkind == "light":
            for room in self.rooms:
                for light in room["lights"]:
                    if light["id"] == tid:
                        return bool(light["on"])
        elif tkind == "room":
            for room in self.rooms:
                if room["id"] == tid and room["kind"] == "room":
                    return bool(room["on"])
        return False

    def _suppress_target(self, tkind, tid, expires, expected_on=None):
        entry = (expires, expected_on)
        self.echo_suppress[(tkind, tid)] = entry
        if tkind == "room":
            for room in self.rooms:
                if room["id"] == tid and room["kind"] == "room":
                    if room.get("grouped_light"):
                        self.echo_suppress[("grouped_light", room["grouped_light"])] = entry
                    for light in room["lights"]:
                        self.echo_suppress[("light", light["id"])] = entry
                    break

    def _dispatch_rule_target(self, target, body, recall_scene=False):
        """Queue a rule-initiated command for a light or room target (lock held). Returns True if queued."""
        tkind, tid = target["kind"], target["id"]
        if tkind == "light":
            return self.queue("light", tid, body, from_rule=True)
        if tkind == "room":
            if recall_scene and target.get("scene_id") and self.may_queue("scene", target["scene_id"]):
                self.pending[("scene", target["scene_id"])] = {"recall": {"action": "active"}}
                self.refresh_hold_until = 0
                return True
            for room in self.rooms:
                if room["id"] == tid and room["kind"] == "room":
                    if valid_uuid(room.get("grouped_light")):
                        return self.queue("grouped_light", room["grouped_light"], body, from_rule=True)
                    queued_any = False
                    for light in room["lights"]:
                        if self.queue("light", light["id"], body, from_rule=True):
                            queued_any = True
                    return queued_any
        return False

    def evaluate_rules(self, transitions, now_mono=None, now_time=None, depth=0):
        """Evaluate dependency rules for [(kind, rid, old_state, new_state), ...] (call with lock held)."""
        if getattr(self, "rules_paused", False) or depth >= MAX_CASCADE_DEPTH:
            return 0
        rules = getattr(self, "rules", None)
        if not rules or not self.config:
            return 0
        now_mono = time.monotonic() if now_mono is None else float(now_mono)
        # Prune expired echo suppressions (supports both (expires, expected_on) and legacy bare float)
        self.echo_suppress = {
            k: v for k, v in self.echo_suppress.items()
            if _echo_entry(v)[0] > now_mono
        }
        fired = 0
        for kind, rid, old_state, new_state in transitions:
            if kind != "desktop" and (kind, rid) in self.echo_suppress:
                exp, expected_on = _echo_entry(self.echo_suppress[(kind, rid)])
                if exp > now_mono:
                    new_on = new_state.get("on") if isinstance(new_state, dict) else None
                    if expected_on is None or new_on is None or new_on == expected_on:
                        continue
                    del self.echo_suppress[(kind, rid)]
            for rule in rules:
                if not rule.get("enabled", True):
                    continue
                if rule.get("trigger_kind") != kind or rule.get("trigger_id") != rid:
                    continue
                if now_mono - self.rule_last_fired.get(rule["id"], -1e9) < RULE_COOLDOWN:
                    continue
                if not in_time_window(rule.get("time_start", ""), rule.get("time_end", ""), now=now_time):
                    continue
                mode = None
                new_bri = new_state.get("bri") if isinstance(new_state, dict) else None
                if kind == "desktop":
                    mode = "desktop"
                else:
                    old_on = old_state.get("on") if isinstance(old_state, dict) else None
                    new_on = new_state.get("on") if isinstance(new_state, dict) else None
                    old_bri = old_state.get("bri") if isinstance(old_state, dict) else None
                    on_changed = (old_on is not None and new_on is not None and old_on != new_on)
                    bri_changed = (old_bri is not None and new_bri is not None and abs(float(old_bri) - float(new_bri)) >= 0.5)
                    tstate = rule.get("trigger_state")
                    if on_changed and ((new_on is True and tstate == "on") or (new_on is False and tstate == "off")):
                        mode = "trigger"
                    elif on_changed and rule.get("mirror_off") and tstate == "on" and new_on is False:
                        mode = "mirror_off"
                    elif (not on_changed) and bri_changed and rule.get("match_bri") and tstate == "on" and new_on is True:
                        mode = "match_bri"
                if mode is None:
                    continue
                applied = 0
                for target in rule["targets"]:
                    expected_on = None
                    if mode == "mirror_off":
                        ok = self._dispatch_rule_target(target, {"on": {"on": False}})
                        expected_on = False
                    elif mode == "match_bri":
                        if target["action"] != "on" or target.get("scene_id"):
                            continue
                        if rule.get("only_if_off") and self._target_is_on(target["kind"], target["id"]):
                            continue
                        b = round(clamp(float(new_bri), 1.0, 100.0), 1)
                        ok = self._dispatch_rule_target(target, {"on": {"on": True}, "dimming": {"brightness": b}})
                        expected_on = True
                    else:
                        if rule.get("only_if_off") and self._target_is_on(target["kind"], target["id"]):
                            continue
                        if target["action"] == "off":
                            ok = self._dispatch_rule_target(target, {"on": {"on": False}})
                            expected_on = False
                        else:
                            expected_on = True
                            if target.get("scene_id") and target["kind"] == "room" and self.known("scene", target["scene_id"]):
                                ok = self._dispatch_rule_target(target, {}, recall_scene=True)
                            else:
                                body = {"on": {"on": True}}
                                if mode == "trigger" and rule.get("match_bri") and new_bri is not None:
                                    body["dimming"] = {"brightness": round(clamp(float(new_bri), 1.0, 100.0), 1)}
                                elif target.get("bri") is not None:
                                    body["dimming"] = {"brightness": round(clamp(float(target["bri"]), 1.0, 100.0), 1)}
                                ok = self._dispatch_rule_target(target, body)
                    if ok:
                        self._suppress_target(target["kind"], target["id"], now_mono + RULE_ECHO_WINDOW,
                                              expected_on=expected_on)
                        applied += 1
                if applied:
                    self.rule_last_fired[rule["id"]] = now_mono
                    detail = "%s (%d target%s)" % (mode, applied, "" if applied == 1 else "s")
                    self.rule_log.append({
                        "ts": int(time.time()),
                        "rule_id": rule["id"],
                        "rule_name": rule["name"],
                        "detail": clean(detail, 120),
                    })
                    if len(self.rule_log) > MAX_RULE_LOG:
                        self.rule_log = self.rule_log[-MAX_RULE_LOG:]
                    fired += 1
        return fired

    # -- SSE eventstream
    def apply_sse_updates(self, updates, generation=None, now_mono=None, now_time=None):
        """Apply parsed SSE (kind, rid, changes) updates to in-memory state, run rules, and emit."""
        if not updates:
            return False
        changed = False
        rules_fired = 0
        with self.lock:
            if generation is not None and not self.connection_current(generation):
                return False
            transitions = []
            for kind, rid, changes in updates:
                if kind == "light":
                    old_light = None
                    old_rooms = {}
                    for room in self.rooms:
                        if room["kind"] == "room" and valid_uuid(room["id"]):
                            old_rooms[room["id"]] = {"on": room["on"], "bri": room["bri"]}
                        for light in room["lights"]:
                            if light["id"] == rid:
                                if old_light is None:
                                    old_light = {"on": light["on"], "bri": light["bri"]}
                                if "on" in changes:
                                    light["on"] = changes["on"]
                                if "bri" in changes:
                                    light["bri"] = changes["bri"]
                                if "mirek" in changes:
                                    light["mirek"] = changes["mirek"]
                                    if changes["mirek"] is not None:
                                        light["color"] = mirek_to_hex(changes["mirek"])
                                if "xy" in changes:
                                    light["color"] = xy_to_hex(changes["xy"][0], changes["xy"][1])
                                    light["mirek"] = None
                                if "effect" in changes:
                                    light["effect"] = changes["effect"]
                                lit = [l for l in room["lights"] if l["on"]]
                                room["on"], room["on_count"] = bool(lit), len(lit)
                                room["color"] = blend_colors([l["color"] for l in lit])
                                if lit and room["grouped_light"] == "":
                                    room["bri"] = round(sum(l["bri"] for l in lit) / len(lit), 1)
                                changed = True
                    if old_light is not None:
                        for room in self.rooms:
                            for light in room["lights"]:
                                if light["id"] == rid:
                                    new_light = {"on": light["on"], "bri": light["bri"]}
                                    if new_light != old_light:
                                        transitions.append(("light", rid, old_light, new_light))
                                    break
                            else:
                                continue
                            break
                        for room in self.rooms:
                            if room["kind"] == "room" and room["id"] in old_rooms:
                                new_rs = {"on": room["on"], "bri": room["bri"]}
                                if new_rs != old_rooms[room["id"]]:
                                    transitions.append(("room", room["id"], old_rooms[room["id"]], new_rs))
                elif kind == "grouped_light":
                    for room in self.rooms:
                        if room["grouped_light"] == rid:
                            old_rs = {"on": room["on"], "bri": room["bri"]}
                            if "on" in changes:
                                room["on"] = changes["on"]
                            if "bri" in changes:
                                room["bri"] = changes["bri"]
                            new_rs = {"on": room["on"], "bri": room["bri"]}
                            changed = True
                            if new_rs != old_rs:
                                transitions.append(("grouped_light", rid, old_rs, new_rs))
                                if room["kind"] == "room" and valid_uuid(room["id"]):
                                    transitions.append(("room", room["id"], old_rs, new_rs))
            if changed:
                self.on_count = sum(1 for r in self.rooms if r["kind"] != "zone" for l in r["lights"] if l["on"])
                if transitions:
                    rules_fired = self.evaluate_rules(transitions, now_mono=now_mono, now_time=now_time)
        if changed:
            self.emit()
        if rules_fired:
            self.wake.set()
        return changed

    def handle_sse_line(self, line, generation=None, now_mono=None, now_time=None):
        updates = parse_sse_data_line(line)
        if updates:
            return self.apply_sse_updates(updates, generation=generation, now_mono=now_mono, now_time=now_time)
        return False

    def sse_loop(self):
        """Background daemon listening on /eventstream/clip/v2 with pinned TLS and exponential backoff."""
        backoff_idx = 0
        last_gen = -1
        while not self._stop.is_set():
            config, generation = self.connection()
            if generation != last_gen:
                backoff_idx = 0
                last_gen = generation
            if not config:
                self.sse_wake.wait(5.0)
                self.sse_wake.clear()
                continue
            pin = {"fingerprint": config.get("fingerprint"), "bridge_id": config.get("bridge_id", "")}
            conn = resp = sock = None
            connected_ok = False
            try:
                conn, resp, sock = open_sse_stream(config["ip"], config["app_key"], pin)
                with self.lock:
                    if not self.connection_current(generation) or self._stop.is_set():
                        continue
                    self._sse_sock = sock
                connected_ok = True
                backoff_idx = 0

                def read_chunk():
                    while not self._stop.is_set():
                        with self.lock:
                            if not self.connection_current(generation):
                                return b""
                        try:
                            return resp.read1(8192)
                        except socket.timeout:
                            continue

                for raw_line in iter_bounded_lines(read_chunk, max_line=MAX_BODY):
                    if self._stop.is_set():
                        break
                    with self.lock:
                        if not self.connection_current(generation):
                            break
                    self.handle_sse_line(raw_line, generation=generation)
            except (PinError, PermissionError):
                backoff_idx = len(SSE_BACKOFF) - 1
            except (OSError, http.client.HTTPException, ValueError):
                pass
            finally:
                with self.lock:
                    if self._sse_sock is sock:
                        self._sse_sock = None
                if resp is not None:
                    try:
                        resp.close()
                    except OSError:
                        pass
                if conn is not None:
                    try:
                        conn.close()
                    except OSError:
                        pass
                if sock is not None:
                    try:
                        sock.close()
                    except OSError:
                        pass
            if self._stop.is_set():
                break
            with self.lock:
                gen_changed = (self.conn_generation != last_gen) or (not self.config) or bool(self.pair_ip)
            if gen_changed:
                continue
            delay = SSE_BACKOFF[min(backoff_idx, len(SSE_BACKOFF) - 1)]
            if not connected_ok and backoff_idx < len(SSE_BACKOFF) - 1:
                backoff_idx += 1
            self.sse_wake.wait(delay)
            self.sse_wake.clear()

    def send_api_queue(self):
        """Send one queued schedule/bridge API call (POST/PUT/DELETE) and trigger refresh."""
        config, generation = self.connection()
        if not config:
            return False
        with self.lock:
            if not self.api_queue:
                return False
            method, path, body = self.api_queue.pop(0)
        try:
            self.api(method, path, body, config=config)
            with self.lock:
                if self.connection_current(generation):
                    self.last_refresh = 0.0
                    self.refresh_hold_until = 0.0
        except BridgeError:
            with self.lock:
                if self.connection_current(generation):
                    self.set_error("errCommand", "Command failed (%s)", "bridge error")
        except (OSError, PinError, PermissionError, socket.timeout, http.client.HTTPException) as exc:
            with self.lock:
                if self.connection_current(generation):
                    self.set_error("errCommand", "Command failed (%s)", self.safe(exc, 60))
        return True

    def send_pending(self):
        sent = False
        now = time.time()
        config, generation = self.connection()
        with self.lock:
            items = list(self.pending.items()) if config else []
        for (kind, rid), body in items:
            interval = GROUP_INTERVAL if kind == "grouped_light" else LIGHT_INTERVAL
            if now - self.last_sent.get(kind, 0) < interval:
                continue
            with self.lock:
                if not self.connection_current(generation):
                    break   # Unpair, Pair or a relocation since the batch started
                if self.pending.get((kind, rid)) is not body:
                    continue
                del self.pending[(kind, rid)]
            try:
                self.api("PUT", "/clip/v2/resource/%s/%s" % (kind, rid), body, config=config)
            except BridgeError:
                with self.lock:
                    if self.connection_current(generation):
                        self.set_error("errCommand", "Command failed (%s)", "bridge error")
            except (OSError, PinError, PermissionError, socket.timeout, http.client.HTTPException) as exc:
                with self.lock:
                    if self.connection_current(generation):
                        self.set_error("errCommand", "Command failed (%s)", self.safe(exc, 60))
            self.last_sent[kind] = time.time()
            sent = True
        return sent

    def handle(self, cmd):
        name = cmd.get("cmd")
        if name == "open":
            self.panel_open = True
            self.wake.set()
        elif name == "closed":
            self.panel_open = False
        elif name == "refresh":
            self.last_refresh = 0
            self.wake.set()
        elif name == "discover":
            self.start_discovery()
        elif name == "pair":
            ip = str(cmd.get("ip", "")).strip()
            if not valid_bridge_ip(ip):
                with self.lock:
                    self.set_error("errEnterIp", "Enter the bridge's local IPv4 address")
                self.emit()
                return
            with self.lock:
                self.pair_generation += 1
                self.conn_generation += 1
                self.pending = {}   # commands for the old connection are not sent any more
                if hasattr(self, "api_queue"):
                    self.api_queue = []
                self.pair_ip, self.pair_until, self.phase, self.error = ip, time.time() + PAIR_WINDOW, "pairing", ""
            self._reset_sse()
            self.emit()
            self.wake.set()
        elif name == "cancel_pair":
            with self.lock:
                self.pair_generation += 1
                self.pair_ip, self.phase = "", ("connecting" if self.config else "unpaired")
            self._reset_sse()
            self.emit()
        elif name == "unpair":
            # Stop synth before clearing state (outside lock to avoid join deadlock)
            if hasattr(self, "synth"):
                self.synth.stop(emit=False)
            # One lock for invalidating, deleting and clearing: try_pair validates and saves
            # under it too, so no pairing reply can save a key between deletion and clearing.
            with self.lock:
                self.pair_generation += 1
                self.conn_generation += 1
                removed = remove_config()
                self.pair_ip = ""
                self.config, self.rooms, self.light_count, self.on_count = {}, [], 0, 0
                self.bridge_home_gl, self.bridge_schedules = "", []
                self._local_sched_fired = {}
                self._rebuild_schedules()
                self.schedule_script_id = DEFAULT_SCHEDULE_SCRIPT_ID
                self.api_queue = []
                self.echo_suppress = {}
                self.rule_last_fired = {}
                self.phase, self.error, self.pending = "unpaired", "", {}
                if not removed:
                    # Otherwise the pairing would silently come back with the next start.
                    self.set_error("errInternal", "Something went wrong (%s)",
                                   "the saved pairing could not be deleted: " + CONFIG_PATH)
            self._reset_sse()
            self.emit()
            self.start_discovery()
        elif name in ("light", "group") and self.config:
            kind = "light" if name == "light" else "grouped_light"
            rid = cmd.get("id")
            if not valid_uuid(rid):
                return
            body = {}
            if isinstance(cmd.get("on"), bool):
                body["on"] = {"on": cmd["on"]}
            bri = cmd.get("bri")
            if isinstance(bri, (int, float)) and not isinstance(bri, bool):
                bri = finite(bri, None, 1.0, 100.0)
                if bri is None:
                    return
                body["dimming"] = {"brightness": round(bri, 1)}
                body["on"] = {"on": True}
            if "mirek" in cmd and cmd["mirek"] is not None:
                raw_m = cmd["mirek"]
                if isinstance(raw_m, bool) or not isinstance(raw_m, (int, float)):
                    return
                m = finite(raw_m, None, 153.0, 500.0)
                if m is None:
                    return
                body["color_temperature"] = {"mirek": int(round(m))}
                body["on"] = {"on": True}
            if "xy" in cmd and cmd["xy"] is not None:
                raw_xy = cmd["xy"]
                if not isinstance(raw_xy, (list, tuple)) or len(raw_xy) != 2:
                    return
                rx, ry = raw_xy[0], raw_xy[1]
                if isinstance(rx, bool) or isinstance(ry, bool) or not isinstance(rx, (int, float)) or not isinstance(ry, (int, float)):
                    return
                if not (math.isfinite(rx) and math.isfinite(ry) and 0.0 <= rx <= 1.0 and 0.0 <= ry <= 1.0):
                    return
                body["color"] = {"xy": {"x": round(float(rx), 4), "y": round(float(ry), 4)}}
                body["on"] = {"on": True}
            if "effect" in cmd and cmd["effect"] is not None:
                eff = cmd["effect"]
                if not isinstance(eff, str) or EFFECT_RE.match(eff) is None:
                    return
                body["effects"] = {"effect": eff}
                if eff != "no_effect":
                    body["on"] = {"on": True}
            if body:
                self.queue(kind, rid, body)
        elif name == "all_off" and self.config:
            self.all_off()
        elif name == "all_on" and self.config:
            self.all_on(cmd.get("exclude_rooms"))
        elif name == "scene" and self.config:
            rid = cmd.get("id")
            if valid_uuid(rid):
                with self.lock:
                    if not self.may_queue("scene", rid):
                        return
                    self.pending[("scene", rid)] = {"recall": {"action": "active"}}
                    self.refresh_hold_until = 0
                self.wake.set()
        elif name == "theme_save":
            theme = validate_theme(cmd.get("theme"))
            if theme is None:
                return
            with self.lock:
                if theme["id"] in BUILTIN_THEME_IDS:
                    return
                candidate = [t for t in self.custom_themes if t["id"] != theme["id"]]
                if len(candidate) >= MAX_THEMES:
                    return
                candidate.append(theme)
                save_themes(candidate)
                self.custom_themes = candidate
            self.emit()
        elif name == "theme_delete":
            tid = cmd.get("id")
            if not isinstance(tid, str) or tid in BUILTIN_THEME_IDS:
                return
            with self.lock:
                new_themes = [t for t in self.custom_themes if t["id"] != tid]
                if len(new_themes) == len(self.custom_themes):
                    return
                save_themes(new_themes)
                self.custom_themes = new_themes
            self.emit()
        elif name == "theme_apply" and self.config:
            theme_id = cmd.get("theme_id")
            light_id = cmd.get("light_id")
            if not isinstance(theme_id, str) or not valid_uuid(light_id):
                return
            body = None
            with self.lock:
                if not self.known("light", light_id):
                    return
                light = self._find_light(light_id)
                theme = next((t for t in (BUILTIN_THEMES + self.custom_themes) if t["id"] == theme_id), None)
                if light is None or theme is None:
                    return
                body = {
                    "on": {"on": True},
                    "dimming": {"brightness": round(clamp(float(theme["bri"]), 1.0, 100.0), 1)},
                }
                if theme.get("mirek") is not None and light.get("supports_ct"):
                    m_min = light.get("mirek_min", 153)
                    m_max = light.get("mirek_max", 500)
                    m = int(round(clamp(float(theme["mirek"]), float(m_min), float(m_max))))
                    body["color_temperature"] = {"mirek": m}
                elif light.get("supports_color"):
                    xy = hex_to_xy(theme.get("hex"))
                    if xy is not None:
                        body["color"] = {"xy": {"x": xy[0], "y": xy[1]}}
                eff = theme.get("effect") or "no_effect"
                avail = light.get("available_effects") or []
                if eff == "no_effect" or eff in avail:
                    body["effects"] = {"effect": eff}
            if body:
                self.queue("light", light_id, body)
        elif name == "schedule_mode":
            mode = cmd.get("mode")
            if mode not in SCHEDULE_MODES:
                return
            with self.lock:
                save_local_schedules({"mode": mode, "schedules": self.local_schedules})
                self.schedule_mode = mode
            self.emit()
        elif name == "schedule_create" and self.config:
            source = cmd.get("source") if cmd.get("source") in SCHEDULE_MODES else self.schedule_mode
            if source == "local":
                with self.lock:
                    if not self.known_target(cmd.get("target_kind"), cmd.get("target_id")):
                        return
                    if cmd.get("action") == "scene" and not self.known("scene", cmd.get("scene_id")):
                        return
                    if len(self.local_schedules) >= MAX_LOCAL_SCHEDULES:
                        return
                    raw_sched = {
                        "id": str(uuid.uuid4()),
                        "name": cmd.get("name"),
                        "enabled": cmd.get("enabled", True),
                        "target_kind": cmd.get("target_kind"),
                        "target_id": cmd.get("target_id"),
                        "target_name": self._target_name(cmd.get("target_kind"), cmd.get("target_id")) or cmd.get("target_name", ""),
                        "trigger_kind": "time",
                        "time": cmd.get("time"),
                        "offset_min": 0,
                        "days": cmd.get("days", []),
                        "action": cmd.get("action"),
                        "bri": cmd.get("bri"),
                        "scene_id": cmd.get("scene_id", ""),
                        "fade_min": cmd.get("fade_min", 0),
                        "source": "local",
                    }
                    sched = validate_local_schedule(raw_sched)
                    if sched is None:
                        return
                    candidate = list(self.local_schedules) + [sched]
                    save_local_schedules({"mode": self.schedule_mode, "schedules": candidate})
                    self.local_schedules = candidate
                    self._rebuild_schedules()
                self.emit()
                self.wake.set()
            else:
                with self.lock:
                    if not self.known_target(cmd.get("target_kind"), cmd.get("target_id")):
                        return
                    if cmd.get("action") == "scene" and not self.known("scene", cmd.get("scene_id")):
                        return
                    payload = build_schedule_payload(cmd, getattr(self, "schedule_script_id", DEFAULT_SCHEDULE_SCRIPT_ID))
                    if payload is None or len(self.api_queue) >= MAX_API_QUEUE:
                        return
                    self.api_queue.append(("POST", "/clip/v2/resource/behavior_instance", payload))
                self.wake.set()
        elif name == "schedule_toggle":
            sid = cmd.get("id")
            enabled = cmd.get("enabled")
            if not valid_uuid(sid) or not isinstance(enabled, bool):
                return
            with self.lock:
                local_target = next((s for s in self.local_schedules if s["id"] == sid), None)
                if local_target is not None:
                    candidate = [dict(s, enabled=enabled) if s["id"] == sid else s for s in self.local_schedules]
                    save_local_schedules({"mode": self.schedule_mode, "schedules": candidate})
                    self.local_schedules = candidate
                    self._rebuild_schedules()
                    handled_local = True
                else:
                    handled_local = False
                    if not self.config:
                        return
                    target = next((s for s in self.bridge_schedules if s["id"] == sid), None)
                    if target is None or len(self.api_queue) >= MAX_API_QUEUE:
                        return
                    target["enabled"] = enabled
                    self._rebuild_schedules()
                    self.api_queue.append(("PUT", "/clip/v2/resource/behavior_instance/%s" % sid, {"enabled": enabled}))
            self.emit()
            if not handled_local or enabled:
                self.wake.set()
        elif name == "schedule_delete":
            sid = cmd.get("id")
            if not valid_uuid(sid):
                return
            with self.lock:
                if any(s["id"] == sid for s in self.local_schedules):
                    candidate = [s for s in self.local_schedules if s["id"] != sid]
                    save_local_schedules({"mode": self.schedule_mode, "schedules": candidate})
                    self.local_schedules = candidate
                    self._local_sched_fired.pop(sid, None)
                    self._rebuild_schedules()
                    handled_local = True
                else:
                    handled_local = False
                    if not self.config:
                        return
                    if not any(s["id"] == sid for s in self.bridge_schedules) or len(self.api_queue) >= MAX_API_QUEUE:
                        return
                    self.bridge_schedules = [s for s in self.bridge_schedules if s["id"] != sid]
                    self._rebuild_schedules()
                    self.api_queue.append(("DELETE", "/clip/v2/resource/behavior_instance/%s" % sid, None))
            self.emit()
            if not handled_local:
                self.wake.set()
        elif name == "synth_config":
            with self.lock:
                changed = self.synth.configure(cmd)
            if changed:
                self.emit()
        elif name == "synth_start" and self.config:
            self.synth.start()
        elif name == "synth_stop":
            self.synth.stop(emit=True)
        elif name == "synth_toggle":
            if self.synth.running:
                self.synth.stop(emit=True)
            elif self.config:
                self.synth.start()
        elif name == "rule_save":
            rule = validate_rule(cmd.get("rule"))
            if rule is None:
                return
            with self.lock:
                candidate = [r for r in self.rules if r["id"] != rule["id"]]
                if len(candidate) >= MAX_RULES:
                    return
                candidate.append(rule)
                cycle_err = detect_rule_cycles(candidate)
                if cycle_err:
                    self.set_error("errRuleCycle", "Rule rejected (%s)", cycle_err)
                    rejected = True
                else:
                    rejected = False
                    save_rules({"paused": self.rules_paused, "rules": candidate})
                    self.rules = candidate
                    if self.error_code == "errRuleCycle":
                        self.error, self.error_code, self.error_args = "", "", []
            self.emit()
        elif name == "rule_delete":
            rid = cmd.get("id")
            if not valid_uuid(rid):
                return
            with self.lock:
                new_rules = [r for r in self.rules if r["id"] != rid]
                if len(new_rules) == len(self.rules):
                    return
                save_rules({"paused": self.rules_paused, "rules": new_rules})
                self.rules = new_rules
                self.rule_last_fired.pop(rid, None)
            self.emit()
        elif name == "rule_toggle":
            rid = cmd.get("id")
            enabled = cmd.get("enabled")
            if not valid_uuid(rid) or not isinstance(enabled, bool):
                return
            with self.lock:
                if not any(r["id"] == rid for r in self.rules):
                    return
                candidate = [dict(r, enabled=enabled) if r["id"] == rid else r for r in self.rules]
                if enabled:
                    cycle_err = detect_rule_cycles(candidate)
                    if cycle_err:
                        self.set_error("errRuleCycle", "Rule rejected (%s)", cycle_err)
                        self.emit()
                        return
                save_rules({"paused": self.rules_paused, "rules": candidate})
                self.rules = candidate
                if self.error_code == "errRuleCycle":
                    self.error, self.error_code, self.error_args = "", "", []
            self.emit()
        elif name == "rules_pause":
            paused = cmd.get("paused")
            if not isinstance(paused, bool):
                return
            with self.lock:
                save_rules({"paused": paused, "rules": self.rules})
                self.rules_paused = paused
            self.emit()
        elif name == "desktop_event" and self.config:
            event = cmd.get("event")
            if event not in ("wake", "sleep"):
                return
            with self.lock:
                fired = self.evaluate_rules([("desktop", event, {}, {"event": event})])
            if fired:
                self.emit()
                self.wake.set()

    # -- loops
    def worker(self):
        """One round at a time. Whatever goes wrong in a round (a bridge reply of a shape
        nobody expected) is shown as an error and the next round runs: the thread never dies
        while the helper process stays up, which the extension could not notice."""
        while True:
            with self.lock:
                generations = (self.pair_generation, self.conn_generation)
            try:
                self.work_once()
            except Exception as exc:   # noqa: BLE001 - the loop must survive anything
                with self.lock:
                    # Not after Unpair, Cancel or a new Pair: the failure belonged to the old state.
                    if generations == (self.pair_generation, self.conn_generation):
                        self.set_error("errInternal", "Something went wrong (%s)", self.safe(exc, 60))
                try:
                    self.emit()
                except OSError:
                    return
                time.sleep(2.0)

    def work_once(self):
        if self.pair_ip:
            with self.lock:
                expired = bool(self.pair_ip) and time.time() > self.pair_until   # a new Pair may have just started
                if expired:
                    self.pair_ip = ""
                    self.phase = "connecting" if self.config else "unpaired"
                    self.set_error("errNoPress", "No button press seen. Press the round button on the bridge, then try again.")
            if not expired:
                self.try_pair()
            self.emit()
            if self.pair_ip:
                self.wake.wait(2.0)
                self.wake.clear()
            return
        if self.config:
            self.check_local_schedules()
        if self.send_api_queue():
            self.emit()
            return
        if self.send_pending():
            with self.lock:
                has_more = bool(self.pending)
            if not has_more:
                # Let the bridge settle, then show the real state.
                self.last_refresh = time.time() - (POLL_OPEN if self.panel_open else POLL_CLOSED) + 1.2
            return
        with self.lock:
            has_more = bool(self.pending) or bool(self.api_queue)
        if has_more:
            time.sleep(0.05)
            return
        interval = POLL_OPEN if self.panel_open else POLL_CLOSED
        if self.config and time.time() - self.last_refresh >= interval:
            self.refresh()
            self.emit()
        elif not self.config and not self.candidates and not self.discovering and self.panel_open:
            self.discover()
        with self.lock:
            has_local_sched = any(s.get("enabled") for s in self.local_schedules)
        wait = max(0.05, interval - (time.time() - self.last_refresh)) if self.config else 5.0
        if self.config and has_local_sched:
            wait = min(wait, LOCAL_SCHEDULE_POLL_CAP)
        self.wake.wait(wait)
        self.wake.clear()

    def run(self):
        self.emit()
        threading.Thread(target=self.worker, daemon=True).start()
        threading.Thread(target=self.sse_loop, daemon=True).start()
        try:
            for cmd in read_commands(sys.stdin.buffer):
                try:
                    self.handle(cmd)
                except Exception as exc:   # noqa: BLE001 - one bad command must not end the helper
                    with self.lock:
                        self.set_error("errInternal", "Something went wrong (%s)", self.safe(exc, 60))
                    self.emit()
        finally:
            self._stop.set()
            if hasattr(self, "synth"):
                self.synth.stop(emit=False)
            self._reset_sse()


def _terminate(signum, _frame):
    sys.exit(128 + signum)   # unwinds the main thread; atexit then ends the children


def main():
    atexit.register(kill_children)
    signal.signal(signal.SIGTERM, _terminate)
    if "--once" in sys.argv:
        hue = Hue()
        hue.refresh()
        print(json.dumps(hue.snapshot(), indent=1))
        return
    try:
        Hue().run()
    except (KeyboardInterrupt, BrokenPipeError):
        pass


if __name__ == "__main__":
    main()
