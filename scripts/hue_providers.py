#!/usr/bin/env python3
"""Multi-device local-first lighting providers for Maprisma Hue Lights & Omarchy Hue Lights.

Pure Python standard library only (zero external dependencies, Python 3.9+ compatible).
Supports 8 additional local smart-lighting ecosystems alongside Philips Hue Bridge:
  - Home Assistant (REST /api/states + /api/template areas + /api/services)
  - WLED (HTTP /json, /json/state, /presets.json)
  - WiZ (UDP port 38899 JSON getPilot / setPilot + built-in scenes)
  - LIFX (UDP port 56700 LAN v2 binary protocol HSBK)
  - Govee LAN (UDP port 4003 control + port 4002 devStatus replies)
  - Nanoleaf (HTTP port 16021 OpenAPI + 5-second power-button pairing + effects)
  - Shelly (HTTP Gen2/Gen3 RPC + Gen1 REST fallback)
  - IKEA DIRIGERA (HTTPS port 8443 + TLS TOFU pinning + OAuth2 PKCE button pairing)

Security:
  - Local LAN only (RFC1918 / link-local excluding 169.254.169.254 / .local; loopback only for HA).
  - Resolves .local hostnames to IPv4 BEFORE connecting, validates the resolved IP, and connects
    directly to that pinned IP (prevents DNS rebinding SSRF).
  - Hard wall-clock deadline on every HTTP/UDP request (prevents slowloris hangs).
  - HTTPS connections pin the leaf certificate SHA-256 fingerprint on first connect/pair (TOFU)
    and fail closed on certificate mismatch.
  - `providers.json` opened with O_NOFOLLOW + fstat owner/0600 verification; secrets never emitted
    in snapshots, pairing replies, or error strings.
  - Dedicated bounded coalesced background worker for extra-provider sends and refreshes.
"""

import base64
import copy
import hashlib
import http.client
import ipaddress
import json
import math
import os
import re
import secrets
import socket
import ssl
import stat
import struct
import threading
import time
import urllib.parse
import uuid

MAX_BODY = 2 * 1024 * 1024
MAX_HA_BODY = 8 * 1024 * 1024
MAX_PROVIDERS = 32
MAX_PENDING_EXTRA = 256
DEFAULT_WARM_HEX = "#ffd27a"
METADATA_IP = "169.254.169.254"

PROVIDER_KINDS = {
    "home_assistant": {
        "id": "home_assistant",
        "prefix": "ha",
        "name": "Home Assistant",
        "badge": "HA",
        "default_port": 8123,
        "bonjour": "_home-assistant._tcp",
        "default_https": False,
        "requires_token": True,
        "supports_button_pairing": False,
        "allows_loopback": True,
        "is_hub": True,
    },
    "wled": {
        "id": "wled",
        "prefix": "wled",
        "name": "WLED",
        "badge": "WLED",
        "default_port": 80,
        "bonjour": "_wled._tcp",
        "default_https": False,
        "requires_token": False,
        "supports_button_pairing": False,
        "allows_loopback": False,
        "is_hub": False,
    },
    "wiz": {
        "id": "wiz",
        "prefix": "wiz",
        "name": "WiZ",
        "badge": "WiZ",
        "default_port": 38899,
        "bonjour": None,
        "default_https": False,
        "requires_token": False,
        "supports_button_pairing": False,
        "allows_loopback": False,
        "is_hub": False,
    },
    "lifx": {
        "id": "lifx",
        "prefix": "lifx",
        "name": "LIFX",
        "badge": "LIFX",
        "default_port": 56700,
        "bonjour": None,
        "default_https": False,
        "requires_token": False,
        "supports_button_pairing": False,
        "allows_loopback": False,
        "is_hub": False,
    },
    "govee": {
        "id": "govee",
        "prefix": "govee",
        "name": "Govee LAN",
        "badge": "Govee",
        "default_port": 4003,
        "bonjour": None,
        "default_https": False,
        "requires_token": False,
        "supports_button_pairing": False,
        "allows_loopback": False,
        "is_hub": False,
    },
    "nanoleaf": {
        "id": "nanoleaf",
        "prefix": "nanoleaf",
        "name": "Nanoleaf",
        "badge": "Nanoleaf",
        "default_port": 16021,
        "bonjour": "_nanoleafapi._tcp",
        "default_https": False,
        "requires_token": True,
        "supports_button_pairing": True,
        "allows_loopback": False,
        "is_hub": False,
    },
    "shelly": {
        "id": "shelly",
        "prefix": "shelly",
        "name": "Shelly",
        "badge": "Shelly",
        "default_port": 80,
        "bonjour": "_shelly._tcp",
        "default_https": False,
        "requires_token": False,
        "supports_button_pairing": False,
        "allows_loopback": False,
        "is_hub": False,
    },
    "dirigera": {
        "id": "dirigera",
        "prefix": "dirigera",
        "name": "IKEA DIRIGERA",
        "badge": "IKEA",
        "default_port": 8443,
        "bonjour": "_ihsp._tcp",
        "default_https": True,
        "requires_token": True,
        "supports_button_pairing": True,
        "allows_loopback": False,
        "is_hub": True,
    },
}

PREFIX_TO_KIND = {meta["prefix"]: kind for kind, meta in PROVIDER_KINDS.items()}

UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
NS_ID_RE = re.compile(r"^[a-z][a-z0-9_]{1,15}:[A-Za-z0-9._:@/-]{1,140}$")
LOCAL_LABEL_RE = re.compile(r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$")
SHA256_HEX_RE = re.compile(r"^[0-9a-f]{64}$")

WIZ_SCENES = [
    (6, "Cozy"), (11, "Warm White"), (12, "Daylight"), (1, "Ocean"),
    (2, "Romance"), (3, "Sunset"), (4, "Party"), (5, "Fireplace"),
    (7, "Forest"), (14, "Night Light"), (15, "Focus"), (16, "Relax"),
]


class ProviderError(Exception):
    pass


# --------------------------------------------------------------------------- Validation & Color Math

def clamp(value, low, high):
    return max(low, min(high, value))


def finite(value, default, low, high):
    if isinstance(value, bool) or value is None:
        return default
    try:
        num = float(value)
    except (TypeError, ValueError):
        return default
    if not math.isfinite(num):
        return default
    return clamp(num, low, high)


def clean(text, limit=60):
    if not isinstance(text, str):
        text = str(text or "")
    out = "".join(ch for ch in text if ord(ch) >= 0x20 and ord(ch) != 0x7F and ch not in "<>&")
    return out.strip()[:limit]


def clean_token(raw, limit=512):
    if not isinstance(raw, str):
        return ""
    # Strip CR/LF and all ASCII control characters so HTTP header injection is impossible.
    return "".join(ch for ch in raw if ord(ch) >= 0x20 and ord(ch) != 0x7F).strip()[:limit]


def valid_uuid(value):
    return isinstance(value, str) and bool(UUID_RE.match(value))


def valid_resource_id(value):
    if not isinstance(value, str) or not value or len(value) > 160:
        return False
    if valid_uuid(value):
        return True
    if ".." in value:
        return False
    return bool(NS_ID_RE.match(value))


def provider_prefix(rid):
    if not isinstance(rid, str) or valid_uuid(rid) or ":" not in rid:
        return "hue"
    prefix = rid.split(":", 1)[0].lower()
    return prefix if prefix in PREFIX_TO_KIND else "hue"


def is_extra_resource(rid):
    return provider_prefix(rid) != "hue"


def _is_allowed_ipv4(ip_str, allow_loopback=False):
    if ip_str == METADATA_IP:
        return False
    try:
        ip = ipaddress.ip_address(ip_str)
    except ValueError:
        return False
    if ip.version != 4:
        return False
    if ip.is_loopback:
        return bool(allow_loopback)
    return (ip.is_private or ip.is_link_local) and not ip.is_multicast and not ip.is_unspecified


def valid_local_host(host, allow_loopback=False):
    if not isinstance(host, str):
        return False
    h = host.strip().lower()
    if not h or len(h) > 253 or any(c in h for c in "/\\ \t\r\n@?"):
        return False
    if allow_loopback and h in ("localhost", "127.0.0.1"):
        return True
    if _is_allowed_ipv4(h, allow_loopback=allow_loopback):
        return True
    if h.endswith(".local") and ".." not in h and not h.startswith("."):
        labels = h.split(".")
        return len(labels) >= 2 and all(LOCAL_LABEL_RE.match(lbl) for lbl in labels)
    return False


def resolve_local_ipv4(host, port, allow_loopback=False):
    """Validate and resolve a local host to a concrete allowed IPv4 address (prevents DNS rebinding)."""
    if not valid_local_host(host, allow_loopback=allow_loopback):
        raise ProviderError("Only local network addresses (LAN IP or .local) are allowed.")
    h = host.strip().lower()
    if _is_allowed_ipv4(h, allow_loopback=allow_loopback):
        return h
    if allow_loopback and h == "localhost":
        return "127.0.0.1"
    try:
        infos = socket.getaddrinfo(h, int(port), socket.AF_INET, socket.SOCK_STREAM)
    except OSError as exc:
        raise ProviderError(f"Could not resolve local host: {clean(str(exc), 80)}") from exc
    for info in infos:
        ip_str = info[4][0]
        if _is_allowed_ipv4(ip_str, allow_loopback=allow_loopback):
            return ip_str
    raise ProviderError("Host resolved to a non-local address and was blocked.")


def _slug(raw):
    out = re.sub(r"[^a-z0-9]+", "_", str(raw or "").lower()).strip("_")
    return (out or "lights")[:32]


def _srgb_gamma(c):
    c = clamp(c, 0.0, 1.0)
    return 12.92 * c if c <= 0.0031308 else 1.055 * (c ** (1.0 / 2.4)) - 0.055


def rgb_to_hex(r, g, b):
    return "#%02x%02x%02x" % (int(clamp(round(r), 0, 255)), int(clamp(round(g), 0, 255)), int(clamp(round(b), 0, 255)))


def hex_to_rgb(hex_str):
    if not isinstance(hex_str, str) or len(hex_str) != 7 or not hex_str.startswith("#"):
        return None
    try:
        v = int(hex_str[1:], 16)
        return ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
    except ValueError:
        return None


def xy_to_rgb(x, y):
    try:
        x, y = float(x), float(y)
    except (TypeError, ValueError):
        return (255, 210, 122)
    if not (math.isfinite(x) and math.isfinite(y) and 0.0 <= x <= 1.0 and 0.0 < y <= 1.0):
        return (255, 210, 122)
    z = 1.0 - x - y
    big_x = (1.0 / y) * x
    big_z = (1.0 / y) * z
    r = big_x * 1.656492 - 0.354851 - big_z * 0.255038
    g = -big_x * 0.707196 + 1.655397 + big_z * 0.036152
    b = big_x * 0.051713 - 0.121364 + big_z * 1.011530
    peak = max(r, g, b)
    if peak > 1.0:
        r, g, b = r / peak, g / peak, b / peak
    return (
        int(clamp(round(_srgb_gamma(r) * 255.0), 0, 255)),
        int(clamp(round(_srgb_gamma(g) * 255.0), 0, 255)),
        int(clamp(round(_srgb_gamma(b) * 255.0), 0, 255)),
    )


def xy_to_hex(x, y):
    r, g, b = xy_to_rgb(x, y)
    return rgb_to_hex(r, g, b)


def rgb_to_hsb(r, g, b):
    rf, gf, bf = clamp(r, 0, 255) / 255.0, clamp(g, 0, 255) / 255.0, clamp(b, 0, 255) / 255.0
    mx, mn = max(rf, gf, bf), min(rf, gf, bf)
    delta = mx - mn
    hue = 0.0
    if delta > 1e-5:
        if mx == rf:
            hue = 60.0 * (((gf - bf) / delta) % 6.0)
        elif mx == gf:
            hue = 60.0 * (((bf - rf) / delta) + 2.0)
        else:
            hue = 60.0 * (((rf - gf) / delta) + 4.0)
        if hue < 0:
            hue += 360.0
    sat = (delta / mx) * 100.0 if mx > 0 else 0.0
    return (hue, sat, mx * 100.0)


def hsb_to_hex(hue, sat, bri=100.0):
    h = float(hue) % 360.0
    s = clamp(float(sat), 0.0, 100.0) / 100.0
    v = clamp(float(bri), 0.0, 100.0) / 100.0
    c = v * s
    x = c * (1.0 - abs(((h / 60.0) % 2.0) - 1.0))
    m = v - c
    if h < 60:
        r1, g1, b1 = c, x, 0.0
    elif h < 120:
        r1, g1, b1 = x, c, 0.0
    elif h < 180:
        r1, g1, b1 = 0.0, c, x
    elif h < 240:
        r1, g1, b1 = 0.0, x, c
    elif h < 300:
        r1, g1, b1 = x, 0.0, c
    else:
        r1, g1, b1 = c, 0.0, x
    return rgb_to_hex((r1 + m) * 255.0, (g1 + m) * 255.0, (b1 + m) * 255.0)


def mirek_to_kelvin(mirek):
    try:
        m = int(round(float(mirek)))
    except (TypeError, ValueError):
        return 2700
    return int(round(1_000_000.0 / m)) if m > 0 else 2700


def kelvin_to_mirek(kelvin):
    try:
        k = int(round(float(kelvin)))
    except (TypeError, ValueError):
        return 370
    return int(clamp(round(1_000_000.0 / k), 153, 500)) if k > 0 else 370


def mirek_to_hex(mirek):
    k = clamp(mirek_to_kelvin(mirek), 1000, 40000) / 100.0
    if k <= 66.0:
        r = 255.0
        g = 99.4708025861 * math.log(k) - 161.1195681661
        b = 0.0 if k <= 19.0 else 138.5177312231 * math.log(k - 10.0) - 305.0447927307
    else:
        r = 329.698727446 * ((k - 60.0) ** -0.1332047592)
        g = 288.1221695283 * ((k - 60.0) ** -0.0755148492)
        b = 255.0
    return rgb_to_hex(r, g, b)


def blend_colors(colors):
    rs = gs = bs = count = 0
    for c in colors:
        rgb = hex_to_rgb(c)
        if rgb:
            rs += rgb[0]
            gs += rgb[1]
            bs += rgb[2]
            count += 1
    if not count:
        return DEFAULT_WARM_HEX
    return rgb_to_hex(rs / count, gs / count, bs / count)


def sanitize_connection(raw):
    if not isinstance(raw, dict):
        return None
    kind = str(raw.get("kind") or "").strip().lower()
    if kind not in PROVIDER_KINDS:
        return None
    meta = PROVIDER_KINDS[kind]
    host = str(raw.get("host") or "").strip()
    if not valid_local_host(host, allow_loopback=meta["allows_loopback"]):
        return None
    cid = re.sub(r"[^a-z0-9_-]+", "", str(raw.get("id") or "").lower())[:24]
    if not cid:
        cid = uuid.uuid4().hex[:12]
    name = clean(raw.get("name") or meta["name"], 40) or meta["name"]
    raw_port = raw.get("port")
    try:
        port = int(raw_port) if raw_port not in (None, "") else meta["default_port"]
    except (TypeError, ValueError):
        port = meta["default_port"]
    if not (1 <= port <= 65535):
        port = meta["default_port"]
    use_https = bool(raw.get("use_https", meta["default_https"]))
    if kind == "dirigera":
        use_https = True
    elif kind != "home_assistant":
        use_https = False
    token = clean_token(raw.get("token") or "")
    room_name = clean(raw.get("room_name") or "", 40)
    device_id = clean(raw.get("device_id") or "", 64)
    fp = str(raw.get("cert_fingerprint") or "").strip().lower()
    cert_fp = fp if SHA256_HEX_RE.match(fp) else ""
    return {
        "id": cid,
        "kind": kind,
        "name": name,
        "host": host,
        "port": port,
        "use_https": use_https,
        "enabled": bool(raw.get("enabled", True)),
        "room_name": room_name,
        "device_id": device_id,
        "token": token,
        "cert_fingerprint": cert_fp,
    }


# --------------------------------------------------------------------------- Local-Only Transport

def _http_request(
    host,
    port,
    method,
    path,
    body=None,
    headers=None,
    use_https=False,
    timeout=4.5,
    allow_loopback=False,
    max_body=MAX_BODY,
    expected_fp="",
    capture_fp=None,
):
    """Send one bounded local HTTP/HTTPS request with DNS-rebinding guard, wall-clock deadline,
    and TOFU TLS certificate fingerprint verification."""
    ip = resolve_local_ipv4(host, port, allow_loopback=allow_loopback)
    deadline = time.monotonic() + float(timeout)
    hdrs = dict(headers or {})
    hdrs.setdefault("Host", f"{host}:{port}")
    payload = None
    if body is not None:
        if isinstance(body, (bytes, bytearray)):
            payload = bytes(body)
        elif isinstance(body, str):
            payload = body.encode("utf-8")
        else:
            payload = json.dumps(body, separators=(",", ":")).encode("utf-8")
            hdrs.setdefault("Content-Type", "application/json")

    if use_https:
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
        conn = http.client.HTTPSConnection(ip, int(port), timeout=timeout, context=ctx)
    else:
        conn = http.client.HTTPConnection(ip, int(port), timeout=timeout)

    try:
        conn.connect()
        if use_https:
            der = conn.sock.getpeercert(binary_form=True)
            if not der:
                raise ProviderError("Device presented no TLS certificate.")
            seen_fp = hashlib.sha256(der).hexdigest()
            if expected_fp and seen_fp != expected_fp:
                raise ProviderError("TLS certificate fingerprint changed — possible MITM or new hub.")
            if isinstance(capture_fp, list):
                capture_fp[:] = [seen_fp]
        rem = deadline - time.monotonic()
        if rem <= 0:
            raise ProviderError("Request timed out.")
        conn.sock.settimeout(rem)
        conn.request(method, path, body=payload, headers=hdrs)
        resp = conn.getresponse()
        chunks = []
        total = 0
        while True:
            rem = deadline - time.monotonic()
            if rem <= 0:
                raise ProviderError("Response timed out.")
            if conn.sock is not None:
                conn.sock.settimeout(rem)
            chunk = resp.read(min(16384, max_body - total + 1))
            if not chunk:
                break
            total += len(chunk)
            if total > max_body:
                raise ProviderError("Response too large.")
            chunks.append(chunk)
        return resp.status, b"".join(chunks)
    finally:
        try:
            conn.close()
        except OSError:
            pass


def _udp_exchange(host, port, payload, expect_reply=True, timeout=2.0, listen_port=0):
    """Send one UDP datagram to a resolved local IPv4 and accept replies ONLY from that IP."""
    target_ip = resolve_local_ipv4(host, port, allow_loopback=False)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        if listen_port:
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(("", int(listen_port)))
        sock.sendto(payload, (target_ip, int(port)))
        if not expect_reply:
            return None
        deadline = time.monotonic() + float(timeout)
        while True:
            rem = deadline - time.monotonic()
            if rem <= 0:
                return None
            sock.settimeout(rem)
            try:
                data, addr = sock.recvfrom(65535)
            except socket.timeout:
                return None
            if addr and addr[0] == target_ip:
                return data
    finally:
        sock.close()


# --------------------------------------------------------------------------- Pure Parsers & Codecs

def _make_light(lid, name, is_on, bri, color, provider, device_id="", dimmable=True, reachable=True,
                supports_color=True, supports_ct=True, mirek=None, mirek_min=153, mirek_max=500,
                effect="no_effect", available_effects=None):
    return {
        "id": lid,
        "device_id": device_id,
        "name": clean(name or "Light", 50) or "Light",
        "on": bool(is_on),
        "bri": round(float(clamp(bri, 0.0, 100.0)), 1),
        "dimmable": bool(dimmable),
        "color": color if hex_to_rgb(color) else DEFAULT_WARM_HEX,
        "reachable": bool(reachable),
        "supports_color": bool(supports_color),
        "supports_ct": bool(supports_ct),
        "mirek": mirek,
        "mirek_min": int(min(mirek_min, mirek_max)),
        "mirek_max": int(max(mirek_min, mirek_max)),
        "effect": effect or "no_effect",
        "available_effects": list(available_effects or []),
        "motion_aware": False,
        "motion_active": False,
        "provider": provider,
    }


def _make_room(rid, name, lights, provider, scenes=None, archetype=""):
    on_lights = [l for l in lights if l["on"]]
    avg_bri = round(sum(l["bri"] for l in on_lights) / len(on_lights), 1) if on_lights else 0.0
    return {
        "id": rid,
        "kind": "room",
        "name": clean(name or "Room", 40) or "Room",
        "grouped_light": "",
        "on": bool(on_lights),
        "bri": avg_bri,
        "on_count": len(on_lights),
        "archetype": archetype or provider,
        "color": blend_colors([l["color"] for l in on_lights]),
        "lights": lights,
        "scenes": list(scenes or []),
        "provider": provider,
    }


# 1. Home Assistant
def parse_home_assistant_states(conn, states, entity_areas=None):
    entity_areas = entity_areas or {}
    cid = conn["id"]
    fallback_room = conn.get("room_name") or conn["name"]
    lights_by_room = {}
    scenes = []
    sensors = []

    for item in (states if isinstance(states, list) else []):
        if not isinstance(item, dict):
            continue
        eid = str(item.get("entity_id") or "")
        if not eid:
            continue
        st = str(item.get("state") or "").lower()
        attrs = item.get("attributes") if isinstance(item.get("attributes"), dict) else {}
        friendly = clean(attrs.get("friendly_name") or eid, 50) or eid

        if eid.startswith("light."):
            reachable = st not in ("unavailable", "unknown")
            is_on = st == "on"
            raw_bri = finite(attrs.get("brightness"), None, 0.0, 255.0)
            bri = round((raw_bri / 255.0) * 100.0, 1) if raw_bri is not None else (100.0 if is_on else 0.0)
            modes = {str(m).lower() for m in (attrs.get("supported_color_modes") or []) if isinstance(m, str)}
            supports_color = bool(modes & {"rgb", "rgbw", "rgbww", "hs", "xy"}) or any(
                attrs.get(k) is not None for k in ("rgb_color", "hs_color", "xy_color")
            )
            supports_ct = ("color_temp" in modes) or any(
                attrs.get(k) is not None for k in ("color_temp_kelvin", "color_temp", "min_color_temp_kelvin")
            )
            dimmable = (modes != {"onoff"}) if modes else True
            min_k = int(finite(attrs.get("min_color_temp_kelvin"), 2000, 1500, 9000))
            max_k = int(finite(attrs.get("max_color_temp_kelvin"), 6500, 1500, 9000))
            mirek = None
            if attrs.get("color_temp_kelvin") is not None:
                k_val = finite(attrs.get("color_temp_kelvin"), None, 1000, 10000)
                if k_val:
                    mirek = kelvin_to_mirek(k_val)
            elif attrs.get("color_temp") is not None:
                m_val = finite(attrs.get("color_temp"), None, 100, 600)
                if m_val:
                    mirek = int(clamp(round(m_val), 153, 500))

            color_hex = DEFAULT_WARM_HEX
            rgb = attrs.get("rgb_color")
            xy = attrs.get("xy_color")
            hs = attrs.get("hs_color")
            if isinstance(rgb, list) and len(rgb) >= 3:
                color_hex = rgb_to_hex(finite(rgb[0], 255, 0, 255), finite(rgb[1], 210, 0, 255), finite(rgb[2], 122, 0, 255))
            elif isinstance(xy, list) and len(xy) >= 2:
                color_hex = xy_to_hex(xy[0], xy[1])
            elif isinstance(hs, list) and len(hs) >= 2:
                color_hex = hsb_to_hex(finite(hs[0], 0, 0, 360), finite(hs[1], 0, 0, 100), 100.0)
            elif mirek is not None:
                color_hex = mirek_to_hex(mirek)

            fx_list = [clean(x, 32) for x in (attrs.get("effect_list") or [])[:20] if isinstance(x, str) and clean(x, 32)]
            active_fx = clean(attrs.get("effect") or "", 32) or "no_effect"

            light = _make_light(
                lid=f"ha:{cid}:{eid}",
                name=friendly,
                is_on=is_on,
                bri=bri,
                color=color_hex,
                provider="home_assistant",
                device_id=eid,
                dimmable=dimmable,
                reachable=reachable,
                supports_color=supports_color,
                supports_ct=supports_ct,
                mirek=mirek,
                mirek_min=kelvin_to_mirek(max(min_k, max_k)),
                mirek_max=kelvin_to_mirek(min(min_k, max_k)),
                effect=active_fx,
                available_effects=fx_list,
            )
            area = clean(entity_areas.get(eid) or attrs.get("area_name") or "", 40) or fallback_room
            lights_by_room.setdefault(area, []).append(light)

        elif eid.startswith("scene."):
            scenes.append({
                "id": f"ha:{cid}:{eid}",
                "name": clean(friendly, 30) or eid,
                "active": False,
                "provider": "home_assistant",
            })

        elif eid.startswith("binary_sensor."):
            dc = str(attrs.get("device_class") or "").lower()
            area = clean(entity_areas.get(eid) or attrs.get("area_name") or "", 40) or fallback_room
            room_id = f"ha:{cid}:room:{_slug(area)}"
            if dc in ("motion", "occupancy", "presence"):
                sensors.append({
                    "id": f"ha:{cid}:{eid}",
                    "device_id": eid,
                    "kind": "motion",
                    "name": friendly,
                    "room_id": room_id,
                    "enabled": st != "unavailable",
                    "motion": st == "on",
                    "open": False,
                    "temperature": None,
                    "lux": None,
                    "battery": None,
                    "battery_state": "",
                    "sensitivity": None,
                    "sensitivity_max": None,
                    "button_ids": [],
                    "last_event": "",
                    "provider": "home_assistant",
                })
            elif dc in ("door", "window", "opening", "garage_door"):
                sensors.append({
                    "id": f"ha:{cid}:{eid}",
                    "device_id": eid,
                    "kind": "contact",
                    "name": friendly,
                    "room_id": room_id,
                    "enabled": st != "unavailable",
                    "motion": False,
                    "open": st == "on",
                    "temperature": None,
                    "lux": None,
                    "battery": None,
                    "battery_state": "",
                    "sensitivity": None,
                    "sensitivity_max": None,
                    "button_ids": [],
                    "last_event": "",
                    "provider": "home_assistant",
                })

    scenes.sort(key=lambda s: s["name"].lower())
    rooms = []
    used_rids = set()
    for idx, rname in enumerate(sorted(lights_by_room.keys(), key=str.lower)):
        members = sorted(lights_by_room[rname], key=lambda l: l["name"].lower())
        base_rid = f"ha:{cid}:room:{_slug(rname)}"
        rid = base_rid
        suffix = 2
        while rid in used_rids:
            rid = f"{base_rid}_{suffix}"
            suffix += 1
        used_rids.add(rid)
        rooms.append(_make_room(
            rid=rid,
            name=rname,
            lights=members,
            provider="home_assistant",
            scenes=scenes[:24] if idx == 0 else [],
        ))
    return rooms, sensors


# 2. WLED
def parse_wled(conn, data, presets=None):
    cid = conn["id"]
    state = data.get("state") if isinstance(data.get("state"), dict) else {}
    info = data.get("info") if isinstance(data.get("info"), dict) else {}
    effects = [str(e) for e in (data.get("effects") or []) if isinstance(e, str)]
    name = clean(info.get("name") or conn["name"], 40) or conn["name"]
    room_name = conn.get("room_name") or name
    is_on = bool(state.get("on", False))
    bri = round((finite(state.get("bri"), 128, 0, 255) / 255.0) * 100.0, 1)
    active_ps = int(finite(state.get("ps"), -1, -1, 1000))
    color_hex = DEFAULT_WARM_HEX
    active_fx = "no_effect"
    segs = state.get("seg") if isinstance(state.get("seg"), list) else []
    if segs and isinstance(segs[0], dict):
        cols = segs[0].get("col")
        if isinstance(cols, list) and cols and isinstance(cols[0], list) and len(cols[0]) >= 3:
            color_hex = rgb_to_hex(finite(cols[0][0], 255, 0, 255), finite(cols[0][1], 210, 0, 255), finite(cols[0][2], 122, 0, 255))
        raw_fx = segs[0].get("fx")
        if isinstance(raw_fx, int) and not isinstance(raw_fx, bool) and 0 < raw_fx < len(effects):
            active_fx = clean(effects[raw_fx], 32) or "no_effect"
    curated_fx = [clean(e, 32) for e in effects[1:17] if clean(e, 32)]
    scenes = []
    if isinstance(presets, dict):
        for k, val in presets.items():
            if not isinstance(val, dict):
                continue
            try:
                pid = int(k)
            except (TypeError, ValueError):
                continue
            if pid <= 0:
                continue
            pname = clean(val.get("n") or "", 30)
            if pname:
                scenes.append({"id": f"wled:{cid}:ps:{pid}", "name": pname, "active": pid == active_ps, "provider": "wled"})
    scenes.sort(key=lambda s: s["name"].lower())
    light = _make_light(
        lid=f"wled:{cid}:0",
        name=name,
        is_on=is_on,
        bri=bri,
        color=color_hex,
        provider="wled",
        device_id=clean(info.get("mac") or "", 32),
        effect=active_fx,
        available_effects=curated_fx,
    )
    return [_make_room(f"wled:{cid}:room", room_name, [light], "wled", scenes=scenes)], [], effects


def build_wled_payload(body, effects_list=None):
    payload = {}
    if "on" in body and isinstance(body["on"], dict):
        payload["on"] = bool(body["on"].get("on"))
    if "dimming" in body and isinstance(body["dimming"], dict):
        bri = finite(body["dimming"].get("brightness"), 100.0, 1.0, 100.0)
        payload["bri"] = int(clamp(round(bri * 2.55), 1, 255))
    seg0 = {}
    if "color" in body and isinstance(body["color"], dict) and isinstance(body["color"].get("xy"), dict):
        xy = body["color"]["xy"]
        r, g, b = xy_to_rgb(xy.get("x"), xy.get("y"))
        seg0["col"] = [[r, g, b]]
    elif "color_temperature" in body and isinstance(body["color_temperature"], dict):
        m = body["color_temperature"].get("mirek")
        rgb = hex_to_rgb(mirek_to_hex(m))
        if rgb:
            seg0["col"] = [list(rgb)]
    if "effects" in body and isinstance(body["effects"], dict):
        eff = str(body["effects"].get("effect") or "")
        if not eff or eff == "no_effect":
            seg0["fx"] = 0
        elif effects_list:
            for i, name in enumerate(effects_list):
                if clean(name, 32).lower() == eff.lower():
                    seg0["fx"] = i
                    break
    if seg0:
        payload["seg"] = [seg0]
    return payload


# 3. WiZ
def parse_wiz_pilot(conn, data):
    cid = conn["id"]
    res = data.get("result") if isinstance(data.get("result"), dict) else data
    is_on = bool(res.get("state", False))
    bri = finite(res.get("dimming"), 100.0 if is_on else 0.0, 0.0, 100.0)
    temp_k = res.get("temp")
    mirek = kelvin_to_mirek(temp_k) if temp_k is not None else None
    active_scene = int(finite(res.get("sceneId"), 0, 0, 100))
    r, g, b = res.get("r"), res.get("g"), res.get("b")
    if all(isinstance(v, (int, float)) for v in (r, g, b)) and (r or g or b):
        color_hex = rgb_to_hex(r, g, b)
    elif mirek is not None:
        color_hex = mirek_to_hex(mirek)
    else:
        color_hex = DEFAULT_WARM_HEX
    light = _make_light(
        lid=f"wiz:{cid}:light",
        name=conn["name"],
        is_on=is_on,
        bri=bri,
        color=color_hex,
        provider="wiz",
        device_id=clean(res.get("mac") or conn.get("device_id") or "", 32),
        mirek=mirek,
        mirek_min=154,
        mirek_max=454,
    )
    scenes = [
        {"id": f"wiz:{cid}:scene:{sid}", "name": sname, "active": sid == active_scene, "provider": "wiz"}
        for sid, sname in WIZ_SCENES
    ]
    room_name = conn.get("room_name") or conn["name"]
    return [_make_room(f"wiz:{cid}:room", room_name, [light], "wiz", scenes=scenes)], []


def build_wiz_set_pilot(body=None, scene_id=None):
    body = body or {}
    params = {}
    if "on" in body and isinstance(body["on"], dict):
        params["state"] = bool(body["on"].get("on"))
    if "dimming" in body and isinstance(body["dimming"], dict):
        params["dimming"] = int(clamp(round(finite(body["dimming"].get("brightness"), 100, 10, 100)), 10, 100))
    if scene_id is not None:
        params["state"] = True
        params["sceneId"] = int(scene_id)
    elif "color" in body and isinstance(body["color"], dict) and isinstance(body["color"].get("xy"), dict):
        xy = body["color"]["xy"]
        r, g, b = xy_to_rgb(xy.get("x"), xy.get("y"))
        params["r"], params["g"], params["b"] = r, g, b
    elif "color_temperature" in body and isinstance(body["color_temperature"], dict):
        params["temp"] = int(clamp(mirek_to_kelvin(body["color_temperature"].get("mirek")), 2200, 6500))
    return json.dumps({"method": "setPilot", "params": params}, separators=(",", ":")).encode("utf-8")


# 4. LIFX Binary LAN Protocol (port 56700)
LIFX_GET_COLOR = 101
LIFX_SET_COLOR = 102
LIFX_LIGHT_STATE = 107
LIFX_SET_POWER = 117


def encode_lifx_packet(msg_type, payload=b"", source=0x4D415052, res_required=True):
    size = 36 + len(payload)
    header = struct.pack(
        "<HHI8s6sBBQH H",
        size,
        0x3400,
        source,
        b"\x00" * 8,
        b"\x00" * 6,
        1 if res_required else 0,
        1,
        0,
        msg_type,
        0,
    )
    return header + payload


def encode_lifx_get_color():
    return encode_lifx_packet(LIFX_GET_COLOR, b"", res_required=True)


def encode_lifx_set_power(is_on, duration_ms=250):
    payload = struct.pack("<HI", 65535 if is_on else 0, int(duration_ms))
    return encode_lifx_packet(LIFX_SET_POWER, payload, res_required=False)


def encode_lifx_set_color(hue16, sat16, bri16, kelvin, duration_ms=250):
    payload = struct.pack("<BHHHHI", 0, int(hue16), int(sat16), int(bri16), int(kelvin), int(duration_ms))
    return encode_lifx_packet(LIFX_SET_COLOR, payload, res_required=False)


def decode_lifx_light_state(pkt):
    if not isinstance(pkt, (bytes, bytearray)) or len(pkt) < 36 + 52:
        return None
    msg_type = struct.unpack_from("<H", pkt, 32)[0]
    if msg_type != LIFX_LIGHT_STATE:
        return None
    h_raw, s_raw, b_raw, k_raw, _dim, power_raw = struct.unpack_from("<HHHHhH", pkt, 36)
    label_raw = bytes(pkt[48:80]).split(b"\x00", 1)[0]
    label = clean(label_raw.decode("utf-8", errors="ignore"), 40)
    return {
        "hue": (h_raw / 65535.0) * 360.0,
        "sat": (s_raw / 65535.0) * 100.0,
        "bri": (b_raw / 65535.0) * 100.0,
        "kelvin": int(clamp(k_raw, 1500, 9000)),
        "power_on": power_raw > 0,
        "label": label,
    }


def lifx_to_rooms(conn, st):
    cid = conn["id"]
    name = st["label"] or conn["name"]
    mirek = kelvin_to_mirek(st["kelvin"])
    color_hex = hsb_to_hex(st["hue"], st["sat"], 100.0) if st["sat"] >= 4.0 else mirek_to_hex(mirek)
    light = _make_light(
        lid=f"lifx:{cid}:light",
        name=name,
        is_on=st["power_on"],
        bri=st["bri"],
        color=color_hex,
        provider="lifx",
        device_id=conn.get("device_id") or "",
        mirek=mirek,
        mirek_min=111,
        mirek_max=666,
    )
    room_name = conn.get("room_name") or name
    return [_make_room(f"lifx:{cid}:room", room_name, [light], "lifx")], []


# 5. Govee LAN (port 4003 control, port 4002 replies)
def parse_govee_status(conn, data):
    cid = conn["id"]
    msg = data.get("msg") if isinstance(data.get("msg"), dict) else data
    inner = msg.get("data") if isinstance(msg.get("data"), dict) else msg
    is_on = bool(int(finite(inner.get("onOff"), 0, 0, 1)))
    bri = finite(inner.get("brightness"), 100.0 if is_on else 0.0, 0.0, 100.0)
    kelvin = int(finite(inner.get("colorTemInKelvin"), 0, 0, 9000))
    mirek = kelvin_to_mirek(kelvin) if kelvin >= 1500 else None
    col = inner.get("color") if isinstance(inner.get("color"), dict) else {}
    r, g, b = col.get("r"), col.get("g"), col.get("b")
    if all(isinstance(v, (int, float)) for v in (r, g, b)) and (r or g or b):
        color_hex = rgb_to_hex(r, g, b)
    elif mirek is not None:
        color_hex = mirek_to_hex(mirek)
    else:
        color_hex = DEFAULT_WARM_HEX
    light = _make_light(
        lid=f"govee:{cid}:light",
        name=conn["name"],
        is_on=is_on,
        bri=bri,
        color=color_hex,
        provider="govee",
        device_id=conn.get("device_id") or "",
        mirek=mirek,
        mirek_min=111,
        mirek_max=500,
    )
    room_name = conn.get("room_name") or conn["name"]
    return [_make_room(f"govee:{cid}:room", room_name, [light], "govee")], []


def build_govee_commands(body):
    cmds = []
    if "on" in body and isinstance(body["on"], dict):
        cmds.append({"msg": {"cmd": "turn", "data": {"value": 1 if body["on"].get("on") else 0}}})
    if "dimming" in body and isinstance(body["dimming"], dict):
        bri = int(clamp(round(finite(body["dimming"].get("brightness"), 100, 1, 100)), 1, 100))
        cmds.append({"msg": {"cmd": "brightness", "data": {"value": bri}}})
    if "color" in body and isinstance(body["color"], dict) and isinstance(body["color"].get("xy"), dict):
        r, g, b = xy_to_rgb(body["color"]["xy"].get("x"), body["color"]["xy"].get("y"))
        cmds.append({"msg": {"cmd": "colorwc", "data": {"color": {"r": r, "g": g, "b": b}, "colorTemInKelvin": 0}}})
    elif "color_temperature" in body and isinstance(body["color_temperature"], dict):
        k = int(clamp(mirek_to_kelvin(body["color_temperature"].get("mirek")), 2000, 9000))
        cmds.append({"msg": {"cmd": "colorwc", "data": {"color": {"r": 0, "g": 0, "b": 0}, "colorTemInKelvin": k}}})
    return [json.dumps(c, separators=(",", ":")).encode("utf-8") for c in cmds]


# 6. Nanoleaf (HTTP 16021)
def parse_nanoleaf(conn, data):
    cid = conn["id"]
    state = data.get("state") if isinstance(data.get("state"), dict) else {}
    effects_obj = data.get("effects") if isinstance(data.get("effects"), dict) else {}
    name = clean(data.get("name") or conn["name"], 40) or conn["name"]
    is_on = bool((state.get("on") or {}).get("value", False))
    bri = finite((state.get("brightness") or {}).get("value"), 100.0 if is_on else 0.0, 0.0, 100.0)
    hue = finite((state.get("hue") or {}).get("value"), 0.0, 0.0, 360.0)
    sat = finite((state.get("sat") or {}).get("value"), 0.0, 0.0, 100.0)
    ct_k = int(finite((state.get("ct") or {}).get("value"), 3200, 1200, 6500))
    mirek = kelvin_to_mirek(ct_k)
    cmode = str(state.get("colorMode") or "")
    color_hex = mirek_to_hex(mirek) if (cmode == "ct" or sat < 3.0) else hsb_to_hex(hue, sat, 100.0)
    selected_fx = clean(effects_obj.get("select") or "", 40)
    raw_list = [x for x in (effects_obj.get("effectsList") or []) if isinstance(x, str)]
    scene_map = {}
    scenes = []
    for fx in raw_list[:24]:
        cfx = clean(fx, 30)
        if not cfx:
            continue
        sid = f"nanoleaf:{cid}:fx:{_slug(cfx)}"
        scene_map[sid] = fx
        scenes.append({"id": sid, "name": cfx, "active": fx == selected_fx, "provider": "nanoleaf"})
    light = _make_light(
        lid=f"nanoleaf:{cid}:panel",
        name=name,
        is_on=is_on,
        bri=bri,
        color=color_hex,
        provider="nanoleaf",
        device_id=clean(data.get("serialNo") or "", 40),
        mirek=mirek,
        mirek_min=154,
        mirek_max=833,
        effect=("no_effect" if (not selected_fx or selected_fx.startswith("*")) else selected_fx),
        available_effects=[clean(x, 32) for x in raw_list[:16] if clean(x, 32)],
    )
    room_name = conn.get("room_name") or name
    return [_make_room(f"nanoleaf:{cid}:room", room_name, [light], "nanoleaf", scenes=scenes)], [], scene_map


# 7. Shelly (Gen2/Gen3 RPC + Gen1 REST)
def parse_shelly_gen2(conn, info, status):
    cid = conn["id"]
    label = clean(info.get("name") or conn["name"], 36) or conn["name"]
    lights = []
    meta_map = {}
    for key in sorted(status.keys()):
        obj = status[key]
        if not isinstance(obj, dict) or ":" not in key:
            continue
        comp, ch_s = key.split(":", 1)
        comp = comp.lower()
        if comp not in ("light", "rgb", "rgbw", "switch"):
            continue
        try:
            ch = int(ch_s)
        except ValueError:
            continue
        is_on = bool(obj.get("output", False))
        bri = finite(obj.get("brightness"), 100.0 if is_on else 0.0, 0.0, 100.0)
        rgb = obj.get("rgb")
        color_hex = DEFAULT_WARM_HEX
        if isinstance(rgb, list) and len(rgb) >= 3:
            color_hex = rgb_to_hex(finite(rgb[0], 255, 0, 255), finite(rgb[1], 210, 0, 255), finite(rgb[2], 122, 0, 255))
        lid = f"shelly:{cid}:{comp}:{ch}"
        meta_map[lid] = {"gen": 2, "kind": comp, "channel": ch}
        lights.append(_make_light(
            lid=lid,
            name=(label if ch == 0 else f"{label} {ch + 1}"),
            is_on=is_on,
            bri=bri,
            color=color_hex,
            provider="shelly",
            device_id=clean(info.get("mac") or "", 32),
            dimmable=(comp != "switch"),
            supports_color=(comp in ("rgb", "rgbw")),
            supports_ct=False,
        ))
    room_name = conn.get("room_name") or label
    rooms = [_make_room(f"shelly:{cid}:room", room_name, lights, "shelly")] if lights else []
    return rooms, [], meta_map


def parse_shelly_gen1(conn, status):
    cid = conn["id"]
    lights = []
    meta_map = {}
    if isinstance(status.get("lights"), list) and status["lights"]:
        for idx, item in enumerate(status["lights"]):
            if not isinstance(item, dict):
                continue
            is_on = bool(item.get("ison", False))
            bri = finite(item.get("brightness", item.get("gain")), 100.0 if is_on else 0.0, 0.0, 100.0)
            mode = str(item.get("mode") or "").lower()
            is_color = (mode == "color") or (mode != "white" and all(k in item for k in ("red", "green", "blue")))
            temp_k = item.get("temp")
            mirek = kelvin_to_mirek(temp_k) if temp_k is not None else None
            if is_color:
                color_hex = rgb_to_hex(finite(item.get("red"), 255, 0, 255), finite(item.get("green"), 210, 0, 255), finite(item.get("blue"), 122, 0, 255))
            elif mirek is not None:
                color_hex = mirek_to_hex(mirek)
            else:
                color_hex = DEFAULT_WARM_HEX
            comp = "color" if is_color else ("white" if mode == "white" else "light")
            lid = f"shelly:{cid}:{comp}:{idx}"
            meta_map[lid] = {"gen": 1, "kind": comp, "channel": idx}
            lights.append(_make_light(
                lid=lid,
                name=(conn["name"] if idx == 0 else f"{conn['name']} {idx + 1}"),
                is_on=is_on,
                bri=bri,
                color=color_hex,
                provider="shelly",
                supports_color=is_color,
                supports_ct=(temp_k is not None),
                mirek=mirek,
            ))
    elif isinstance(status.get("relays"), list):
        for idx, item in enumerate(status["relays"]):
            if not isinstance(item, dict):
                continue
            is_on = bool(item.get("ison", False))
            lid = f"shelly:{cid}:relay:{idx}"
            meta_map[lid] = {"gen": 1, "kind": "relay", "channel": idx}
            lights.append(_make_light(
                lid=lid,
                name=(conn["name"] if idx == 0 else f"{conn['name']} {idx + 1}"),
                is_on=is_on,
                bri=(100.0 if is_on else 0.0),
                color=DEFAULT_WARM_HEX,
                provider="shelly",
                dimmable=False,
                supports_color=False,
                supports_ct=False,
            ))
    room_name = conn.get("room_name") or conn["name"]
    rooms = [_make_room(f"shelly:{cid}:room", room_name, lights, "shelly")] if lights else []
    return rooms, [], meta_map


# 8. IKEA DIRIGERA (HTTPS 8443)
def parse_dirigera(conn, devices, raw_scenes=None):
    cid = conn["id"]
    lights_by_room = {}
    room_names = {}
    sensors = []
    kelvin_bounds = {}
    for dev in (devices if isinstance(devices, list) else []):
        if not isinstance(dev, dict):
            continue
        did = str(dev.get("id") or "")
        if not did:
            continue
        dtype = str(dev.get("type") or "").lower()
        dev_type = str(dev.get("deviceType") or "").lower()
        attrs = dev.get("attributes") if isinstance(dev.get("attributes"), dict) else {}
        caps = dev.get("capabilities") if isinstance(dev.get("capabilities"), dict) else {}
        can_recv = set(caps.get("canReceive") or [])
        reachable = bool(dev.get("isReachable", True))
        dname = clean(attrs.get("customName") or attrs.get("model") or "IKEA Light", 50) or "IKEA Light"
        robj = dev.get("room") if isinstance(dev.get("room"), dict) else {}
        rkey = str(robj.get("id") or "unassigned")
        rname = clean(robj.get("name") or conn.get("room_name") or conn["name"], 40) or conn["name"]
        room_names[rkey] = rname

        if dtype == "light" or dev_type == "light":
            is_on = bool(attrs.get("isOn", False))
            bri = finite(attrs.get("lightLevel"), 100.0 if is_on else 0.0, 0.0, 100.0)
            supports_color = ("colorHue" in can_recv) or ("colorHue" in attrs)
            supports_ct = ("colorTemperature" in can_recv) or ("colorTemperature" in attrs)
            dimmable = ("lightLevel" in can_recv) or ("lightLevel" in attrs)
            ct_k = attrs.get("colorTemperature")
            mirek = kelvin_to_mirek(ct_k) if ct_k is not None else None
            min_k = int(finite(attrs.get("colorTemperatureMin"), 2200, 1500, 6500))
            max_k = int(finite(attrs.get("colorTemperatureMax"), 4000, 1500, 6500))
            lo_k, hi_k = min(min_k, max_k), max(min_k, max_k)
            lid = f"dirigera:{cid}:{did}"
            kelvin_bounds[lid] = (lo_k, hi_k)
            h_val = finite(attrs.get("colorHue"), None, 0.0, 360.0)
            s_val = finite(attrs.get("colorSaturation"), None, 0.0, 1.0)
            if h_val is not None and s_val is not None and s_val >= 0.04:
                color_hex = hsb_to_hex(h_val, s_val * 100.0, 100.0)
            elif mirek is not None:
                color_hex = mirek_to_hex(mirek)
            else:
                color_hex = DEFAULT_WARM_HEX
            light = _make_light(
                lid=lid,
                name=dname,
                is_on=is_on,
                bri=bri,
                color=color_hex,
                provider="dirigera",
                device_id=did,
                dimmable=dimmable,
                reachable=reachable,
                supports_color=supports_color,
                supports_ct=supports_ct,
                mirek=mirek,
                mirek_min=kelvin_to_mirek(hi_k),
                mirek_max=kelvin_to_mirek(lo_k),
            )
            lights_by_room.setdefault(rkey, []).append(light)
        elif dtype == "sensor":
            skind = None
            if "motion" in dev_type or "isDetected" in attrs:
                skind = "motion"
            elif "openclose" in dev_type or "isOpen" in attrs:
                skind = "contact"
            if skind:
                sensors.append({
                    "id": f"dirigera:{cid}:sensor:{did}",
                    "device_id": did,
                    "kind": skind,
                    "name": dname,
                    "room_id": f"dirigera:{cid}:room:{rkey}",
                    "enabled": reachable,
                    "motion": bool(attrs.get("isDetected", False)),
                    "open": bool(attrs.get("isOpen", False)),
                    "temperature": finite(attrs.get("currentTemperature"), None, -50.0, 120.0),
                    "lux": finite(attrs.get("illuminance"), None, 0.0, 200000.0),
                    "battery": int(finite(attrs.get("batteryPercentage"), None, 0, 100)) if attrs.get("batteryPercentage") is not None else None,
                    "battery_state": "",
                    "sensitivity": None,
                    "sensitivity_max": None,
                    "button_ids": [],
                    "last_event": "",
                    "provider": "dirigera",
                })

    scenes = []
    for sc in (raw_scenes if isinstance(raw_scenes, list) else []):
        if not isinstance(sc, dict):
            continue
        sid = str(sc.get("id") or "")
        info = sc.get("info") if isinstance(sc.get("info"), dict) else {}
        sname = clean(info.get("name") or "", 30)
        if sid and sname:
            scenes.append({"id": f"dirigera:{cid}:scene:{sid}", "name": sname, "active": False, "provider": "dirigera"})
    scenes.sort(key=lambda s: s["name"].lower())

    rooms = []
    for idx, rkey in enumerate(sorted(lights_by_room.keys(), key=lambda k: room_names.get(k, "").lower())):
        members = sorted(lights_by_room[rkey], key=lambda l: l["name"].lower())
        rooms.append(_make_room(
            rid=f"dirigera:{cid}:room:{rkey}",
            name=room_names.get(rkey, conn["name"]),
            lights=members,
            provider="dirigera",
            scenes=scenes[:24] if idx == 0 else [],
        ))
    return rooms, sensors, kelvin_bounds


# --------------------------------------------------------------------------- Button Pairing & Discovery

def pair_nanoleaf(host, port=16021, attempts=12, delay=2.0):
    resolve_local_ipv4(host, port, allow_loopback=False)
    for _ in range(attempts):
        status, raw = _http_request(host, int(port), "POST", "/api/v1/new", timeout=3.0)
        if 200 <= status < 300:
            try:
                data = json.loads(raw.decode("utf-8"))
                tok = clean_token(data.get("auth_token") or "")
                if tok:
                    return tok, ""
            except (ValueError, UnicodeDecodeError):
                pass
        time.sleep(delay)
    raise ProviderError("Nanoleaf pairing timed out. Hold the power button for 5-7 seconds and retry.")


def pair_dirigera(host, port=8443, attempts=15, delay=2.0):
    resolve_local_ipv4(host, port, allow_loopback=False)
    verifier = secrets.token_urlsafe(48)[:64]
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode("ascii")).digest()).rstrip(b"=").decode("ascii")
    qs = urllib.parse.urlencode({
        "audience": "homesmart.local",
        "response_type": "code",
        "code_challenge": challenge,
        "code_challenge_method": "S256",
    })
    pinned = []
    status, raw = _http_request(
        host, int(port), "GET", f"/v1/oauth/authorize?{qs}",
        use_https=True, timeout=5.0, capture_fp=pinned,
    )
    if not (200 <= status < 300):
        raise ProviderError(f"DIRIGERA authorize failed (HTTP {status}).")
    fp = pinned[0] if pinned else ""
    try:
        code = str(json.loads(raw.decode("utf-8")).get("code") or "")
    except (ValueError, UnicodeDecodeError):
        code = ""
    if not code:
        raise ProviderError("DIRIGERA did not return an authorization code.")
    form = urllib.parse.urlencode({
        "code": code,
        "name": "MaprismaLights",
        "grant_type": "authorization_code",
        "code_verifier": verifier,
    }).encode("ascii")
    for _ in range(attempts):
        t_status, t_raw = _http_request(
            host, int(port), "POST", "/v1/oauth/token",
            body=form,
            headers={"Content-Type": "application/x-www-form-urlencoded"},
            use_https=True,
            timeout=4.0,
            expected_fp=fp,
        )
        if 200 <= t_status < 300:
            try:
                tok = clean_token(json.loads(t_raw.decode("utf-8")).get("access_token") or "")
                if tok:
                    return tok, fp
            except (ValueError, UnicodeDecodeError):
                pass
        time.sleep(delay)
    raise ProviderError("DIRIGERA pairing timed out. Press the Action Button underneath the hub and retry.")


def discover_providers(run_bounded=None):
    """Best-effort LAN discovery across Avahi Bonjour services + WiZ/LIFX UDP broadcast."""
    found = {}

    def add(kind, name, host, port):
        meta = PROVIDER_KINDS.get(kind)
        if not meta or not valid_local_host(host, allow_loopback=meta["allows_loopback"]):
            return
        key = f"{kind}:{host}:{port}"
        found[key] = {
            "id": key,
            "kind": kind,
            "name": clean(name or meta["name"], 48) or meta["name"],
            "host": host,
            "port": int(port),
            "badge": meta["badge"],
        }

    if run_bounded is not None:
        for kind, meta in PROVIDER_KINDS.items():
            svc = meta.get("bonjour")
            if not svc:
                continue
            try:
                out = run_bounded(["avahi-browse", "-rtp", svc], timeout=2.5)
            except Exception:
                out = ""
            for line in (out or "").splitlines():
                parts = line.split(";")
                if len(parts) >= 9 and parts[0] == "=" and parts[2] == "IPv4":
                    svc_name = parts[3].replace("\\032", " ")
                    ip = parts[7]
                    try:
                        port = int(parts[8])
                    except ValueError:
                        port = meta["default_port"]
                    add(kind, svc_name, ip, port)

    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        sock.settimeout(0.6)
        sock.sendto(b'{"method":"getSystemConfig","params":{}}', ("255.255.255.255", 38899))
        deadline = time.monotonic() + 0.8
        while time.monotonic() < deadline:
            try:
                data, addr = sock.recvfrom(4096)
                obj = json.loads(data.decode("utf-8", errors="ignore"))
                res = obj.get("result") if isinstance(obj, dict) else None
                if isinstance(res, dict) and ("mac" in res or "moduleName" in res):
                    mod = clean(res.get("moduleName") or "WiZ Light", 40)
                    add("wiz", mod, addr[0], 38899)
            except (socket.timeout, ValueError, OSError):
                break
        sock.close()
    except OSError:
        pass

    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        sock.settimeout(0.6)
        sock.sendto(encode_lifx_get_color(), ("255.255.255.255", 56700))
        deadline = time.monotonic() + 0.8
        while time.monotonic() < deadline:
            try:
                data, addr = sock.recvfrom(4096)
                st = decode_lifx_light_state(data)
                if st:
                    add("lifx", st["label"] or "LIFX Light", addr[0], 56700)
            except (socket.timeout, OSError):
                break
        sock.close()
    except OSError:
        pass

    return sorted(found.values(), key=lambda d: (d["kind"], d["name"].lower()))


# --------------------------------------------------------------------------- ProviderManager

class ProviderManager:
    """Manages persisted extra providers, bounded ordered commands, and non-blocking polling."""

    def __init__(self, config_dir):
        self.config_dir = config_dir
        self.lock = threading.RLock()
        self.connections = []
        self.conn_generations = {}     # conn_id -> int (bumped on edit/toggle/remove)
        self.results = {}              # conn_id -> {"rooms", "sensors", "light_count", "on_count"}
        self.connected = {}            # conn_id -> bool
        self.errors = {}               # conn_id -> str
        self.wled_effects = {}         # conn_id -> [str]
        self.nanoleaf_scenes = {}      # conn_id -> {sid: fx_name}
        self.shelly_meta = {}          # lid -> {"gen", "kind", "channel"}
        self.lifx_state = {}           # conn_id -> decoded dict
        self.dirigera_kelvin = {}      # lid -> (lo_k, hi_k)
        self.discovered = []
        self.discovering = False
        self.last_action = {"running": False, "kind": "", "ok": False, "message": "", "pair_id": ""}
        # Server-side one-time pairing tokens (never emitted in snapshots!)
        self._paired_tokens = {}       # pair_id -> {"kind", "host", "port", "token", "fp", "expires"}
        self._pending_cmds = {}        # (kind, rid) -> body (coalesced & bounded)
        self._refresh_hold_until = 0.0
        self._last_refresh = 0.0
        self._worker_wake = threading.Event()
        self._worker_stop = threading.Event()
        self._worker_thread = None
        self._on_change = None
        self.load()

    def _path(self):
        return os.path.join(self.config_dir, "providers.json")

    def _ensure_private_dir(self):
        os.makedirs(self.config_dir, mode=0o700, exist_ok=True)
        st = os.lstat(self.config_dir)
        if not stat.S_ISDIR(st.st_mode) or stat.S_ISLNK(st.st_mode) or st.st_uid != os.getuid():
            raise PermissionError("Config directory is not private.")
        if st.st_mode & 0o077:
            os.chmod(self.config_dir, 0o700)

    def load(self):
        path = self._path()
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        try:
            fd = os.open(path, flags)
        except OSError:
            return
        try:
            st = os.fstat(fd)
            if (not stat.S_ISREG(st.st_mode)
                    or st.st_uid != os.getuid()
                    or (st.st_mode & 0o077) != 0
                    or st.st_size > 262144):
                return
            with os.fdopen(fd, "r", encoding="utf-8") as fh:
                fd = -1
                raw = json.load(fh)
        except (OSError, ValueError):
            return
        finally:
            if fd >= 0:
                try:
                    os.close(fd)
                except OSError:
                    pass
        items = raw.get("providers") if isinstance(raw, dict) else (raw if isinstance(raw, list) else [])
        loaded = []
        seen = set()
        for entry in items:
            if len(loaded) >= MAX_PROVIDERS:
                break
            clean_c = sanitize_connection(entry)
            if clean_c and clean_c["id"] not in seen:
                seen.add(clean_c["id"])
                loaded.append(clean_c)
        with self.lock:
            self.connections = loaded
            for c in loaded:
                self.conn_generations.setdefault(c["id"], 1)

    def save(self):
        self._ensure_private_dir()
        with self.lock:
            payload = {"version": 1, "providers": list(self.connections)}
        data = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode("utf-8")
        path = self._path()
        tmp = os.path.join(self.config_dir, f".providers.{os.getpid()}.{secrets.token_hex(4)}.tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            os.write(fd, data)
            os.fsync(fd)
        finally:
            os.close(fd)
        os.replace(tmp, path)

    def secrets(self):
        with self.lock:
            toks = {c["token"] for c in self.connections if c.get("token")}
            toks.update(v["token"] for v in self._paired_tokens.values() if v.get("token"))
            return toks

    def redact(self, text, extra_secrets=None):
        out = str(text or "")
        all_sec = self.secrets()
        if extra_secrets:
            all_sec = all_sec | set(extra_secrets)
        for sec in all_sec:
            if isinstance(sec, str) and len(sec) >= 6:
                out = out.replace(sec, "***")
        return clean(out, 140)

    def has_enabled(self):
        with self.lock:
            return any(c.get("enabled") for c in self.connections)

    def store_paired_token(self, kind, host, port, token, cert_fp=""):
        """Hold a newly paired token server-side under a one-time pair_id (never emitted in snapshots)."""
        pair_id = secrets.token_hex(8)
        now = time.monotonic()
        with self.lock:
            # Prune expired entries
            self._paired_tokens = {k: v for k, v in self._paired_tokens.items() if v["expires"] > now}
            self._paired_tokens[pair_id] = {
                "kind": kind,
                "host": host.strip().lower(),
                "port": int(port),
                "token": clean_token(token),
                "fp": cert_fp,
                "expires": now + 600.0,
            }
        return pair_id

    def _resolve_saved_or_paired_secret(self, raw, consume_pair=False):
        merged = dict(raw) if isinstance(raw, dict) else {}
        cand = sanitize_connection(merged)
        if not cand:
            return None
        pair_id = str(merged.get("pair_id") or "").strip()
        if not cand["token"] and pair_id:
            with self.lock:
                rec = self._paired_tokens.get(pair_id)
                if (rec
                        and rec["expires"] > time.monotonic()
                        and rec["kind"] == cand["kind"]
                        and rec["host"] == cand["host"].lower()
                        and rec["port"] == cand["port"]):
                    cand["token"] = rec["token"]
                    if rec["fp"] and not cand["cert_fingerprint"]:
                        cand["cert_fingerprint"] = rec["fp"]
                    if consume_pair:
                        self._paired_tokens.pop(pair_id, None)
        if not cand["token"]:
            # Reuse saved token ONLY when kind, host, AND port still match the saved connection!
            with self.lock:
                for c in self.connections:
                    if (c["id"] == cand["id"]
                            and c["kind"] == cand["kind"]
                            and c["host"].lower() == cand["host"].lower()
                            and c["port"] == cand["port"]):
                        cand["token"] = c.get("token", "")
                        if c.get("cert_fingerprint") and not cand["cert_fingerprint"]:
                            cand["cert_fingerprint"] = c["cert_fingerprint"]
                        break
        return cand

    def save_connection(self, raw):
        clean_c = self._resolve_saved_or_paired_secret(raw, consume_pair=True)
        if not clean_c:
            raise ProviderError("Invalid device configuration (check local IP / host).")
        meta = PROVIDER_KINDS[clean_c["kind"]]
        if meta["requires_token"] and not clean_c["token"]:
            raise ProviderError("Access token is required (enter a token or re-pair for this host).")
        with self.lock:
            replaced = False
            for idx, c in enumerate(self.connections):
                if c["id"] == clean_c["id"]:
                    self.connections[idx] = clean_c
                    replaced = True
                    break
            if not replaced:
                if len(self.connections) >= MAX_PROVIDERS:
                    raise ProviderError("Maximum number of devices reached.")
                self.connections.append(clean_c)
            self.conn_generations[clean_c["id"]] = self.conn_generations.get(clean_c["id"], 0) + 1
            self.errors.pop(clean_c["id"], None)
        self.save()
        self.trigger_refresh()
        return clean_c

    def remove_connection(self, cid):
        with self.lock:
            self.connections = [c for c in self.connections if c["id"] != cid]
            self.conn_generations[cid] = self.conn_generations.get(cid, 0) + 1
            self.results.pop(cid, None)
            self.connected.pop(cid, None)
            self.errors.pop(cid, None)
            self.wled_effects.pop(cid, None)
            self.nanoleaf_scenes.pop(cid, None)
            self.lifx_state.pop(cid, None)
            self.shelly_meta = {k: v for k, v in self.shelly_meta.items() if not k.startswith(f"shelly:{cid}:")}
            self.dirigera_kelvin = {k: v for k, v in self.dirigera_kelvin.items() if not k.startswith(f"dirigera:{cid}:")}
        self.save()

    def toggle_connection(self, cid, enabled):
        with self.lock:
            for c in self.connections:
                if c["id"] == cid:
                    c["enabled"] = bool(enabled)
                    self.conn_generations[cid] = self.conn_generations.get(cid, 0) + 1
                    if not enabled:
                        self.results.pop(cid, None)
                        self.connected[cid] = False
                        self.errors[cid] = ""
                    break
        self.save()
        if enabled:
            self.trigger_refresh()

    # -- Dedicated non-blocking worker for polling & coalesced sends
    def start_worker(self, on_change, is_panel_open_fn):
        with self.lock:
            if self._worker_thread is not None and self._worker_thread.is_alive():
                return
            self._on_change = on_change
            self._worker_stop.clear()

            def _loop():
                while not self._worker_stop.is_set():
                    sent_any = self._drain_pending()
                    if sent_any:
                        continue
                    if self.has_enabled():
                        panel_open = False
                        try:
                            panel_open = bool(is_panel_open_fn())
                        except Exception:
                            pass
                        interval = 4.0 if panel_open else 20.0
                        now = time.time()
                        if now >= self._refresh_hold_until and now - self._last_refresh >= interval:
                            self.refresh_all()
                            self._last_refresh = time.time()
                            if self._on_change:
                                self._on_change()
                        wait = max(0.2, min(interval, max(self._refresh_hold_until - time.time(), interval - (time.time() - self._last_refresh))))
                    else:
                        wait = 5.0
                    self._worker_wake.wait(wait)
                    self._worker_wake.clear()

            self._worker_thread = threading.Thread(target=_loop, daemon=True)
            self._worker_thread.start()

    def stop_worker(self):
        self._worker_stop.set()
        self._worker_wake.set()

    def trigger_refresh(self):
        self._last_refresh = 0.0
        self._refresh_hold_until = 0.0
        self._worker_wake.set()

    def queue_command(self, kind, rid, body):
        with self.lock:
            key = (kind, rid)
            if key not in self._pending_cmds and len(self._pending_cmds) >= MAX_PENDING_EXTRA:
                return False
            merged = dict(self._pending_cmds.get(key, {}))
            merged.update(body)
            self._pending_cmds[key] = merged
            self._refresh_hold_until = time.time() + 1.5
        self._worker_wake.set()
        return True

    def _drain_pending(self):
        with self.lock:
            if not self._pending_cmds:
                return False
            key, body = next(iter(self._pending_cmds.items()))
            del self._pending_cmds[key]
        kind, rid = key
        try:
            if kind == "scene":
                self.recall_scene(rid)
                self._refresh_hold_until = time.time() + 0.5
                self._last_refresh = 0.0
            else:
                self.set_light(rid, body)
        except Exception as exc:  # noqa: BLE001
            parts = rid.split(":", 2)
            if len(parts) >= 2:
                cid = parts[1]
                with self.lock:
                    self.errors[cid] = self.redact(str(exc)) or "Command failed"
                if self._on_change:
                    self._on_change()
        return True

    def _persist_captured_fp(self, cid, gen, captured_fp):
        if not captured_fp or not SHA256_HEX_RE.match(captured_fp):
            return
        changed = False
        with self.lock:
            if self.conn_generations.get(cid) != gen:
                return
            for c in self.connections:
                if c["id"] == cid and not c.get("cert_fingerprint"):
                    c["cert_fingerprint"] = captured_fp
                    changed = True
                    break
        if changed:
            try:
                self.save()
            except OSError:
                pass

    def _fetch_ha_areas(self, conn, hdrs, fp_capture):
        tpl = (
            "{% set ns = namespace(items=[]) %}"
            "{% for s in states if s.domain in ['light', 'binary_sensor'] %}"
            "{% set a = area_name(s.entity_id) %}"
            "{% if a %}{% set ns.items = ns.items + [[s.entity_id, a]] %}{% endif %}"
            "{% endfor %}{{ ns.items | to_json }}"
        )
        try:
            st, raw = _http_request(
                conn["host"], conn["port"], "POST", "/api/template",
                body={"template": tpl},
                headers=hdrs,
                use_https=conn["use_https"],
                timeout=3.0,
                allow_loopback=True,
                expected_fp=conn.get("cert_fingerprint", ""),
                capture_fp=fp_capture,
            )
            if 200 <= st < 300:
                pairs = json.loads(raw.decode("utf-8"))
                if isinstance(pairs, list):
                    return {str(k): str(v) for k, v in pairs if isinstance(k, str) and isinstance(v, str)}
        except Exception:
            pass
        return {}

    def fetch_single(self, conn, gen=None):
        kind = conn["kind"]
        host, port = conn["host"], conn["port"]
        token = conn.get("token", "")
        expected_fp = conn.get("cert_fingerprint", "")
        fp_capture = []

        if kind == "home_assistant":
            if not token:
                raise ProviderError("Access token is required.")
            hdrs = {"Authorization": f"Bearer {token}", "Accept": "application/json"}
            st, raw = _http_request(
                host, port, "GET", "/api/states",
                headers=hdrs,
                use_https=conn["use_https"],
                timeout=6.0,
                allow_loopback=True,
                max_body=MAX_HA_BODY,
                expected_fp=expected_fp,
                capture_fp=fp_capture,
            )
            if not (200 <= st < 300):
                raise ProviderError(f"Home Assistant HTTP {st}")
            if fp_capture and gen is not None:
                self._persist_captured_fp(conn["id"], gen, fp_capture[0])
            states = json.loads(raw.decode("utf-8"))
            areas = self._fetch_ha_areas(conn, hdrs, fp_capture)
            rooms, sensors = parse_home_assistant_states(conn, states, entity_areas=areas)
            return rooms, sensors

        if kind == "wled":
            st, raw = _http_request(host, port, "GET", "/json", timeout=4.5)
            if not (200 <= st < 300):
                raise ProviderError(f"WLED HTTP {st}")
            data = json.loads(raw.decode("utf-8"))
            presets = {}
            try:
                ps_st, ps_raw = _http_request(host, port, "GET", "/presets.json", timeout=2.5)
                if 200 <= ps_st < 300:
                    presets = json.loads(ps_raw.decode("utf-8"))
            except Exception:
                pass
            rooms, sensors, effects = parse_wled(conn, data, presets)
            with self.lock:
                self.wled_effects[conn["id"]] = effects
            return rooms, sensors

        if kind == "wiz":
            reply = _udp_exchange(host, port, b'{"method":"getPilot","params":{}}', expect_reply=True, timeout=2.0)
            if not reply:
                raise ProviderError("No UDP response from WiZ device.")
            data = json.loads(reply.decode("utf-8"))
            return parse_wiz_pilot(conn, data)

        if kind == "lifx":
            reply = _udp_exchange(host, port, encode_lifx_get_color(), expect_reply=True, timeout=2.0)
            decoded = decode_lifx_light_state(reply) if reply else None
            if not decoded:
                raise ProviderError("No LAN response from LIFX device.")
            with self.lock:
                self.lifx_state[conn["id"]] = decoded
            return lifx_to_rooms(conn, decoded)

        if kind == "govee":
            # Govee LAN sends devStatus responses to client UDP port 4002.
            reply = None
            try:
                reply = _udp_exchange(
                    host, port, b'{"msg":{"cmd":"devStatus","data":{}}}',
                    expect_reply=True, timeout=2.0, listen_port=4002,
                )
            except OSError:
                reply = _udp_exchange(
                    host, port, b'{"msg":{"cmd":"devStatus","data":{}}}',
                    expect_reply=True, timeout=1.5, listen_port=0,
                )
            if not reply:
                raise ProviderError("No Govee LAN reply on UDP port 4002 (enable LAN Control in Govee app).")
            data = json.loads(reply.decode("utf-8"))
            return parse_govee_status(conn, data)

        if kind == "nanoleaf":
            if not token:
                raise ProviderError("Nanoleaf auth token is required.")
            st, raw = _http_request(host, port, "GET", f"/api/v1/{urllib.parse.quote(token, safe='')}/", timeout=4.5)
            if not (200 <= st < 300):
                raise ProviderError(f"Nanoleaf HTTP {st}")
            data = json.loads(raw.decode("utf-8"))
            rooms, sensors, scene_map = parse_nanoleaf(conn, data)
            with self.lock:
                self.nanoleaf_scenes[conn["id"]] = scene_map
            return rooms, sensors

        if kind == "shelly":
            try:
                s1, r1 = _http_request(host, port, "GET", "/rpc/Shelly.GetDeviceInfo", timeout=3.0)
                if s1 == 401:
                    raise ProviderError("Shelly Gen2 authentication required.")
                if 200 <= s1 < 300:
                    s2, r2 = _http_request(host, port, "GET", "/rpc/Shelly.GetStatus", timeout=3.0)
                    if s2 == 401:
                        raise ProviderError("Shelly Gen2 authentication required.")
                    if 200 <= s2 < 300:
                        rooms, sensors, meta_map = parse_shelly_gen2(conn, json.loads(r1.decode("utf-8")), json.loads(r2.decode("utf-8")))
                        with self.lock:
                            self.shelly_meta.update(meta_map)
                        return rooms, sensors
            except ProviderError:
                raise
            except Exception:
                pass
            st, raw = _http_request(host, port, "GET", "/status", timeout=4.0)
            if st == 401:
                raise ProviderError("Shelly authentication required.")
            if not (200 <= st < 300):
                raise ProviderError(f"Shelly HTTP {st}")
            rooms, sensors, meta_map = parse_shelly_gen1(conn, json.loads(raw.decode("utf-8")))
            with self.lock:
                self.shelly_meta.update(meta_map)
            return rooms, sensors

        if kind == "dirigera":
            if not token:
                raise ProviderError("DIRIGERA access token is required.")
            hdrs = {"Authorization": f"Bearer {token}", "Accept": "application/json"}
            st, raw = _http_request(
                host, port, "GET", "/v1/devices",
                headers=hdrs, use_https=True, timeout=5.5,
                expected_fp=expected_fp, capture_fp=fp_capture,
            )
            if not (200 <= st < 300):
                raise ProviderError(f"DIRIGERA HTTP {st}")
            if fp_capture and gen is not None:
                self._persist_captured_fp(conn["id"], gen, fp_capture[0])
            devices = json.loads(raw.decode("utf-8"))
            raw_scenes = []
            try:
                sc_st, sc_raw = _http_request(
                    host, port, "GET", "/v1/scenes",
                    headers=hdrs, use_https=True, timeout=3.5,
                    expected_fp=(expected_fp or (fp_capture[0] if fp_capture else "")),
                )
                if 200 <= sc_st < 300:
                    raw_scenes = json.loads(sc_raw.decode("utf-8"))
            except Exception:
                pass
            rooms, sensors, k_bounds = parse_dirigera(conn, devices, raw_scenes)
            with self.lock:
                self.dirigera_kelvin.update(k_bounds)
            return rooms, sensors

        raise ProviderError(f"Unknown provider: {kind}")

    def test_connection(self, raw):
        clean_c = self._resolve_saved_or_paired_secret(raw, consume_pair=False)
        if not clean_c:
            raise ProviderError("Invalid host or configuration.")
        try:
            rooms, _sensors = self.fetch_single(clean_c)
        except Exception as exc:  # noqa: BLE001
            raise ProviderError(self.redact(str(exc), extra_secrets={clean_c.get("token", "")})) from exc
        lcount = sum(len(r["lights"]) for r in rooms)
        return f"{clean_c['name']} · {lcount}"

    def refresh_all(self):
        with self.lock:
            active = [(dict(c), self.conn_generations.get(c["id"], 0)) for c in self.connections if c.get("enabled")]
        for conn, gen in active:
            cid = conn["id"]
            try:
                rooms, sensors = self.fetch_single(conn, gen=gen)
                lcount = sum(len(r["lights"]) for r in rooms)
                ocount = sum(1 for r in rooms for l in r["lights"] if l["on"])
                with self.lock:
                    # Drop stale refresh if connection was removed, disabled, or edited while in-flight.
                    still_live = any(c["id"] == cid and c.get("enabled") for c in self.connections)
                    if not still_live or self.conn_generations.get(cid) != gen:
                        continue
                    if time.time() < self._refresh_hold_until:
                        continue
                    self.results[cid] = {"rooms": rooms, "sensors": sensors, "light_count": lcount, "on_count": ocount}
                    self.connected[cid] = True
                    self.errors[cid] = ""
            except Exception as exc:  # noqa: BLE001
                err_safe = self.redact(str(exc), extra_secrets={conn.get("token", "")}) or "Connection failed"
                with self.lock:
                    still_live = any(c["id"] == cid and c.get("enabled") for c in self.connections)
                    if not still_live or self.conn_generations.get(cid) != gen:
                        continue
                    self.connected[cid] = False
                    self.errors[cid] = err_safe

    def set_light(self, rid, body):
        parts = rid.split(":", 2)
        if len(parts) < 3:
            return
        _prefix, cid, rest = parts[0], parts[1], parts[2]
        with self.lock:
            conn = next((dict(c) for c in self.connections if c["id"] == cid and c.get("enabled")), None)
            wled_fx = list(self.wled_effects.get(cid, []))
            shelly_meta = dict(self.shelly_meta.get(rid, {}))
            lifx_st = dict(self.lifx_state.get(cid, {}))
            dir_bounds = self.dirigera_kelvin.get(rid, (2200, 4000))
        if not conn:
            return
        kind = conn["kind"]
        host, port, token = conn["host"], conn["port"], conn.get("token", "")
        expected_fp = conn.get("cert_fingerprint", "")

        if kind == "home_assistant":
            turn_off = isinstance(body.get("on"), dict) and body["on"].get("on") is False
            svc = "turn_off" if turn_off else "turn_on"
            payload = {"entity_id": rest}
            if not turn_off:
                if isinstance(body.get("dimming"), dict):
                    payload["brightness_pct"] = int(clamp(round(finite(body["dimming"].get("brightness"), 100, 1, 100)), 1, 100))
                if isinstance(body.get("color"), dict) and isinstance(body["color"].get("xy"), dict):
                    r, g, b = xy_to_rgb(body["color"]["xy"].get("x"), body["color"]["xy"].get("y"))
                    payload["rgb_color"] = [r, g, b]
                elif isinstance(body.get("color_temperature"), dict):
                    payload["color_temp_kelvin"] = mirek_to_kelvin(body["color_temperature"].get("mirek"))
                if isinstance(body.get("effects"), dict):
                    eff = str(body["effects"].get("effect") or "")
                    if eff and eff != "no_effect":
                        payload["effect"] = eff
            _http_request(
                host, port, "POST", f"/api/services/light/{svc}",
                body=payload,
                headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
                use_https=conn["use_https"],
                timeout=5.0,
                allow_loopback=True,
                expected_fp=expected_fp,
            )
            return

        if kind == "wled":
            payload = build_wled_payload(body, wled_fx)
            if payload:
                _http_request(host, port, "POST", "/json/state", body=payload, timeout=4.0)
            return

        if kind == "wiz":
            _udp_exchange(host, port, build_wiz_set_pilot(body), expect_reply=False, timeout=1.5)
            return

        if kind == "lifx":
            if isinstance(body.get("on"), dict):
                is_on = bool(body["on"].get("on"))
                _udp_exchange(host, port, encode_lifx_set_power(is_on), expect_reply=False, timeout=1.5)
                lifx_st["power_on"] = is_on
            if any(k in body for k in ("dimming", "color", "color_temperature")):
                bri = finite((body.get("dimming") or {}).get("brightness"), lifx_st.get("bri", 80.0), 1.0, 100.0)
                hue = lifx_st.get("hue", 0.0)
                sat = lifx_st.get("sat", 0.0)
                kelvin = lifx_st.get("kelvin", 3200)
                if isinstance(body.get("color"), dict) and isinstance(body["color"].get("xy"), dict):
                    r, g, b = xy_to_rgb(body["color"]["xy"].get("x"), body["color"]["xy"].get("y"))
                    hue, sat, _ = rgb_to_hsb(r, g, b)
                elif isinstance(body.get("color_temperature"), dict):
                    sat = 0.0
                    kelvin = int(clamp(mirek_to_kelvin(body["color_temperature"].get("mirek")), 1500, 9000))
                pkt = encode_lifx_set_color(
                    int(clamp(round((hue / 360.0) * 65535.0), 0, 65535)),
                    int(clamp(round((sat / 100.0) * 65535.0), 0, 65535)),
                    int(clamp(round((bri / 100.0) * 65535.0), 1, 65535)),
                    int(clamp(kelvin, 1500, 9000)),
                )
                _udp_exchange(host, port, pkt, expect_reply=False, timeout=1.5)
                lifx_st.update({"hue": hue, "sat": sat, "bri": bri, "kelvin": kelvin})
            with self.lock:
                self.lifx_state[cid] = lifx_st
            return

        if kind == "govee":
            for pkt in build_govee_commands(body):
                _udp_exchange(host, port, pkt, expect_reply=False, timeout=1.5)
            return

        if kind == "nanoleaf":
            qt = urllib.parse.quote(token, safe="")
            if isinstance(body.get("effects"), dict):
                eff = str(body["effects"].get("effect") or "")
                if eff and eff != "no_effect":
                    _http_request(host, port, "PUT", f"/api/v1/{qt}/effects", body={"select": eff}, timeout=4.0)
                    return
            payload = {}
            if isinstance(body.get("on"), dict):
                payload["on"] = {"value": bool(body["on"].get("on"))}
            if isinstance(body.get("dimming"), dict):
                payload["brightness"] = {"value": int(clamp(round(finite(body["dimming"].get("brightness"), 100, 0, 100)), 0, 100))}
            if isinstance(body.get("color"), dict) and isinstance(body["color"].get("xy"), dict):
                r, g, b = xy_to_rgb(body["color"]["xy"].get("x"), body["color"]["xy"].get("y"))
                hue, sat, _ = rgb_to_hsb(r, g, b)
                payload["hue"] = {"value": int(clamp(round(hue), 0, 360))}
                payload["sat"] = {"value": int(clamp(round(sat), 0, 100))}
            elif isinstance(body.get("color_temperature"), dict):
                payload["ct"] = {"value": int(clamp(mirek_to_kelvin(body["color_temperature"].get("mirek")), 1200, 6500))}
            if payload:
                _http_request(host, port, "PUT", f"/api/v1/{qt}/state", body=payload, timeout=4.0)
            return

        if kind == "shelly" and shelly_meta:
            gen, skind, ch = shelly_meta["gen"], shelly_meta["kind"], shelly_meta["channel"]
            if gen == 2:
                method_map = {"rgb": "RGB.Set", "rgbw": "RGBW.Set", "switch": "Switch.Set"}
                rpc = method_map.get(skind, "Light.Set")
                payload = {"id": ch}
                if isinstance(body.get("on"), dict):
                    payload["on"] = bool(body["on"].get("on"))
                if isinstance(body.get("dimming"), dict) and skind != "switch":
                    payload["brightness"] = int(clamp(round(finite(body["dimming"].get("brightness"), 100, 0, 100)), 0, 100))
                if isinstance(body.get("color"), dict) and isinstance(body["color"].get("xy"), dict) and skind in ("rgb", "rgbw"):
                    r, g, b = xy_to_rgb(body["color"]["xy"].get("x"), body["color"]["xy"].get("y"))
                    payload["rgb"] = [r, g, b]
                _http_request(host, port, "POST", f"/rpc/{rpc}", body=payload, timeout=4.0)
            else:
                q = {}
                if isinstance(body.get("on"), dict):
                    q["turn"] = "on" if body["on"].get("on") else "off"
                if isinstance(body.get("dimming"), dict) and skind != "relay":
                    key = "gain" if skind == "color" else "brightness"
                    q[key] = int(clamp(round(finite(body["dimming"].get("brightness"), 100, 0, 100)), 0, 100))
                if isinstance(body.get("color"), dict) and isinstance(body["color"].get("xy"), dict) and skind == "color":
                    r, g, b = xy_to_rgb(body["color"]["xy"].get("x"), body["color"]["xy"].get("y"))
                    q["red"], q["green"], q["blue"] = r, g, b
                elif isinstance(body.get("color_temperature"), dict) and skind in ("light", "white"):
                    q["temp"] = int(clamp(mirek_to_kelvin(body["color_temperature"].get("mirek")), 2700, 6500))
                qs = ("?" + urllib.parse.urlencode(q)) if q else ""
                _http_request(host, port, "GET", f"/{skind}/{ch}{qs}", timeout=4.0)
            return

        if kind == "dirigera":
            attrs = {}
            if isinstance(body.get("on"), dict):
                attrs["isOn"] = bool(body["on"].get("on"))
            if isinstance(body.get("dimming"), dict):
                attrs["lightLevel"] = int(clamp(round(finite(body["dimming"].get("brightness"), 100, 1, 100)), 1, 100))
            if isinstance(body.get("color"), dict) and isinstance(body["color"].get("xy"), dict):
                r, g, b = xy_to_rgb(body["color"]["xy"].get("x"), body["color"]["xy"].get("y"))
                hue, sat, _ = rgb_to_hsb(r, g, b)
                attrs["colorHue"] = hue
                attrs["colorSaturation"] = round(clamp(sat / 100.0, 0.0, 1.0), 4)
            elif isinstance(body.get("color_temperature"), dict):
                lo_k, hi_k = dir_bounds
                attrs["colorTemperature"] = int(clamp(mirek_to_kelvin(body["color_temperature"].get("mirek")), lo_k, hi_k))
            if attrs:
                _http_request(
                    host, port, "PATCH", f"/v1/devices/{urllib.parse.quote(rest, safe='')}",
                    body=[{"attributes": attrs}],
                    headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
                    use_https=True,
                    timeout=4.5,
                    expected_fp=expected_fp,
                )

    def recall_scene(self, rid):
        parts = rid.split(":", 2)
        if len(parts) < 3:
            return
        _prefix, cid, rest = parts[0], parts[1], parts[2]
        with self.lock:
            conn = next((dict(c) for c in self.connections if c["id"] == cid and c.get("enabled")), None)
            nano_map = dict(self.nanoleaf_scenes.get(cid, {}))
        if not conn:
            return
        kind, host, port, token = conn["kind"], conn["host"], conn["port"], conn.get("token", "")
        expected_fp = conn.get("cert_fingerprint", "")
        if kind == "home_assistant":
            _http_request(
                host, port, "POST", "/api/services/scene/turn_on",
                body={"entity_id": rest},
                headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
                use_https=conn["use_https"],
                timeout=5.0,
                allow_loopback=True,
                expected_fp=expected_fp,
            )
        elif kind == "wled" and rest.startswith("ps:"):
            pid = int(rest[3:])
            _http_request(host, port, "POST", "/json/state", body={"on": True, "ps": pid}, timeout=4.0)
        elif kind == "wiz" and rest.startswith("scene:"):
            sid = int(rest[6:])
            _udp_exchange(host, port, build_wiz_set_pilot(scene_id=sid), expect_reply=False, timeout=1.5)
        elif kind == "nanoleaf" and rid in nano_map:
            qt = urllib.parse.quote(token, safe="")
            _http_request(host, port, "PUT", f"/api/v1/{qt}/effects", body={"select": nano_map[rid]}, timeout=4.0)
        elif kind == "dirigera" and rest.startswith("scene:"):
            sid = rest[6:]
            _http_request(
                host, port, "POST", f"/v1/scenes/{urllib.parse.quote(sid, safe='')}/trigger",
                headers={"Authorization": f"Bearer {token}"},
                use_https=True,
                timeout=4.0,
                expected_fp=expected_fp,
            )

    def snapshot_parts(self):
        """Return deep-copied (extra_rooms, extra_sensors, extra_light_count, extra_on_count, providers_list)."""
        with self.lock:
            rooms = []
            sensors = []
            lcount = 0
            ocount = 0
            plist = []
            for c in self.connections:
                cid = c["id"]
                res = self.results.get(cid) if c.get("enabled") else None
                plist.append({
                    "id": cid,
                    "kind": c["kind"],
                    "name": c["name"],
                    "host": c["host"],
                    "port": c["port"],
                    "use_https": c["use_https"],
                    "enabled": bool(c.get("enabled", True)),
                    "room_name": c.get("room_name", ""),
                    "has_token": bool(c.get("token")),
                    "connected": bool(self.connected.get(cid, False)) if c.get("enabled") else False,
                    "light_count": int(res["light_count"]) if res else 0,
                    "error": self.errors.get(cid, "") if c.get("enabled") else "",
                })
                if res:
                    rooms.extend(copy.deepcopy(res["rooms"]))
                    sensors.extend(copy.deepcopy(res["sensors"]))
                    lcount += res["light_count"]
                    ocount += res["on_count"]
            return rooms, sensors, lcount, ocount, plist
