#!/usr/bin/env python3
"""Local-first natural-language Ask AI engine for Maprisma Hue Lights.

Zero external dependencies (Python 3 standard library only). Works 100% offline
out-of-the-box for English and German commands (lights, rooms, scenes, themes,
schedules, dependency/sensor rules, MotionAware/sensor toggles, and status queries),
with optional local-only Ollama fallback (`http://127.0.0.1:11434`).

Security:
- Bridge secrets (app_key, client_key, fingerprint) are NEVER sent to Ollama or
  included in any reply.
- Only loopback HTTP Ollama URLs (127.0.0.1, localhost, [::1]) are permitted.
- All executed actions go through `hue.handle(...)` and Hue's built-in validators.
"""

import http.client
import json
import math
import re
import socket
import urllib.parse
import uuid


DEFAULT_OLLAMA_URL = "http://127.0.0.1:11434"
OLLAMA_PROBE_TIMEOUT = 0.35
OLLAMA_TAGS_TIMEOUT = 1.5
OLLAMA_CHAT_TIMEOUT = 15.0
OLLAMA_MAX_RESPONSE = 256 * 1024

UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
HEX_COLOR_RE = re.compile(r"#([0-9a-fA-F]{6})\b")
EFFECT_RE = re.compile(r"^[a-z0-9_]{1,32}$")
DAY_ORDER = ("mon", "tue", "wed", "thu", "fri", "sat", "sun")

# Named colors -> sRGB hex (EN + DE)
NAMED_COLORS = {
    "red": "#ff1a1a",
    "rot": "#ff1a1a",
    "rotes": "#ff1a1a",
    "rotem": "#ff1a1a",
    "crimson": "#dc143c",
    "orange": "#ff7b00",
    "amber": "#ffaa00",
    "bernstein": "#ffaa00",
    "gold": "#ffd000",
    "yellow": "#ffe600",
    "gelb": "#ffe600",
    "gelbes": "#ffe600",
    "lime": "#80ff00",
    "green": "#1aff33",
    "grün": "#1aff33",
    "gruen": "#1aff33",
    "grünes": "#1aff33",
    "mint": "#00ff99",
    "teal": "#00bfa5",
    "cyan": "#00e5ff",
    "turquoise": "#00e5ff",
    "türkis": "#00e5ff",
    "tuerkis": "#00e5ff",
    "blue": "#1a53ff",
    "blau": "#1a53ff",
    "blaues": "#1a53ff",
    "indigo": "#4b0082",
    "purple": "#9b26ff",
    "violet": "#8a2be2",
    "lila": "#9b26ff",
    "violett": "#8a2be2",
    "magenta": "#ff00cc",
    "pink": "#ff5ca8",
    "rosa": "#ff5ca8",
}

# Color temperature keywords -> mirek (153..500), checked longest-first
WARMTH_KEYWORDS = [
    ("candlelight", 454),
    ("kerzenlicht", 454),
    ("warm white", 370),
    ("warmweiß", 370),
    ("warmweiss", 370),
    ("soft white", 333),
    ("neutral white", 250),
    ("neutralweiß", 250),
    ("neutralweiss", 250),
    ("cool white", 153),
    ("cold white", 153),
    ("kaltweiß", 153),
    ("kaltweiss", 153),
    ("daylight", 182),
    ("tageslicht", 182),
    ("candle", 454),
    ("kerze", 454),
    ("gemütlich", 385),
    ("cozy", 385),
    ("warm", 370),
    ("neutral", 250),
    ("cool", 160),
    ("cold", 153),
    ("kalt", 153),
    ("kühl", 165),
    ("kuehl", 165),
    ("white", 250),
    ("weiß", 250),
    ("weiss", 250),
]

# Native Hue effect keywords -> CLIP v2 effect name, checked longest-first
EFFECT_KEYWORDS = [
    ("no effect", "no_effect"),
    ("stop effect", "no_effect"),
    ("clear effect", "no_effect"),
    ("kein effekt", "no_effect"),
    ("effekt aus", "no_effect"),
    ("effekt stoppen", "no_effect"),
    ("candlelight effect", "candle"),
    ("candle effect", "candle"),
    ("kerzeneffekt", "candle"),
    ("kerze effekt", "candle"),
    ("fire effect", "fire"),
    ("feuereffekt", "fire"),
    ("feuer effekt", "fire"),
    ("sparkle effect", "sparkle"),
    ("prism effect", "prism"),
    ("glisten effect", "glisten"),
    ("opal effect", "opal"),
    ("underwater effect", "underwater"),
    ("cosmos effect", "cosmos"),
    ("sunbeam effect", "sunbeam"),
    ("enchant effect", "enchant"),
    ("sparkle", "sparkle"),
    ("funkeln", "sparkle"),
    ("prism", "prism"),
    ("prisma", "prism"),
    ("glisten", "glisten"),
    ("glitzern", "glisten"),
    ("underwater", "underwater"),
    ("unterwasser", "underwater"),
    ("cosmos", "cosmos"),
    ("kosmos", "cosmos"),
    ("sunbeam", "sunbeam"),
    ("sonnenstrahl", "sunbeam"),
    ("enchant", "enchant"),
    ("verzaubern", "enchant"),
    ("fire", "fire"),
    ("feuer", "fire"),
    ("opal", "opal"),
]

# German/English aliases for built-in theme IDs
THEME_ALIASES = {
    "reading": "reading",
    "lesen": "reading",
    "relax": "relax",
    "entspannen": "relax",
    "entspannung": "relax",
    "focus": "focus",
    "fokus": "focus",
    "konzentration": "focus",
    "sunset glow": "sunset",
    "deep ocean": "ocean",
    "ozean": "ocean",
    "cyberpunk": "cyberpunk",
    "fireplace": "fireplace",
    "kamin": "fireplace",
    "kaminfeuer": "fireplace",
    "nightlight": "nightlight",
    "night light": "nightlight",
    "nachtlicht": "nightlight",
}

DAY_ALIASES = {
    "mon": "mon", "monday": "mon", "mondays": "mon",
    "montag": "mon", "montags": "mon",
    "tue": "tue", "tues": "tue", "tuesday": "tue", "tuesdays": "tue",
    "dienstag": "tue", "dienstags": "tue",
    "wed": "wed", "wednesday": "wed", "wednesdays": "wed",
    "mittwoch": "wed", "mittwochs": "wed",
    "thu": "thu", "thur": "thu", "thurs": "thu", "thursday": "thu", "thursdays": "thu",
    "donnerstag": "thu", "donnerstags": "thu",
    "fri": "fri", "friday": "fri", "fridays": "fri",
    "freitag": "fri", "freitags": "fri",
    "sat": "sat", "saturday": "sat", "saturdays": "sat",
    "samstag": "sat", "samstags": "sat",
    "sun": "sun", "sunday": "sun", "sundays": "sun",
    "sonntag": "sun", "sonntags": "sun",
}

GERMAN_HINT_RE = re.compile(
    r"[äöüß]|"
    r"\b(alle|lichter|lampen|licht|einschalten|ausschalten|schalte|mache|mach|"
    r"stelle|stell|dimme|dimmen|helligkeit|warmweiß|warmweiss|kaltweiß|kaltweiss|"
    r"tageslicht|wenn|sobald|falls|zwischen|und|welche|welches|sind|ist|"
    r"rot|grün|gruen|blau|gelb|lila|rosa|türkis|tuerkis|weiß|weiss|"
    r"zeitplan|sonnenaufgang|sonnenuntergang|werktags|wochentags|wochenende|"
    r"täglich|taeglich|jeden|bewegung|bewegungssensor|kontaktsensor|keine|"
    r"öffnet|oeffnet|schließt|schliesst|deaktiviere|aktiviere|szene|thema|"
    r"raum|zimmer|wohnzimmer|büro|buero|flur|küche|kueche|schlafzimmer|uhr|aus|auf|bei|im|in\s+dem)\b",
    re.IGNORECASE,
)


# ---------------------------------------------------------------------------
# Pure helpers

def _valid_uuid(value):
    return isinstance(value, str) and UUID_RE.match(value) is not None


def _clamp(value, low, high):
    return max(low, min(high, value))


def _clean(text, limit=120):
    text = "".join(ch for ch in str(text or "") if ch.isprintable() and ch not in "<>&")
    return text.strip()[:limit]


def _inv_gamma(c):
    c = _clamp(c, 0.0, 1.0)
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def _hex_to_xy(hex_str):
    if not isinstance(hex_str, str):
        return None
    s = hex_str.strip()
    m = re.fullmatch(r"#?([0-9a-fA-F]{6})", s)
    if not m:
        return None
    h = m.group(1)
    r = _inv_gamma(int(h[0:2], 16) / 255.0)
    g = _inv_gamma(int(h[2:4], 16) / 255.0)
    b = _inv_gamma(int(h[4:6], 16) / 255.0)
    X = r * 0.664511 + g * 0.154324 + b * 0.162028
    Y = r * 0.283881 + g * 0.668433 + b * 0.047685
    Z = r * 0.000088 + g * 0.072310 + b * 0.986039
    total = X + Y + Z
    if total <= 0.0:
        return [0.3127, 0.329]
    return [round(_clamp(X / total, 0.0, 1.0), 4), round(_clamp(Y / total, 0.0, 1.0), 4)]


def _redact(hue, text):
    s = str(text or "")
    fn = getattr(hue, "redact", None)
    if callable(fn):
        try:
            return fn(s)
        except Exception:
            pass
    cfg = getattr(hue, "config", None) or {}
    if isinstance(cfg, dict):
        for k in ("app_key", "client_key"):
            secret = cfg.get(k)
            if isinstance(secret, str) and len(secret) >= 6:
                s = s.replace(secret, "***")
    return s


def _detect_lang(prompt):
    """Return 'de' when the prompt looks German, otherwise 'en'."""
    low = (prompt or "").lower()
    if any(ch in low for ch in "äöüß"):
        return "de"
    de_hits = len(GERMAN_HINT_RE.findall(low))
    en_hits = len(re.findall(
        r"\b(the|turn|switch|dim|set|apply|activate|recall|enable|disable|"
        r"schedule|when|lights?|lamps?|rooms?|status|which|what|every|weekdays|"
        r"weekend|daily|sunrise|sunset|between|mirror|only|bright|warm|cool|off|on)\b",
        low,
    ))
    if de_hits > 0 and de_hits >= en_hits:
        return "de"
    return "en"


def _msg(lang, en, de):
    return de if lang == "de" else en


def _word_re(phrase):
    """Case-insensitive boundary-aware regex for a phrase (works with umlauts/digits)."""
    escaped = re.escape(phrase.strip())
    return re.compile(r"(?<!\w)" + escaped + r"(?!\w)", re.IGNORECASE)


def _find_matches(text, candidates, name_key="name"):
    """Find non-overlapping longest-name matches in `text`.

    Returns list of `(start, end, item)` sorted by position in `text`.
    Longer names always win over shorter overlapping names.
    """
    ordered = sorted(
        (c for c in candidates if str(c.get(name_key) or "").strip()),
        key=lambda c: len(str(c.get(name_key) or "").strip()),
        reverse=True,
    )
    occupied = []
    hits = []
    for item in ordered:
        name = str(item.get(name_key) or "").strip()
        if len(name) < 2:
            continue
        pat = _word_re(name)
        for m in pat.finditer(text):
            s, e = m.start(), m.end()
            if any(not (e <= os_ or s >= oe_) for os_, oe_ in occupied):
                continue
            occupied.append((s, e))
            hits.append((s, e, item))
            break
    hits.sort(key=lambda h: h[0])
    return hits


def _strip_spans(text, spans):
    """Replace matched character spans with spaces so entity names don't trigger keyword parsers."""
    if not spans:
        return text
    chars = list(text)
    for s, e in spans:
        for i in range(max(0, s), min(len(chars), e)):
            chars[i] = " "
    return "".join(chars)


# ---------------------------------------------------------------------------
# Ollama URL validation & safe local client

def validate_ollama_url(url):
    """Validate that `url` is a local loopback HTTP Ollama URL.

    Only allows `http://127.0.0.1[:port]`, `http://localhost[:port]`, or `http://[::1][:port]`.
    Returns `(host, port, base_path)` or raises `ValueError`.
    """
    if not isinstance(url, str) or not url.strip():
        raise ValueError("Ollama URL must be a non-empty string.")
    raw = url.strip()
    parsed = urllib.parse.urlsplit(raw)
    if parsed.scheme.lower() != "http":
        raise ValueError("Only http:// local loopback Ollama URLs are allowed.")
    if parsed.username is not None or parsed.password is not None:
        raise ValueError("Credentials in Ollama URL are not allowed.")
    if parsed.query or parsed.fragment:
        raise ValueError("Query or fragment in Ollama URL is not allowed.")
    host = (parsed.hostname or "").lower()
    if host not in ("127.0.0.1", "localhost", "::1"):
        raise ValueError("Non-loopback Ollama URL rejected (only 127.0.0.1 or localhost allowed).")
    try:
        port = parsed.port if parsed.port is not None else 11434
    except ValueError as exc:
        raise ValueError("Invalid port in Ollama URL.") from exc
    if not (1 <= port <= 65535):
        raise ValueError("Invalid port in Ollama URL.")
    base_path = (parsed.path or "").rstrip("/")
    return host, port, base_path


def _probe_tcp(host, port, timeout=OLLAMA_PROBE_TIMEOUT):
    """Fast TCP reachability probe so offline hosts never stall."""
    try:
        sock = socket.create_connection((host, port), timeout=timeout)
        sock.close()
        return True
    except OSError:
        return False


def _http_json(host, port, method, path, body=None, timeout=OLLAMA_CHAT_TIMEOUT):
    conn = http.client.HTTPConnection(host, port, timeout=timeout)
    try:
        headers = {"Accept": "application/json"}
        payload = None
        if body is not None:
            payload = json.dumps(body, separators=(",", ":")).encode("utf-8")
            headers["Content-Type"] = "application/json"
        conn.request(method, path, body=payload, headers=headers)
        resp = conn.getresponse()
        raw = resp.read(OLLAMA_MAX_RESPONSE + 1)
        if len(raw) > OLLAMA_MAX_RESPONSE:
            raise ValueError("Ollama response exceeded size limit.")
        if resp.status < 200 or resp.status >= 300:
            raise RuntimeError("Ollama returned HTTP %d" % resp.status)
        return json.loads(raw.decode("utf-8"))
    finally:
        conn.close()


# ---------------------------------------------------------------------------
# Catalog extraction from `hue.snapshot()` (zero secrets!)

_EXTRA_RESOURCE_ID_RE = re.compile(r"^[a-z][a-z0-9_]{1,20}:[A-Za-z0-9_-]{1,40}:[A-Za-z0-9_.:-]{1,120}$")


def _valid_resource_id(val):
    return _valid_uuid(val) or (isinstance(val, str) and bool(_EXTRA_RESOURCE_ID_RE.match(val)))


def _build_catalog(hue):
    snap = hue.snapshot() if callable(getattr(hue, "snapshot", None)) else {}
    rooms = []
    lights = []
    seen_lights = set()
    scenes = []
    for r in snap.get("rooms") or []:
        if not isinstance(r, dict):
            continue
        rkind = r.get("kind", "")
        if rkind in ("room", "zone") and _valid_resource_id(r.get("id")):
            rooms.append(r)
        for l in r.get("lights") or []:
            if isinstance(l, dict) and _valid_resource_id(l.get("id")) and l["id"] not in seen_lights:
                seen_lights.add(l["id"])
                entry = dict(l)
                entry["_room_id"] = r.get("id", "")
                entry["_room_name"] = r.get("name", "")
                lights.append(entry)
        for sc in r.get("scenes") or []:
            if isinstance(sc, dict) and _valid_resource_id(sc.get("id")):
                scenes.append({
                    "id": sc["id"],
                    "name": sc.get("name", ""),
                    "room_id": r.get("id", ""),
                    "room_name": r.get("name", ""),
                })
    themes = [t for t in (snap.get("themes") or []) if isinstance(t, dict) and t.get("id")]
    sensors = [s for s in (snap.get("sensors") or []) if isinstance(s, dict) and _valid_resource_id(s.get("id"))]
    motion_areas = [m for m in (snap.get("motion_areas") or []) if isinstance(m, dict) and _valid_resource_id(m.get("id"))]
    return {
        "snap": snap,
        "rooms": rooms,
        "only_rooms": [r for r in rooms if r.get("kind") == "room"],
        "lights": lights,
        "scenes": scenes,
        "themes": themes,
        "sensors": sensors,
        "motion_areas": motion_areas,
    }


def _is_paired(hue):
    if getattr(hue, "config", None):
        return True
    has_extra = getattr(hue, "_has_extra_providers", None)
    return bool(has_extra()) if callable(has_extra) else False


# ---------------------------------------------------------------------------
# Dispatch helpers (all go through `hue.handle(...)`)

def _dispatch_light_or_room(hue, target_kind, target, attrs):
    """Dispatch a validated light or room/zone control command via `hue.handle`."""
    payload = {}
    if "on" in attrs and isinstance(attrs["on"], bool):
        payload["on"] = attrs["on"]
    if "bri" in attrs and attrs["bri"] is not None:
        payload["bri"] = int(round(_clamp(float(attrs["bri"]), 1.0, 100.0)))
    if "mirek" in attrs and attrs["mirek"] is not None:
        payload["mirek"] = int(round(_clamp(float(attrs["mirek"]), 153.0, 500.0)))
    if "xy" in attrs and attrs["xy"] is not None:
        payload["xy"] = attrs["xy"]
    if "effect" in attrs and attrs["effect"] is not None:
        payload["effect"] = attrs["effect"]
    if not payload:
        return False

    if target_kind == "light":
        cmd = {"cmd": "light", "id": target["id"]}
        cmd.update(payload)
        hue.handle(cmd)
        return True

    gl_id = target.get("grouped_light", "")
    if _valid_uuid(gl_id):
        cmd = {"cmd": "group", "id": gl_id}
        cmd.update(payload)
        hue.handle(cmd)
        return True

    # Fallback when room/zone has no grouped_light UUID: control member lights
    dispatched = False
    for light in target.get("lights") or []:
        if _valid_uuid(light.get("id")):
            cmd = {"cmd": "light", "id": light["id"]}
            cmd.update(payload)
            hue.handle(cmd)
            dispatched = True
    return dispatched


def _apply_theme_to_target(hue, theme, target_kind, target):
    """Apply a Light Theme to a light or every light in a room/zone via `hue.handle`."""
    tid = theme["id"]
    if target_kind == "light":
        hue.handle({"cmd": "theme_apply", "theme_id": tid, "light_id": target["id"]})
        return True
    applied = False
    for light in target.get("lights") or []:
        if _valid_uuid(light.get("id")):
            hue.handle({"cmd": "theme_apply", "theme_id": tid, "light_id": light["id"]})
            applied = True
    return applied


# ---------------------------------------------------------------------------
# Intent parsers (English & German)

def _try_status_query(hue, cat, prompt, lang):
    low = prompt.strip().lower()
    is_status = bool(re.search(
        r"^(?:status|lichtstatus|übersicht|uebersicht|summary)\b|"
        r"\b(?:which|what|are any|how many)\s+(?:lights?|lamps?|rooms?)\s+(?:are|is)\s+(?:on|lit|off)\b|"
        r"\bwhat(?:'s| is)\s+(?:on|lit)\b|"
        r"\bshow\s+(?:me\s+)?(?:the\s+)?(?:light\s+)?status\b|"
        r"\bwelche\s+(?:lichter|lampen|räume|raeume)\s+sind\s+(?:an|ein|aus)\b|"
        r"\bwas\s+ist\s+(?:an|ein|aus)\b|"
        r"\bwie\s*viele\s+(?:lichter|lampen)\s+sind\s+(?:an|ein)\b|"
        r"\b(?:any|gibt\s+es)\s+motion\b|\baktive\s+bewegung\b",
        low,
    ))
    if not is_status:
        return None

    snap = cat["snap"]
    if not _is_paired(hue) and not cat["lights"]:
        return {
            "ok": True,
            "reply": _msg(lang, "No Hue Bridge is paired yet.", "Es ist noch keine Hue Bridge gekoppelt."),
            "actions": ["status"],
            "engine": "builtin",
        }

    on_count = int(snap.get("on_count", 0))
    light_count = int(snap.get("light_count", len(cat["lights"])))
    lit_rooms = []
    for r in snap.get("rooms") or []:
        if r.get("kind") == "zone":
            continue
        lit = [l for l in (r.get("lights") or []) if l.get("on")]
        if lit:
            names = ", ".join("%s (%d%%)" % (l["name"], int(round(l.get("bri", 0)))) for l in lit[:6])
            lit_rooms.append("%s: %s" % (r.get("name", "Room"), names))

    active_motion = []
    for s in cat["sensors"]:
        if s.get("kind") == "motion" and s.get("motion"):
            active_motion.append(s["name"])
    for a in cat["motion_areas"]:
        if a.get("enabled") and a.get("motion"):
            active_motion.append(a["name"])
    open_contacts = [s["name"] for s in cat["sensors"] if s.get("kind") == "contact" and s.get("open")]

    parts = []
    if lit_rooms:
        if lang == "de":
            parts.append("%d/%d Lichter an — %s." % (on_count, light_count, "; ".join(lit_rooms)))
        else:
            parts.append("%d/%d lights on — %s." % (on_count, light_count, "; ".join(lit_rooms)))
    else:
        parts.append(_msg(
            lang,
            "All lights are currently off (0/%d)." % light_count,
            "Alle Lichter sind derzeit aus (0/%d)." % light_count,
        ))

    if active_motion:
        parts.append(_msg(
            lang,
            "Active motion: %s." % ", ".join(active_motion),
            "Aktive Bewegung: %s." % ", ".join(active_motion),
        ))
    if open_contacts:
        parts.append(_msg(
            lang,
            "Open contacts: %s." % ", ".join(open_contacts),
            "Offene Kontakte: %s." % ", ".join(open_contacts),
        ))

    return {
        "ok": True,
        "reply": " ".join(parts),
        "actions": ["status"],
        "engine": "builtin",
    }


def _extract_time_range(text):
    """Extract optional 'between HH:MM and HH:MM' / 'zwischen HH:MM und HH:MM'.

    Returns `(time_start, time_end, cleaned_text)`.
    """
    m = re.search(
        r"\b(?:between|from|zwischen|von)\s+(\d{1,2}):(\d{2})\s+(?:and|to|und|bis|-)\s+(\d{1,2}):(\d{2})\b",
        text,
        re.IGNORECASE,
    )
    if not m:
        return "", "", text
    sh, sm, eh, em = int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4))
    if not (0 <= sh <= 23 and 0 <= sm <= 59 and 0 <= eh <= 23 and 0 <= em <= 59):
        return "", "", text
    cleaned = text[:m.start()] + " " + text[m.end():]
    return "%02d:%02d" % (sh, sm), "%02d:%02d" % (eh, em), cleaned


def _extract_brightness(text):
    """Extract brightness 1..100 from text, or None."""
    m = re.search(r"\b(\d{1,3})\s*(?:%|(?:percent|prozent)\b)", text, re.IGNORECASE)
    if m:
        val = int(m.group(1))
        return int(_clamp(val, 1, 100))
    m = re.search(
        r"\b(?:dim(?:men)?\s+(?:to|auf)|brightness(?:\s+to)?|helligkeit(?:\s+auf)?|bri|auf)\s+(\d{1,3})\b",
        text,
        re.IGNORECASE,
    )
    if m:
        val = int(m.group(1))
        return int(_clamp(val, 1, 100))
    if re.search(r"\b(?:half\s+brightness|halbe\s+helligkeit)\b", text, re.IGNORECASE):
        return 50
    if re.search(r"\b(?:full\s+brightness|max\s+brightness|volle\s+helligkeit|bright|hell)\b", text, re.IGNORECASE):
        return 100
    if re.search(r"\b(?:dim|dimmen|dimme|abdunkeln)\b", text, re.IGNORECASE):
        return 30
    return None


def _match_rule_trigger(cond_text, cat):
    """Resolve `(trigger_kind, trigger_id, trigger_name, trigger_state)` from a rule condition."""
    low = cond_text.lower()

    # Check desktop wake/sleep first when explicit
    if re.search(r"\b(?:desktop|computer|pc|screen|bildschirm)\b", low):
        if re.search(r"\b(?:sleep|sleeps|suspend|lock|locks|schläft|schlaeft|ruhezustand|sperrt)\b", low):
            return "desktop", "sleep", "Desktop Sleep", ""
        if re.search(r"\b(?:wake|wakes|unlock|unlocks|aufwacht|entsperrt)\b", low):
            return "desktop", "wake", "Desktop Wake", ""

    wants_motion_area = bool(re.search(r"\b(?:motion\s*aware|motion\s+area|bewegungsbereich)\b", low))
    wants_motion = bool(re.search(r"\b(?:motion|movement|bewegung|detects|erkennt|clears|no\s+motion|keine\s+bewegung)\b", low))
    wants_contact = bool(re.search(r"\b(?:contact|door|window|opens?|closes?|closed|kontakt|tür|tuer|fenster|öffnet|oeffnet|schließt|schliesst)\b", low))
    wants_button = bool(re.search(r"\b(?:button|switch|tap|press|pressed|long\s*press|taster|schalter|gedrückt|gedrueckt)\b", low))

    # Build combined candidate pool for longest-name matching
    pool = []
    for s in cat["sensors"]:
        pool.append({"_kind": s["kind"], "id": s["id"], "name": s["name"], "room_id": s.get("room_id", "")})
    for a in cat["motion_areas"]:
        pool.append({"_kind": "motion_area", "id": a["id"], "name": a["name"], "room_id": a.get("room_id", "")})
    for l in cat["lights"]:
        pool.append({"_kind": "light", "id": l["id"], "name": l["name"], "room_id": l.get("_room_id", "")})
    for r in cat["only_rooms"]:
        pool.append({"_kind": "room", "id": r["id"], "name": r["name"], "room_id": r["id"]})

    chosen = None

    # 1. If user explicitly asks for MotionAware, match motion_area by name or linked room
    if wants_motion_area and cat["motion_areas"]:
        ma_hits = _find_matches(cond_text, cat["motion_areas"])
        if ma_hits:
            a = ma_hits[0][2]
            chosen = ("motion_area", a["id"], a["name"])
        else:
            room_hits = _find_matches(cond_text, cat["only_rooms"])
            if room_hits:
                rid = room_hits[0][2]["id"]
                area = next((a for a in cat["motion_areas"] if a.get("room_id") == rid), None)
                if area is not None:
                    chosen = ("motion_area", area["id"], area["name"])
            if chosen is None:
                # Try prefix match against area names
                for a in cat["motion_areas"]:
                    base = re.sub(r"\b(?:motion|security|area|aware)\b", "", a["name"], flags=re.IGNORECASE).strip()
                    if len(base) >= 3 and _word_re(base).search(cond_text):
                        chosen = ("motion_area", a["id"], a["name"])
                        break

    # 2. Longest exact entity match across sensors, motion_areas, lights, rooms
    if chosen is None:
        hits = _find_matches(cond_text, pool)
        if hits:
            # Prefer longest match; break ties by intent keyword
            best_len = max(h[1] - h[0] for h in hits)
            top = [h[2] for h in hits if (h[1] - h[0]) == best_len]
            cand = top[0]
            # If a room matched, check if sensor/motion_area intent maps to a sensor in that room
            if cand["_kind"] == "room":
                rid = cand["id"]
                if wants_contact:
                    s = next((x for x in cat["sensors"] if x["kind"] == "contact" and x.get("room_id") == rid), None)
                    if s:
                        chosen = ("contact", s["id"], s["name"])
                elif wants_button:
                    s = next((x for x in cat["sensors"] if x["kind"] == "button" and x.get("room_id") == rid), None)
                    if s:
                        chosen = ("button", s["id"], s["name"])
                elif wants_motion:
                    s = next((x for x in cat["sensors"] if x["kind"] == "motion" and x.get("room_id") == rid), None)
                    a = next((x for x in cat["motion_areas"] if x.get("room_id") == rid), None)
                    if s:
                        chosen = ("motion", s["id"], s["name"])
                    elif a:
                        chosen = ("motion_area", a["id"], a["name"])
            if chosen is None:
                chosen = (cand["_kind"], cand["id"], cand["name"])

    # 3. Fallback partial match for sensor/area names when sensor keywords are present
    if chosen is None and (wants_motion or wants_contact or wants_button):
        for s in cat["sensors"]:
            if wants_contact and s["kind"] != "contact":
                continue
            if wants_button and s["kind"] != "button":
                continue
            if wants_motion and not wants_contact and not wants_button and s["kind"] != "motion":
                continue
            base = re.sub(r"\b(?:motion|contact|button|sensor|switch|bewegung|kontakt)\b", "", s["name"], flags=re.IGNORECASE).strip()
            if len(base) >= 3 and _word_re(base).search(cond_text):
                chosen = (s["kind"], s["id"], s["name"])
                break

    if chosen is None:
        return None

    tkind, tid, tname = chosen
    # Strip trigger name before inspecting trigger state
    state_text = _word_re(tname).sub(" ", cond_text).lower()
    if tkind in ("motion", "motion_area"):
        if re.search(r"\b(?:no\s+motion|clear|clears|stops?|ends?|keine\s+bewegung|frei|endet)\b", state_text):
            tstate = "clear"
        else:
            tstate = "motion"
    elif tkind == "contact":
        if re.search(r"\b(?:close|closes|closed|shut|shuts|schließt|schliesst|geschlossen|zu)\b", state_text):
            tstate = "closed"
        else:
            tstate = "open"
    elif tkind == "button":
        if re.search(r"\b(?:long\s*press|long_press|held|hold|lang|lange)\b", state_text):
            tstate = "long_press"
        else:
            tstate = "press"
    else:
        if re.search(r"\b(?:turns?\s+off|switches?\s+off|goes?\s+off|is\s+off|off|ausgeht|aus\s+ist|ausgeschaltet|ausschaltet|aus)\b", state_text):
            tstate = "off"
        else:
            tstate = "on"
    return tkind, tid, tname, tstate


def _split_rule_condition_action(text):
    """Split a conditional rule sentence into `(condition, action)`."""
    # Remove leading conditional keyword
    m_lead = re.match(r"^\s*(?:when|if|whenever|wenn|sobald|falls)\b\s*", text, re.IGNORECASE)
    body = text[m_lead.end():] if m_lead else text

    # Explicit separators: 'then', 'dann', '->', '→', ','
    for sep in (
        r"\s*,\s*(?:then|dann)\b\s*",
        r"\s+(?:then|dann)\b\s*",
        r"\s*(?:->|→)\s*",
        r"\s*,\s*",
    ):
        parts = re.split(sep, body, maxsplit=1, flags=re.IGNORECASE)
        if len(parts) == 2 and parts[0].strip() and parts[1].strip():
            return parts[0].strip(), parts[1].strip()

    # Implicit split before an imperative action verb (not at position 0)
    m_verb = re.search(
        r"\s+((?:turn|switch|set|dim|keep|activate|schalte|mache|mach|stelle|stell|dimme)\b.+)$",
        body,
        re.IGNORECASE,
    )
    if m_verb and m_verb.start() > 2:
        return body[:m_verb.start()].strip(), m_verb.group(1).strip()

    return None, None


def _try_rule(hue, cat, prompt, lang):
    if not re.search(r"\b(?:when|if|whenever|wenn|sobald|falls)\b", prompt, re.IGNORECASE):
        return None
    # Guard: "turn on X only if off" without an actual trigger clause is not a rule
    if not re.match(r"^\s*(?:when|if|whenever|wenn|sobald|falls)\b", prompt, re.IGNORECASE):
        # Allow prefix like "create a rule: when ..." / "erstelle regel: wenn ..."
        m_cond = re.search(r"\b(when|if|whenever|wenn|sobald|falls)\b", prompt, re.IGNORECASE)
        if not m_cond:
            return None
        prefix = prompt[:m_cond.start()].lower()
        if not re.search(r"\b(?:rule|automation|regel|erstelle|create|add|always|immer)\b", prefix):
            return None
        work = prompt[m_cond.start():]
    else:
        work = prompt

    time_start, time_end, work = _extract_time_range(work)
    cond_text, action_text = _split_rule_condition_action(work)
    if not cond_text or not action_text:
        return None

    trig = _match_rule_trigger(cond_text, cat)
    if trig is None:
        return None
    tkind, tid, tname, tstate = trig

    # Extract rule options from action_text and strip option phrases before detecting primary action
    act_clean = action_text
    mirror_off = False
    only_if_off = False
    match_bri = False

    mirror_pat = re.compile(
        r"\b(?:and\s+)?(?:mirror(?:\s+off)?|also\s+turn(?:\s+it|\s+them)?\s+off(?:\s+when\s+\w+)?|"
        r"turn\s+(?:it|them)\s+off\s+too|follow\s+off|"
        r"(?:und\s+)?(?:auch\s+(?:wieder\s+)?aus(?:schalten)?|ebenfalls\s+aus(?:schalten)?|mit\s+ausschalten|spiegeln))\b",
        re.IGNORECASE,
    )
    if mirror_pat.search(act_clean):
        mirror_off = True
        act_clean = mirror_pat.sub(" ", act_clean)

    only_off_pat = re.compile(
        r"\b(?:only\s+(?:if|when)\s+(?:it(?:'s|\s+is)\s+|they(?:'re|\s+are)\s+)?off|"
        r"nur\s+(?:wenn|falls)\s+(?:es\s+|sie\s+)?aus(?:geschaltet)?(?:\s+ist|\s+sind)?)\b",
        re.IGNORECASE,
    )
    if only_off_pat.search(act_clean):
        only_if_off = True
        act_clean = only_off_pat.sub(" ", act_clean)

    match_bri_pat = re.compile(
        r"\b(?:(?:and\s+)?match\s+brightness|same\s+brightness|sync\s+brightness|"
        r"(?:und\s+)?helligkeit\s+(?:angleichen|übernehmen|uebernehmen)|gleiche\s+helligkeit)\b",
        re.IGNORECASE,
    )
    if match_bri_pat.search(act_clean):
        match_bri = True
        act_clean = match_bri_pat.sub(" ", act_clean)

    # Match targets (lights and rooms only; validate_rule accepts kind in ("light", "room"))
    target_pool = []
    for l in cat["lights"]:
        target_pool.append({"_kind": "light", "id": l["id"], "name": l["name"]})
    for r in cat["only_rooms"]:
        target_pool.append({"_kind": "room", "id": r["id"], "name": r["name"]})

    hits = _find_matches(act_clean, target_pool)
    targets_raw = []
    matched_spans = []
    seen_t = set()
    for s, e, item in hits:
        if item["_kind"] == tkind and item["id"] == tid:
            continue
        key = (item["_kind"], item["id"])
        if key not in seen_t:
            seen_t.add(key)
            targets_raw.append(item)
            matched_spans.append((s, e))

    if not targets_raw:
        return None

    act_modifiers = _strip_spans(act_clean, matched_spans).lower()
    is_off = bool(re.search(
        r"\b(?:turn\s+off|switch\s+off|shut\s+off|off|ausschalten|ausmachen|abschalten|aus)\b",
        act_modifiers,
    ))
    action = "off" if is_off else "on"
    bri = _extract_brightness(act_modifiers) if action == "on" else None

    targets = []
    for item in targets_raw:
        scene_id = ""
        if item["_kind"] == "room" and action == "on":
            room_scenes = [sc for sc in cat["scenes"] if sc["room_id"] == item["id"]]
            sc_hits = _find_matches(act_modifiers, room_scenes)
            if sc_hits:
                scene_id = sc_hits[0][2]["id"]
        targets.append({
            "kind": item["_kind"],
            "id": item["id"],
            "action": action,
            "bri": bri,
            "scene_id": scene_id,
        })

    if tkind in ("motion", "motion_area", "contact", "button"):
        rkind = "sensor"
    elif tkind == "desktop":
        rkind = "desktop"
    elif action == "off" and tstate == "on":
        rkind = "exclusive"
    elif only_if_off or match_bri:
        rkind = "companion"
    else:
        rkind = "follow"

    rule_id = str(uuid.uuid4())
    target_names = ", ".join(t["name"] for t in targets_raw)
    rule_name = _clean("%s -> %s (%s)" % (tname, target_names, action), 60)
    rule_dict = {
        "id": rule_id,
        "name": rule_name,
        "enabled": True,
        "kind": rkind,
        "trigger_kind": tkind,
        "trigger_id": tid,
        "trigger_state": tstate,
        "targets": targets,
        "mirror_off": mirror_off,
        "match_bri": match_bri,
        "time_start": time_start,
        "time_end": time_end,
        "only_if_off": only_if_off,
    }

    hue.handle({"cmd": "rule_save", "rule": rule_dict})

    saved = any(isinstance(r, dict) and r.get("id") == rule_id for r in (getattr(hue, "rules", None) or []))
    if not saved:
        err = _redact(hue, getattr(hue, "error", "") or "Rule was rejected by validation.")
        return {
            "ok": False,
            "reply": _msg(lang, "Could not save rule: %s" % err, "Regel konnte nicht gespeichert werden: %s" % err),
            "actions": [],
            "engine": "builtin",
        }

    extras = []
    if bri is not None:
        extras.append("%d%%" % bri)
    if mirror_off:
        extras.append(_msg(lang, "mirror off", "mit Ausschalten"))
    if only_if_off:
        extras.append(_msg(lang, "only if off", "nur wenn aus"))
    if match_bri:
        extras.append(_msg(lang, "match brightness", "Helligkeit angleichen"))
    if time_start and time_end:
        extras.append("%s-%s" % (time_start, time_end))
    extra_str = (" [%s]" % ", ".join(extras)) if extras else ""

    reply = _msg(
        lang,
        "Created rule '%s': when %s is %s, turn %s %s%s." % (rule_name, tname, tstate or "triggered", action, target_names, extra_str),
        "Regel '%s' erstellt: wenn %s (%s), schalte %s %s%s." % (rule_name, tname, tstate or "ausgelöst", target_names, "aus" if action == "off" else "ein", extra_str),
    )
    return {
        "ok": True,
        "reply": reply,
        "actions": ["rule_save:%s" % rule_name],
        "engine": "builtin",
    }


def _parse_schedule_time_and_days(text):
    """Extract `(trigger_kind, time_str, offset_min, days, cleaned_text)` or None."""
    work = text
    # Prevent "Sunset Glow" theme name from looking like a sunset schedule
    masked = re.sub(r"\bsunset\s+glow\b", "           ", work, flags=re.IGNORECASE)

    trigger_kind = None
    time_str = ""
    offset_min = 0

    # 1. Solar triggers: sunrise / sunset
    m_solar = re.search(
        r"(?:(\d{1,3})\s*(?:min(?:utes?|uten)?|m)\s+(before|after|vor|nach)\s+)?"
        r"\b(sunrise|sunset|sonnenaufgang|sonnenuntergang)\b"
        r"(?:\s*([+-]\s*\d{1,3})\s*(?:min(?:utes?|uten)?|m)?)?",
        masked,
        re.IGNORECASE,
    )
    if m_solar:
        word = m_solar.group(3).lower()
        trigger_kind = "sunrise" if word in ("sunrise", "sonnenaufgang") else "sunset"
        if m_solar.group(1) and m_solar.group(2):
            mins = int(m_solar.group(1))
            direction = m_solar.group(2).lower()
            offset_min = -mins if direction in ("before", "vor") else mins
        elif m_solar.group(4):
            offset_min = int(m_solar.group(4).replace(" ", ""))
        offset_min = int(_clamp(offset_min, -120, 120))
        work = work[:m_solar.start()] + " " + work[m_solar.end():]
    else:
        # 2. Explicit HH:MM [am|pm]
        m_hm = re.search(r"\b(\d{1,2}):(\d{2})\s*(am|pm|uhr)?\b", work, re.IGNORECASE)
        if m_hm:
            hh, mm = int(m_hm.group(1)), int(m_hm.group(2))
            ampm = (m_hm.group(3) or "").lower()
            if ampm == "pm" and 1 <= hh <= 11:
                hh += 12
            elif ampm == "am" and hh == 12:
                hh = 0
            if 0 <= hh <= 23 and 0 <= mm <= 59:
                trigger_kind = "time"
                time_str = "%02d:%02d" % (hh, mm)
                work = work[:m_hm.start()] + " " + work[m_hm.end():]
        else:
            # 3. "7am" / "10 pm" / "7 Uhr [30]"
            m_ampm = re.search(r"\b(\d{1,2})\s*(am|pm)\b", work, re.IGNORECASE)
            m_uhr = re.search(r"\b(\d{1,2})\s*uhr(?:\s*(\d{1,2}))?\b", work, re.IGNORECASE)
            if m_ampm:
                hh = int(m_ampm.group(1))
                ampm = m_ampm.group(2).lower()
                if ampm == "pm" and 1 <= hh <= 11:
                    hh += 12
                elif ampm == "am" and hh == 12:
                    hh = 0
                if 0 <= hh <= 23:
                    trigger_kind = "time"
                    time_str = "%02d:00" % hh
                    work = work[:m_ampm.start()] + " " + work[m_ampm.end():]
            elif m_uhr:
                hh = int(m_uhr.group(1))
                mm = int(m_uhr.group(2)) if m_uhr.group(2) else 0
                if 0 <= hh <= 23 and 0 <= mm <= 59:
                    trigger_kind = "time"
                    time_str = "%02d:%02d" % (hh, mm)
                    work = work[:m_uhr.start()] + " " + work[m_uhr.end():]

    if trigger_kind is None:
        return None

    # Parse recurrence days
    days = set()
    if re.search(r"\b(?:weekdays?|workdays?|werktags?|wochentags?|mon\s*-\s*fri|mo\s*-\s*fr)\b", work, re.IGNORECASE):
        days.update(("mon", "tue", "wed", "thu", "fri"))
        work = re.sub(r"\b(?:weekdays?|workdays?|werktags?|wochentags?|mon\s*-\s*fri|mo\s*-\s*fr)\b", " ", work, flags=re.IGNORECASE)
    if re.search(r"\b(?:weekends?|wochenenden?)\b", work, re.IGNORECASE):
        days.update(("sat", "sun"))
        work = re.sub(r"\b(?:weekends?|wochenenden?)\b", " ", work, flags=re.IGNORECASE)
    if re.search(r"\b(?:daily|everyday|every\s+day|täglich|taeglich|jeden\s+tag)\b", work, re.IGNORECASE):
        days.update(DAY_ORDER)
        work = re.sub(r"\b(?:daily|everyday|every\s+day|täglich|taeglich|jeden\s+tag)\b", " ", work, flags=re.IGNORECASE)

    for alias, short in DAY_ALIASES.items():
        pat = _word_re(alias)
        if pat.search(work):
            days.add(short)
            work = pat.sub(" ", work)

    ordered_days = [d for d in DAY_ORDER if d in days]
    return trigger_kind, time_str, offset_min, ordered_days, work


def _try_schedule(hue, cat, prompt, lang):
    # Must contain a schedule cue or solar trigger (excluding "Sunset Glow" theme)
    masked = re.sub(r"\bsunset\s+glow\b", "           ", prompt, flags=re.IGNORECASE)
    has_cue = bool(re.search(
        r"\b(?:at|um|bei|zum|schedule|zeitplan|timer|every|jeden|jede|daily|täglich|taeglich|"
        r"weekdays?|werktags?|wochentags?|weekends?|wochenende|sunrise|sunset|sonnenaufgang|sonnenuntergang)\b",
        masked,
        re.IGNORECASE,
    ))
    if not has_cue:
        return None

    parsed = _parse_schedule_time_and_days(prompt)
    if parsed is None:
        return None
    trigger_kind, time_str, offset_min, days, rest = parsed

    if not _is_paired(hue):
        return {
            "ok": False,
            "reply": _msg(lang, "Bridge is not paired yet.", "Bridge ist noch nicht gekoppelt."),
            "actions": [],
            "engine": "builtin",
        }

    # Match target (rooms, zones, lights)
    pool = []
    for r in cat["rooms"]:
        pool.append({"_kind": r["kind"], "id": r["id"], "name": r["name"]})
    for l in cat["lights"]:
        pool.append({"_kind": "light", "id": l["id"], "name": l["name"]})

    hits = _find_matches(rest, pool)
    if not hits:
        return None
    s, e, target = hits[0]
    mod_text = _strip_spans(rest, [(s, e)]).lower()

    # Scene check (if target is a room/zone)
    scene_id = ""
    if target["_kind"] in ("room", "zone"):
        room_scenes = [sc for sc in cat["scenes"] if sc["room_id"] == target["id"]]
        sc_hits = _find_matches(mod_text, room_scenes)
        if sc_hits:
            scene_id = sc_hits[0][2]["id"]

    if scene_id:
        action = "scene"
        bri = None
    elif re.search(r"\b(?:turn\s+off|switch\s+off|shut\s+off|off|ausschalten|ausmachen|abschalten|aus)\b", mod_text):
        action = "off"
        bri = None
    else:
        action = "on"
        bri = _extract_brightness(mod_text)

    when_label = time_str if trigger_kind == "time" else trigger_kind
    sched_name = _clean("%s %s %s" % (target["name"], action, when_label), 60)

    cmd = {
        "cmd": "schedule_create",
        "name": sched_name,
        "enabled": True,
        "target_kind": target["_kind"],
        "target_id": target["id"],
        "trigger_kind": trigger_kind,
        "days": days,
        "action": action,
    }
    if trigger_kind == "time":
        cmd["time"] = time_str
    else:
        cmd["offset_min"] = offset_min
        # Local schedules only support trigger_kind == "time", so route solar schedules to bridge
        cmd["source"] = "bridge"
    if action == "on" and bri is not None:
        cmd["bri"] = bri
    if action == "scene":
        cmd["scene_id"] = scene_id

    before_local = len(getattr(hue, "local_schedules", []) or [])
    before_api = len(getattr(hue, "api_queue", []) or [])
    hue.handle(cmd)
    after_local = len(getattr(hue, "local_schedules", []) or [])
    after_api = len(getattr(hue, "api_queue", []) or [])

    if after_local == before_local and after_api == before_api:
        return {
            "ok": False,
            "reply": _msg(lang, "Could not create schedule for %s." % target["name"], "Zeitplan für %s konnte nicht erstellt werden." % target["name"]),
            "actions": [],
            "engine": "builtin",
        }

    days_str = (" (%s)" % ",".join(days)) if days else ""
    bri_str = (" at %d%%" % bri) if (action == "on" and bri is not None) else ""
    reply = _msg(
        lang,
        "Scheduled %s %s at %s%s%s." % (target["name"], action, when_label, bri_str, days_str),
        "Zeitplan erstellt: %s um %s %s%s%s." % (target["name"], when_label, "aus" if action == "off" else ("Szene" if action == "scene" else "ein"), bri_str, days_str),
    )
    return {
        "ok": True,
        "reply": reply,
        "actions": ["schedule_create:%s" % sched_name],
        "engine": "builtin",
    }


def _try_all_on_off(hue, cat, clause, lang):
    low = clause.lower()
    has_all = bool(re.search(
        r"\b(?:all\s+(?:the\s+)?(?:lights?|lamps?|rooms?)|everything|every\s+(?:light|lamp)|whole\s+(?:house|home)|"
        r"alle\s+(?:lichter|lampen|räume|raeume)|alles|jedes\s+licht|ganzes?\s+haus)\b",
        low,
    ))
    if not has_all:
        return None

    # Check for "except <room>" / "außer <room>"
    exclude_ids = []
    exclude_names = []
    m_exc = re.search(r"\b(?:except|without|außer|ausser|ohne)\b(.+)$", clause, re.IGNORECASE)
    if m_exc:
        exc_hits = _find_matches(m_exc.group(1), cat["only_rooms"])
        for _, _, r in exc_hits:
            exclude_ids.append(r["id"])
            exclude_names.append(r["name"])
        check_text = clause[:m_exc.start()]
    else:
        check_text = clause

    # If a specific room or light is named (outside "except"), this is a room/light command instead!
    if _find_matches(check_text, cat["rooms"]) or _find_matches(check_text, cat["lights"]):
        return None

    check_low = check_text.lower()
    is_off = bool(re.search(r"\b(?:off|aus|ausschalten|ausmachen|abschalten|shut\s+down)\b", check_low))
    is_on = bool(re.search(r"\b(?:on|an|ein|einschalten|anmachen|anschalten)\b", check_low))
    if not is_off and not is_on:
        return None

    if not _is_paired(hue):
        return {
            "ok": False,
            "reply": _msg(lang, "Bridge is not paired yet.", "Bridge ist noch nicht gekoppelt."),
            "actions": [],
            "engine": "builtin",
        }

    if is_off:
        hue.handle({"cmd": "all_off"})
        return {
            "ok": True,
            "reply": _msg(lang, "Turned off all lights.", "Alle Lichter ausgeschaltet."),
            "actions": ["all_off"],
            "engine": "builtin",
        }

    cmd = {"cmd": "all_on"}
    if exclude_ids:
        cmd["exclude_rooms"] = exclude_ids
    hue.handle(cmd)
    if exclude_names:
        return {
            "ok": True,
            "reply": _msg(
                lang,
                "Turned on all lights (except %s)." % ", ".join(exclude_names),
                "Alle Lichter eingeschaltet (außer %s)." % ", ".join(exclude_names),
            ),
            "actions": ["all_on"],
            "engine": "builtin",
        }
    return {
        "ok": True,
        "reply": _msg(lang, "Turned on all lights.", "Alle Lichter eingeschaltet."),
        "actions": ["all_on"],
        "engine": "builtin",
    }


def _try_sensor_toggle(hue, cat, clause, lang):
    low = clause.lower()
    wants_ma = bool(re.search(r"\b(?:motion\s*aware|motion\s+area|bewegungsbereich)\b", low))
    wants_sensor_kw = bool(re.search(
        r"\b(?:sensor|motion|bewegung|bewegungssensor|contact|kontakt|kontaktsensor|sensitivity|empfindlichkeit)\b",
        low,
    ))
    toggleable_sensors = [s for s in cat["sensors"] if s.get("kind") in ("motion", "contact")]

    # Direct longest-name matches on motion_areas and toggleable sensors
    ma_hits = _find_matches(clause, cat["motion_areas"])
    s_hits = _find_matches(clause, toggleable_sensors)

    target_type = None  # "motion_area" or "sensor"
    target = None

    if wants_ma and cat["motion_areas"]:
        if ma_hits:
            target_type, target = "motion_area", ma_hits[0][2]
        else:
            room_hits = _find_matches(clause, cat["only_rooms"])
            if room_hits:
                rid = room_hits[0][2]["id"]
                area = next((a for a in cat["motion_areas"] if a.get("room_id") == rid), None)
                if area is not None:
                    target_type, target = "motion_area", area
            if target is None:
                for a in cat["motion_areas"]:
                    base = re.sub(r"\b(?:motion|security|area|aware)\b", "", a["name"], flags=re.IGNORECASE).strip()
                    if len(base) >= 3 and _word_re(base).search(clause):
                        target_type, target = "motion_area", a
                        break

    if target is None and (ma_hits or s_hits):
        best_ma = (ma_hits[0][1] - ma_hits[0][0], ma_hits[0][2]) if ma_hits else (-1, None)
        best_s = (s_hits[0][1] - s_hits[0][0], s_hits[0][2]) if s_hits else (-1, None)
        if best_s[0] >= best_ma[0] and best_s[1] is not None:
            target_type, target = "sensor", best_s[1]
        elif best_ma[1] is not None:
            target_type, target = "motion_area", best_ma[1]

    if target is None and wants_sensor_kw:
        # Try matching via room name or stripped sensor/area base name
        room_hits = _find_matches(clause, cat["only_rooms"])
        if room_hits:
            rid = room_hits[0][2]["id"]
            if re.search(r"\b(?:contact|kontakt)\b", low):
                s = next((x for x in toggleable_sensors if x["kind"] == "contact" and x.get("room_id") == rid), None)
                if s:
                    target_type, target = "sensor", s
            else:
                s = next((x for x in toggleable_sensors if x["kind"] == "motion" and x.get("room_id") == rid), None)
                a = next((x for x in cat["motion_areas"] if x.get("room_id") == rid), None)
                if s and not wants_ma:
                    target_type, target = "sensor", s
                elif a:
                    target_type, target = "motion_area", a
        if target is None:
            for s in toggleable_sensors:
                base = re.sub(r"\b(?:motion|contact|sensor|bewegung|kontakt)\b", "", s["name"], flags=re.IGNORECASE).strip()
                if len(base) >= 3 and _word_re(base).search(clause):
                    target_type, target = "sensor", s
                    break

    if target is None:
        return None

    # Strip matched name before checking enable/disable/sensitivity
    mod_text = _word_re(target["name"]).sub(" ", clause).lower()
    disable_hit = bool(re.search(
        r"\b(?:disable|deactivate|turn\s+off|switch\s+off|off|pause|deaktiviere|deaktivieren|ausschalten|aus)\b",
        mod_text,
    ))
    enable_hit = bool(re.search(
        r"\b(?:enable|activate|turn\s+on|switch\s+on|on|aktiviere|aktivieren|einschalten|an|ein)\b",
        mod_text,
    ))
    sens_match = re.search(r"\b(?:sensitivity|empfindlichkeit)\s*(?:to|auf|=)?\s*(\d+)\b", mod_text)
    sens_val = int(sens_match.group(1)) if sens_match else None

    if not disable_hit and not enable_hit and sens_val is None:
        return None

    if not _is_paired(hue):
        return {
            "ok": False,
            "reply": _msg(lang, "Bridge is not paired yet.", "Bridge ist noch nicht gekoppelt."),
            "actions": [],
            "engine": "builtin",
        }

    enabled = None
    if disable_hit:
        enabled = False
    elif enable_hit:
        enabled = True

    if target_type == "sensor":
        if enabled is None:
            return None
        hue.handle({"cmd": "sensor_toggle", "kind": target["kind"], "id": target["id"], "enabled": enabled})
        state_en = "Enabled" if enabled else "Disabled"
        state_de = "aktiviert" if enabled else "deaktiviert"
        return {
            "ok": True,
            "reply": _msg(lang, "%s %s." % (state_en, target["name"]), "%s %s." % (target["name"], state_de)),
            "actions": ["sensor_toggle:%s:%s" % (target["name"], "on" if enabled else "off")],
            "engine": "builtin",
        }

    cmd = {"cmd": "motion_area_update", "id": target["id"]}
    if enabled is not None:
        cmd["enabled"] = enabled
    if sens_val is not None:
        smax = target.get("sensitivity_max", 4)
        cmd["sensitivity"] = int(_clamp(sens_val, 0, smax))
    hue.handle(cmd)
    parts_en, parts_de = [], []
    if enabled is not None:
        parts_en.append("enabled" if enabled else "disabled")
        parts_de.append("aktiviert" if enabled else "deaktiviert")
    if "sensitivity" in cmd:
        parts_en.append("sensitivity %d" % cmd["sensitivity"])
        parts_de.append("Empfindlichkeit %d" % cmd["sensitivity"])
    return {
        "ok": True,
        "reply": _msg(
            lang,
            "Updated MotionAware %s (%s)." % (target["name"], ", ".join(parts_en)),
            "MotionAware %s aktualisiert (%s)." % (target["name"], ", ".join(parts_de)),
        ),
        "actions": ["motion_area_update:%s" % target["name"]],
        "engine": "builtin",
    }


def _find_target_in_clause(clause, cat, fallback_target=None):
    """Find best room/zone/light target in `clause`, returning `(kind, item, spans)` or fallback."""
    pool = []
    for l in cat["lights"]:
        pool.append({"_kind": "light", "id": l["id"], "name": l["name"], "_obj": l})
    for r in cat["rooms"]:
        pool.append({"_kind": r["kind"], "id": r["id"], "name": r["name"], "_obj": r})

    hits = _find_matches(clause, pool)
    if hits:
        # Longest name match wins
        best = max(hits, key=lambda h: h[1] - h[0])
        return best[2]["_kind"], best[2]["_obj"], [(best[0], best[1])]

    if fallback_target is not None:
        return fallback_target[0], fallback_target[1], []
    return None, None, []


def _try_theme_or_scene(hue, cat, clause, lang, fallback_target=None):
    low = clause.lower()
    explicit_theme = bool(re.search(r"\b(?:theme|thema)\b", low))
    explicit_scene = bool(re.search(r"\b(?:scene|szene)\b", low))

    target_kind, target_obj, target_spans = _find_target_in_clause(clause, cat, fallback_target=None)
    search_text = _strip_spans(clause, target_spans)

    # Match themes by exact name (built-in + custom) or German/English alias
    theme_match = None
    theme_hits = _find_matches(search_text, cat["themes"])
    if theme_hits:
        theme_match = theme_hits[0][2]
    else:
        for alias in sorted(THEME_ALIASES.keys(), key=len, reverse=True):
            if _word_re(alias).search(search_text):
                tid = THEME_ALIASES[alias]
                found = next((t for t in cat["themes"] if t["id"] == tid), None)
                if found is not None:
                    theme_match = found
                    break

    # Match bridge scenes (prefer scenes belonging to matched room if one was specified)
    scene_match = None
    if target_kind in ("room", "zone") and target_obj is not None:
        room_scenes = [sc for sc in cat["scenes"] if sc["room_id"] == target_obj["id"]]
        sc_hits = _find_matches(search_text, room_scenes)
        if sc_hits:
            scene_match = sc_hits[0][2]
    if scene_match is None:
        sc_hits = _find_matches(search_text, cat["scenes"])
        if sc_hits:
            scene_match = sc_hits[0][2]

    if theme_match is None and scene_match is None:
        return None, None

    if not _is_paired(hue):
        return {
            "ok": False,
            "reply": _msg(lang, "Bridge is not paired yet.", "Bridge ist noch nicht gekoppelt."),
            "actions": [],
            "engine": "builtin",
        }, None

    # Decide between Theme and Scene
    use_scene = False
    if explicit_scene and scene_match is not None:
        use_scene = True
    elif explicit_theme and theme_match is not None:
        use_scene = False
    elif scene_match is not None and theme_match is None:
        use_scene = True
    elif theme_match is not None and scene_match is None:
        use_scene = False
    else:
        # Both matched (e.g., a room has a scene named "Relax" AND there's a "Relax" theme)
        if target_kind == "light":
            use_scene = False
        elif target_kind in ("room", "zone") and target_obj is not None and scene_match["room_id"] == target_obj["id"]:
            # If user says "apply <theme>" -> theme; otherwise recall the room's scene
            use_scene = not bool(re.search(r"\b(?:apply|anwenden)\b", low))
        else:
            use_scene = False

    if use_scene:
        hue.handle({"cmd": "scene", "id": scene_match["id"]})
        room_part = (" (%s)" % scene_match["room_name"]) if scene_match.get("room_name") else ""
        return {
            "ok": True,
            "reply": _msg(
                lang,
                "Activated scene '%s'%s." % (scene_match["name"], room_part),
                "Szene '%s'%s aktiviert." % (scene_match["name"], room_part),
            ),
            "actions": ["scene:%s" % scene_match["name"]],
            "engine": "builtin",
        }, (target_kind, target_obj) if target_obj is not None else fallback_target

    # Apply Theme
    if target_obj is None:
        if fallback_target is not None:
            target_kind, target_obj = fallback_target
        elif len(cat["only_rooms"]) == 1:
            target_kind, target_obj = "room", cat["only_rooms"][0]
        elif len(cat["lights"]) == 1:
            target_kind, target_obj = "light", cat["lights"][0]
        else:
            return {
                "ok": False,
                "reply": _msg(
                    lang,
                    "Which light or room should I apply theme '%s' to?" % theme_match["name"],
                    "Auf welches Licht oder welchen Raum soll das Thema '%s' angewendet werden?" % theme_match["name"],
                ),
                "actions": [],
                "engine": "builtin",
            }, None

    _apply_theme_to_target(hue, theme_match, target_kind, target_obj)
    return {
        "ok": True,
        "reply": _msg(
            lang,
            "Applied theme '%s' to %s." % (theme_match["name"], target_obj["name"]),
            "Thema '%s' auf %s angewendet." % (theme_match["name"], target_obj["name"]),
        ),
        "actions": ["theme_apply:%s:%s" % (theme_match["id"], target_obj["name"])],
        "engine": "builtin",
    }, (target_kind, target_obj)


def _extract_direct_attrs(text):
    """Extract direct light/room control attributes (`on`, `bri`, `mirek`, `xy`, `effect`)."""
    attrs = {}
    work = text

    # 1. Effects (check before warmth so "candle effect" becomes effect="candle", not just warmth!)
    for kw, eff in EFFECT_KEYWORDS:
        pat = _word_re(kw)
        if pat.search(work):
            attrs["effect"] = eff
            work = pat.sub(" ", work)
            break

    # 2. Hex color
    m_hex = HEX_COLOR_RE.search(work)
    if m_hex:
        xy = _hex_to_xy("#" + m_hex.group(1))
        if xy is not None:
            attrs["xy"] = xy
            work = work[:m_hex.start()] + " " + work[m_hex.end():]

    # 3. Explicit Kelvin or mirek
    m_k = re.search(r"\b(\d{4})\s*k\b", work, re.IGNORECASE)
    if m_k:
        kelvin = int(m_k.group(1))
        if 1500 <= kelvin <= 10000:
            attrs["mirek"] = int(round(_clamp(1000000.0 / float(kelvin), 153.0, 500.0)))
            work = work[:m_k.start()] + " " + work[m_k.end():]
    else:
        m_m = re.search(r"\b(\d{3})\s*mirek\b", work, re.IGNORECASE)
        if m_m:
            attrs["mirek"] = int(_clamp(int(m_m.group(1)), 153, 500))
            work = work[:m_m.start()] + " " + work[m_m.end():]

    # 4. Warmth keywords (only if no explicit xy/mirek yet)
    if "mirek" not in attrs and "xy" not in attrs:
        for kw, mirek in WARMTH_KEYWORDS:
            pat = _word_re(kw)
            if pat.search(work):
                attrs["mirek"] = mirek
                work = pat.sub(" ", work)
                break

    # 5. Named colors (only if no xy yet; named colors override generic mirek if both aren't set)
    if "xy" not in attrs:
        for cname in sorted(NAMED_COLORS.keys(), key=len, reverse=True):
            pat = _word_re(cname)
            if pat.search(work):
                xy = _hex_to_xy(NAMED_COLORS[cname])
                if xy is not None:
                    attrs["xy"] = xy
                    attrs.pop("mirek", None)
                    work = pat.sub(" ", work)
                    break

    # 6. Brightness
    bri = _extract_brightness(work)
    if bri is not None:
        attrs["bri"] = bri

    # 7. Power on/off
    low = work.lower()
    is_off = bool(re.search(
        r"\b(?:turn\s+off|switch\s+off|shut\s+off|power\s+off|off|ausschalten|ausmachen|abschalten|aus)\b",
        low,
    ))
    is_on = bool(re.search(
        r"\b(?:turn\s+on|switch\s+on|power\s+on|on|einschalten|anmachen|anschalten|an)\b|"
        r"(?:\bein\s*$|\bein\s*[.,!?])",
        low,
    ))

    has_active_attr = any(k in attrs for k in ("bri", "mirek", "xy")) or (
        attrs.get("effect") not in (None, "no_effect")
    )
    if is_off and not has_active_attr:
        attrs["on"] = False
    elif is_on and not is_off:
        attrs["on"] = True

    return attrs


def _describe_attrs(attrs, lang):
    bits = []
    if attrs.get("on") is False:
        return _msg(lang, "off", "aus")
    if attrs.get("on") is True and len(attrs) == 1:
        return _msg(lang, "on", "an")
    if "bri" in attrs:
        bits.append("%d%%" % attrs["bri"])
    if "mirek" in attrs:
        kelvin = int(round(1000000.0 / float(attrs["mirek"])))
        bits.append("%dK" % kelvin)
    if "xy" in attrs:
        bits.append(_msg(lang, "color", "Farbe"))
    if "effect" in attrs:
        if attrs["effect"] == "no_effect":
            bits.append(_msg(lang, "no effect", "kein Effekt"))
        else:
            bits.append(_msg(lang, "%s effect" % attrs["effect"], "Effekt %s" % attrs["effect"]))
    if not bits and attrs.get("on") is True:
        bits.append(_msg(lang, "on", "an"))
    return ", ".join(bits)


def _try_direct_control(hue, cat, clause, lang, fallback_target=None, fallback_attrs=None):
    target_kind, target_obj, spans = _find_target_in_clause(clause, cat, fallback_target=None)
    matched_own_target = target_obj is not None
    if target_obj is None and fallback_target is not None:
        target_kind, target_obj = fallback_target
        spans = []
    if target_obj is None:
        return None, None, None

    mod_text = _strip_spans(clause, spans)
    attrs = _extract_direct_attrs(mod_text)
    if not attrs:
        if matched_own_target and fallback_attrs:
            attrs = dict(fallback_attrs)
        else:
            return None, (target_kind, target_obj), None

    if not _is_paired(hue):
        return {
            "ok": False,
            "reply": _msg(lang, "Bridge is not paired yet.", "Bridge ist noch nicht gekoppelt."),
            "actions": [],
            "engine": "builtin",
        }, (target_kind, target_obj), None

    if not _dispatch_light_or_room(hue, target_kind, target_obj, attrs):
        return None, (target_kind, target_obj), None

    desc = _describe_attrs(attrs, lang)
    action_tag = "%s:%s:%s" % ("light" if target_kind == "light" else "group", target_obj["name"], desc)
    reply = _msg(
        lang,
        "Set %s to %s." % (target_obj["name"], desc),
        "%s auf %s gesetzt." % (target_obj["name"], desc),
    )
    return {
        "ok": True,
        "reply": reply,
        "actions": [action_tag],
        "engine": "builtin",
    }, (target_kind, target_obj), attrs


def _split_clauses(prompt):
    """Split a non-rule, non-schedule prompt into logical clauses."""
    # Split on ';', '\n', ',', or ' and ' / ' und ' / ' then ' / ' dann '
    raw_parts = re.split(
        r"\s*(?:;|\n|,|\b(?:and|und|then|dann)\b)\s*",
        prompt.strip(),
        flags=re.IGNORECASE,
    )
    return [p.strip() for p in raw_parts if p and p.strip()]


def _run_builtin(hue, cat, prompt, lang):
    # 1. Status query
    res = _try_status_query(hue, cat, prompt, lang)
    if res is not None:
        return res

    # 2. Conditional rule (handle whole prompt as one unit so "between A and B" isn't split)
    res = _try_rule(hue, cat, prompt, lang)
    if res is not None:
        return res

    # 3. Schedule creation (handle whole prompt so "mon, tue and wed" isn't split)
    res = _try_schedule(hue, cat, prompt, lang)
    if res is not None:
        return res

    # 4. Multi-clause execution (all on/off, sensor/MotionAware toggle, theme/scene, direct control)
    clauses = _split_clauses(prompt)
    if not clauses:
        return None

    replies = []
    actions = []
    last_target = None
    last_attrs = None
    pending_targets = []
    any_error = None

    for clause in clauses:
        # 4a. All on / all off
        res = _try_all_on_off(hue, cat, clause, lang)
        if res is not None:
            if not res["ok"]:
                any_error = res
                break
            replies.append(res["reply"])
            actions.extend(res["actions"])
            last_target = None
            last_attrs = None
            pending_targets.clear()
            continue

        # 4b. Sensor / MotionAware toggle
        res = _try_sensor_toggle(hue, cat, clause, lang)
        if res is not None:
            if not res["ok"]:
                any_error = res
                break
            replies.append(res["reply"])
            actions.extend(res["actions"])
            last_target = None
            last_attrs = None
            pending_targets.clear()
            continue

        # 4c. Theme or Scene recall
        res, new_target = _try_theme_or_scene(hue, cat, clause, lang, fallback_target=last_target)
        if res is not None:
            if not res["ok"]:
                any_error = res
                break
            replies.append(res["reply"])
            actions.extend(res["actions"])
            if new_target is not None:
                last_target = new_target
            last_attrs = None
            pending_targets.clear()
            continue

        # 4d. Direct light / room control (supports shared action across "A and B off" or "turn on A and B")
        res, new_target, used_attrs = _try_direct_control(
            hue, cat, clause, lang, fallback_target=last_target, fallback_attrs=last_attrs,
        )
        if res is not None:
            if not res["ok"]:
                any_error = res
                break
            if pending_targets and used_attrs:
                desc = _describe_attrs(used_attrs, lang)
                for pkind, pobj in pending_targets:
                    if _dispatch_light_or_room(hue, pkind, pobj, used_attrs):
                        actions.append("%s:%s:%s" % ("light" if pkind == "light" else "group", pobj["name"], desc))
                        replies.append(_msg(
                            lang,
                            "Set %s to %s." % (pobj["name"], desc),
                            "%s auf %s gesetzt." % (pobj["name"], desc),
                        ))
                pending_targets.clear()
            replies.append(res["reply"])
            actions.extend(res["actions"])
            if new_target is not None:
                last_target = new_target
            if used_attrs is not None:
                last_attrs = used_attrs
            continue

        # Bare target in a list like "Sofa Lamp and Desk Lamp off" -> buffer for next clause's attrs
        if new_target is not None:
            pending_targets.append(new_target)
            continue

        # Unrecognized clause: if nothing matched yet, abort so Ollama fallback can try the whole prompt
        if not actions and not pending_targets:
            return None

    if any_error is not None:
        return any_error
    if actions:
        return {
            "ok": True,
            "reply": " ".join(replies),
            "actions": actions,
            "engine": "builtin",
        }
    return None


# ---------------------------------------------------------------------------
# Optional local Ollama fallback (strictly loopback-only, zero secret leakage)

def _build_ollama_context(cat):
    """Build a compact, secret-free JSON summary of the Hue bridge for Ollama."""
    return {
        "rooms": [
            {
                "id": r["id"],
                "kind": r["kind"],
                "name": r["name"],
                "grouped_light": r.get("grouped_light", ""),
                "on": bool(r.get("on")),
                "bri": r.get("bri", 0),
                "lights": [
                    {
                        "id": l["id"],
                        "name": l["name"],
                        "on": bool(l.get("on")),
                        "bri": l.get("bri", 0),
                    }
                    for l in (r.get("lights") or [])
                ],
                "scenes": [{"id": s["id"], "name": s["name"]} for s in (r.get("scenes") or [])],
            }
            for r in cat["rooms"]
        ],
        "themes": [{"id": t["id"], "name": t["name"]} for t in cat["themes"]],
        "sensors": [
            {"id": s["id"], "kind": s["kind"], "name": s["name"], "enabled": bool(s.get("enabled", True))}
            for s in cat["sensors"]
        ],
        "motion_areas": [
            {"id": a["id"], "name": a["name"], "enabled": bool(a.get("enabled", True))}
            for a in cat["motion_areas"]
        ],
    }


def _execute_ollama_commands(hue, cat, commands, lang):
    """Strictly validate and execute allowlisted commands returned by Ollama via `hue.handle`."""
    allowed_cmds = {
        "all_on", "all_off", "light", "group", "scene",
        "theme_apply", "schedule_create", "rule_save",
        "sensor_toggle", "motion_area_update",
    }
    actions = []
    if not isinstance(commands, list):
        return actions

    rooms_by_id = {r["id"]: r for r in cat["rooms"]}
    for raw in commands[:16]:
        if not isinstance(raw, dict):
            continue
        cname = raw.get("cmd")
        if cname not in allowed_cmds:
            continue
        if cname in ("all_on", "all_off"):
            if not _is_paired(hue):
                continue
            cmd = {"cmd": cname}
            if cname == "all_on" and isinstance(raw.get("exclude_rooms"), list):
                cmd["exclude_rooms"] = [x for x in raw["exclude_rooms"] if _valid_uuid(x)]
            hue.handle(cmd)
            actions.append(cname)
        elif cname in ("light", "group"):
            if not _is_paired(hue):
                continue
            rid = raw.get("id")
            if not _valid_uuid(rid):
                continue
            cmd = {"cmd": cname, "id": rid}
            if isinstance(raw.get("on"), bool):
                cmd["on"] = raw["on"]
            if isinstance(raw.get("bri"), (int, float)) and not isinstance(raw.get("bri"), bool):
                cmd["bri"] = int(round(_clamp(float(raw["bri"]), 1.0, 100.0)))
            if isinstance(raw.get("mirek"), (int, float)) and not isinstance(raw.get("mirek"), bool):
                cmd["mirek"] = int(round(_clamp(float(raw["mirek"]), 153.0, 500.0)))
            if isinstance(raw.get("xy"), list) and len(raw["xy"]) == 2:
                cmd["xy"] = raw["xy"]
            elif isinstance(raw.get("hex"), str):
                xy = _hex_to_xy(raw["hex"])
                if xy is not None:
                    cmd["xy"] = xy
            if isinstance(raw.get("effect"), str) and EFFECT_RE.match(raw["effect"]):
                cmd["effect"] = raw["effect"]
            if len(cmd) > 2:
                hue.handle(cmd)
                actions.append("%s:%s" % (cname, rid))
        elif cname == "scene":
            if not _is_paired(hue):
                continue
            sid = raw.get("id")
            if _valid_uuid(sid):
                hue.handle({"cmd": "scene", "id": sid})
                actions.append("scene:%s" % sid)
        elif cname == "theme_apply":
            if not _is_paired(hue):
                continue
            tid = raw.get("theme_id")
            if not isinstance(tid, str) or not tid:
                continue
            lid = raw.get("light_id")
            rid = raw.get("room_id")
            if _valid_uuid(lid):
                hue.handle({"cmd": "theme_apply", "theme_id": tid, "light_id": lid})
                actions.append("theme_apply:%s:%s" % (tid, lid))
            elif _valid_uuid(rid) and rid in rooms_by_id:
                for l in rooms_by_id[rid].get("lights") or []:
                    if _valid_uuid(l.get("id")):
                        hue.handle({"cmd": "theme_apply", "theme_id": tid, "light_id": l["id"]})
                actions.append("theme_apply:%s:%s" % (tid, rid))
        elif cname == "schedule_create":
            if not _is_paired(hue):
                continue
            cmd = dict(raw)
            if cmd.get("trigger_kind") in ("sunrise", "sunset"):
                cmd["source"] = "bridge"
            before_local = len(getattr(hue, "local_schedules", []) or [])
            before_api = len(getattr(hue, "api_queue", []) or [])
            hue.handle(cmd)
            if (len(getattr(hue, "local_schedules", []) or []) > before_local
                    or len(getattr(hue, "api_queue", []) or []) > before_api):
                actions.append("schedule_create:%s" % _clean(cmd.get("name") or "Schedule", 40))
        elif cname == "rule_save":
            rule = raw.get("rule")
            if isinstance(rule, dict):
                rule_copy = dict(rule)
                rid = rule_copy.get("id")
                if not _valid_uuid(rid):
                    rid = str(uuid.uuid4())
                    rule_copy["id"] = rid
                hue.handle({"cmd": "rule_save", "rule": rule_copy})
                if any(isinstance(r, dict) and r.get("id") == rid for r in (getattr(hue, "rules", None) or [])):
                    actions.append("rule_save:%s" % _clean(rule_copy.get("name") or "Rule", 40))
        elif cname == "sensor_toggle":
            if not _is_paired(hue):
                continue
            if raw.get("kind") in ("motion", "contact") and _valid_uuid(raw.get("id")) and isinstance(raw.get("enabled"), bool):
                hue.handle({"cmd": "sensor_toggle", "kind": raw["kind"], "id": raw["id"], "enabled": raw["enabled"]})
                actions.append("sensor_toggle:%s" % raw["id"])
        elif cname == "motion_area_update":
            if not _is_paired(hue):
                continue
            if _valid_uuid(raw.get("id")):
                cmd = {"cmd": "motion_area_update", "id": raw["id"]}
                if isinstance(raw.get("enabled"), bool):
                    cmd["enabled"] = raw["enabled"]
                if isinstance(raw.get("sensitivity"), int) and not isinstance(raw.get("sensitivity"), bool):
                    cmd["sensitivity"] = raw["sensitivity"]
                if len(cmd) > 2:
                    hue.handle(cmd)
                    actions.append("motion_area_update:%s" % raw["id"])
    return actions


def _run_ollama(hue, cat, prompt, ollama_url, ollama_model, lang):
    try:
        host, port, base_path = validate_ollama_url(ollama_url)
    except ValueError as exc:
        return {
            "ok": False,
            "reply": _redact(hue, str(exc)),
            "actions": [],
            "engine": "ollama",
        }

    if not _probe_tcp(host, port, timeout=OLLAMA_PROBE_TIMEOUT):
        return None

    model = (ollama_model or "").strip()
    if not model:
        try:
            tags = _http_json(host, port, "GET", base_path + "/api/tags", timeout=OLLAMA_TAGS_TIMEOUT)
            models = tags.get("models") if isinstance(tags, dict) else None
            if isinstance(models, list) and models and isinstance(models[0], dict):
                model = str(models[0].get("name") or "").strip()
        except Exception:
            return None
    if not model:
        return None

    system_prompt = (
        "You control Philips Hue lights locally. Return ONLY a JSON object: "
        '{"reply": "<short confirmation>", "commands": [<command objects>]}. '
        "Allowed commands: all_on, all_off, "
        '{"cmd":"light","id":"<uuid>","on":bool,"bri":1-100,"mirek":153-500,"hex":"#rrggbb","effect":"candle|fire|sparkle|prism|no_effect"}, '
        '{"cmd":"group","id":"<grouped_light_uuid>",...}, '
        '{"cmd":"scene","id":"<scene_uuid>"}, '
        '{"cmd":"theme_apply","theme_id":"<id>","light_id":"<uuid>"}, '
        '{"cmd":"sensor_toggle","kind":"motion|contact","id":"<uuid>","enabled":bool}, '
        '{"cmd":"motion_area_update","id":"<uuid>","enabled":bool,"sensitivity":int}, '
        '{"cmd":"schedule_create",...}, {"cmd":"rule_save","rule":{...}}. '
        "Use ONLY IDs from this bridge state:\n"
        + json.dumps(_build_ollama_context(cat), separators=(",", ":"))
    )

    try:
        resp = _http_json(
            host,
            port,
            "POST",
            base_path + "/api/chat",
            body={
                "model": model,
                "stream": False,
                "format": "json",
                "messages": [
                    {"role": "system", "content": system_prompt},
                    {"role": "user", "content": prompt},
                ],
            },
            timeout=OLLAMA_CHAT_TIMEOUT,
        )
    except Exception:
        return None

    content = ""
    if isinstance(resp, dict):
        msg_obj = resp.get("message")
        if isinstance(msg_obj, dict):
            content = str(msg_obj.get("content") or "")
    if not content:
        return None

    try:
        parsed = json.loads(content)
    except ValueError:
        return None
    if not isinstance(parsed, dict):
        return None

    cmds = parsed.get("commands")
    if cmds is None and "cmd" in parsed:
        cmds = [parsed]
    actions = _execute_ollama_commands(hue, cat, cmds or [], lang)
    reply = _clean(_redact(hue, parsed.get("reply") or ""), 400)
    if not actions and not reply:
        return None
    if not reply:
        reply = _msg(lang, "Executed %d action(s)." % len(actions), "%d Aktion(en) ausgeführt." % len(actions))
    return {
        "ok": bool(actions) or bool(reply),
        "reply": reply,
        "actions": [_redact(hue, a) for a in actions],
        "engine": "ollama",
    }


# ---------------------------------------------------------------------------
# Public entry point

def execute_ask_ai(hue, prompt, ollama_url=DEFAULT_OLLAMA_URL, ollama_model=""):
    """Interpret a natural-language prompt (EN or DE) and execute safe Hue actions on `hue`.

    Returns: {"ok": bool, "reply": str, "actions": [str], "engine": "builtin"|"ollama"}
    """
    if not isinstance(prompt, str) or not prompt.strip():
        return {
            "ok": False,
            "reply": "Please enter a command.",
            "actions": [],
            "engine": "builtin",
        }

    text = prompt.strip()[:500]
    lang = _detect_lang(text)
    cat = _build_catalog(hue)

    # 1. Always try the zero-latency built-in offline parser first
    builtin_res = _run_builtin(hue, cat, text, lang)
    if builtin_res is not None:
        builtin_res["reply"] = _redact(hue, builtin_res.get("reply", ""))
        builtin_res["actions"] = [_redact(hue, a) for a in builtin_res.get("actions", [])]
        return builtin_res

    # 2. Optional local-only Ollama fallback (fast 0.35s probe; rejects non-loopback URLs)
    if ollama_url:
        ollama_res = _run_ollama(hue, cat, text, ollama_url, ollama_model, lang)
        if ollama_res is not None:
            ollama_res["reply"] = _redact(hue, ollama_res.get("reply", ""))
            ollama_res["actions"] = [_redact(hue, a) for a in ollama_res.get("actions", [])]
            return ollama_res

    # 3. Clean offline fallback reply
    return {
        "ok": False,
        "reply": _msg(
            lang,
            "I couldn't match a light, room, scene, theme, schedule, sensor, or rule in that request.",
            "Ich konnte kein passendes Licht, keinen Raum, keine Szene, kein Thema, keinen Zeitplan, Sensor oder keine Regel erkennen.",
        ),
        "actions": [],
        "engine": "builtin",
    }
