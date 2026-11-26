#!/usr/bin/env python3
"""MCP (Model Context Protocol) JSON-RPC 2.0 stdio server for Maprisma Lights.

Reuses scripts/hue-lights.py directly (zero external dependencies, Python 3 stdlib only).
All bridge communication goes through Hue.api() / Hue.refresh() with pinned TLS.
Bridge credentials (app_key, client_key, certificate fingerprint) are never exposed.
"""

import difflib
import importlib.util
import json
import math
from pathlib import Path
import signal
import sys
import time


SERVER_NAME = "maprisma-hue-lights"
SERVER_VERSION = "0.5.2"
DEFAULT_PROTOCOL_VERSION = "2024-11-05"
SUPPORTED_PROTOCOL_VERSIONS = ("2024-11-05", "2025-03-26")
STATE_RESOURCE_URI = "hue://state"
MAX_LINE_BYTES = 512 * 1024  # 512 KiB per JSON-RPC message line
DEFAULT_STALE_SECONDS = 1.5

_HUE_MODULE = None


def load_hue_module(script_path=None):
    """Load scripts/hue-lights.py from the same directory via importlib (works with python3 -I)."""
    global _HUE_MODULE
    if script_path is None and _HUE_MODULE is not None:
        return _HUE_MODULE
    path = Path(script_path) if script_path is not None else (Path(__file__).resolve().parent / "hue-lights.py")
    spec = importlib.util.spec_from_file_location("hue_lights", str(path))
    if spec is None or spec.loader is None:
        raise ImportError("Unable to load hue-lights.py from %s" % path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    if script_path is None:
        _HUE_MODULE = mod
    return mod


class ToolError(Exception):
    """Expected user/validation/resolution/bridge error returned as MCP tool isError=True."""


class RpcError(Exception):
    """JSON-RPC 2.0 protocol error."""

    def __init__(self, code, message, data=None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data


def _is_number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(float(value))


def _is_int(value):
    return isinstance(value, int) and not isinstance(value, bool)


def _format_candidate(item, id_key="id", name_key="name", extra_fn=None):
    name = item.get(name_key) or ""
    iid = item.get(id_key) or ""
    extra = extra_fn(item) if extra_fn else ""
    if extra:
        return "%s (%s, id=%s)" % (name, extra, iid)
    return "%s (id=%s)" % (name, iid)


def _resolve_item(query, items, kind_label, id_key="id", name_key="name", extra_fn=None):
    """Resolve an item by exact ID or case-insensitive name.

    Raises ToolError listing close candidates if ambiguous or not found.
    """
    if not isinstance(query, str) or not query.strip():
        raise ToolError("Invalid %s identifier: must be a non-empty string." % kind_label)
    q = query.strip()
    q_low = q.lower()

    # 1. Exact ID match (case-insensitive for UUIDs / theme IDs)
    for item in items:
        iid = str(item.get(id_key) or "")
        if iid and (iid == q or iid.lower() == q_low):
            return item

    # 2. Exact case-insensitive name match
    exact = [it for it in items if str(it.get(name_key) or "").strip().lower() == q_low]
    if len(exact) == 1:
        return exact[0]
    if len(exact) > 1:
        opts = ", ".join(_format_candidate(it, id_key, name_key, extra_fn) for it in exact[:8])
        raise ToolError("Ambiguous %s '%s'. Matches: %s. Specify the ID to disambiguate." % (kind_label, q, opts))

    # 3. Not found -> build helpful candidate suggestions
    if not items:
        raise ToolError("%s '%s' not found (no %ss available)." % (kind_label.capitalize(), q, kind_label))

    scored = []
    seen_ids = set()
    for it in items:
        nm = str(it.get(name_key) or "").strip()
        nm_low = nm.lower()
        iid = str(it.get(id_key) or "")
        if nm_low and (q_low in nm_low or nm_low in q_low):
            if iid not in seen_ids:
                seen_ids.add(iid)
                scored.append(it)

    name_to_items = {}
    for it in items:
        nm_low = str(it.get(name_key) or "").strip().lower()
        if nm_low:
            name_to_items.setdefault(nm_low, []).append(it)
    for close_name in difflib.get_close_matches(q_low, list(name_to_items.keys()), n=5, cutoff=0.45):
        for it in name_to_items[close_name]:
            iid = str(it.get(id_key) or "")
            if iid not in seen_ids:
                seen_ids.add(iid)
                scored.append(it)

    if scored:
        suggestions = ", ".join(_format_candidate(it, id_key, name_key, extra_fn) for it in scored[:6])
        raise ToolError("%s '%s' not found. Did you mean: %s?" % (kind_label.capitalize(), q, suggestions))

    available = ", ".join(_format_candidate(it, id_key, name_key, extra_fn) for it in items[:8])
    suffix = " (and %d more)" % (len(items) - 8) if len(items) > 8 else ""
    raise ToolError("%s '%s' not found. Available %ss: %s%s." % (kind_label.capitalize(), q, kind_label, available, suffix))


# ---------------------------------------------------------------------------
# Tool definitions (JSON Schemas)

TOOLS = [
    {
        "name": "hue_status",
        "description": "Get Philips Hue Bridge status, model info (including Bridge Pro detection), light/room/sensor/MotionAware counts, and rule pause state.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "refresh": {"type": "boolean", "description": "Force a fresh read from the Hue Bridge."},
            },
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_list_rooms",
        "description": "List all Philips Hue rooms and zones with power state, brightness, blended color, member lights, scenes, and any linked MotionAware area.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["room", "zone", "other"],
                    "description": "Optional filter by group kind.",
                },
                "refresh": {"type": "boolean", "description": "Force a fresh read from the Hue Bridge."},
            },
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_list_lights",
        "description": "List Philips Hue lights with live power, brightness, color, color temperature (mirek), effect, reachability, and MotionAware status.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "room": {"type": "string", "description": "Optional room/zone UUID or case-insensitive name filter."},
                "only_on": {"type": "boolean", "description": "When true, return only lights that are currently turned on."},
                "refresh": {"type": "boolean", "description": "Force a fresh read from the Hue Bridge."},
            },
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_list_sensors",
        "description": "List all physical Hue sensors (motion, contact, button/switch with temperature, lux, battery, last_event) and Hue Bridge Pro MotionAware areas.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["motion", "contact", "button", "motion_area"],
                    "description": "Optional filter by sensor kind.",
                },
                "refresh": {"type": "boolean", "description": "Force a fresh read from the Hue Bridge."},
            },
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_list_themes",
        "description": "List all built-in and custom Light Themes (with brightness, color hex, mirek, and effect).",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_list_rules",
        "description": "List dependency and sensor automation rules along with whether rules are paused.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "refresh": {"type": "boolean", "description": "Force a fresh read from the Hue Bridge."},
            },
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_list_schedules",
        "description": "List Philips Hue Bridge schedules and local schedules.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "refresh": {"type": "boolean", "description": "Force a fresh read from the Hue Bridge."},
            },
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_get_floorplan",
        "description": "Get the current Maprisma Lights floorplan document (template_id, canvas, room/zone boxes, lamp positions, and sensor positions).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "refresh": {"type": "boolean", "description": "Force a fresh read from the Hue Bridge."},
            },
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_set_light",
        "description": "Control a single Philips Hue light by UUID or case-insensitive name (power, brightness 1..100, color temperature 2000..6500K, hex color #rrggbb, or effect).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": "Light UUID or case-insensitive name."},
                "on": {"type": "boolean", "description": "Turn light on (true) or off (false)."},
                "brightness": {"type": "number", "minimum": 1, "maximum": 100, "description": "Brightness percentage (1..100)."},
                "kelvin": {"type": "number", "minimum": 2000, "maximum": 6500, "description": "Color temperature in Kelvin (2000..6500), clamped to the light's supported range."},
                "color_hex": {"type": "string", "description": "sRGB hex color (e.g. '#ff2e93'). Mutually exclusive with kelvin."},
                "effect": {"type": "string", "description": "Hue effect name (e.g. 'candle', 'fire', 'no_effect')."},
            },
            "required": ["target"],
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_set_room",
        "description": "Control a Philips Hue room or zone by UUID or case-insensitive name (power, brightness 1..100, color temperature 2000..6500K, or hex color #rrggbb).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": "Room or zone UUID or case-insensitive name."},
                "on": {"type": "boolean", "description": "Turn room/zone on (true) or off (false)."},
                "brightness": {"type": "number", "minimum": 1, "maximum": 100, "description": "Brightness percentage (1..100)."},
                "kelvin": {"type": "number", "minimum": 2000, "maximum": 6500, "description": "Color temperature in Kelvin (2000..6500)."},
                "color_hex": {"type": "string", "description": "sRGB hex color (e.g. '#1e88e5'). Mutually exclusive with kelvin."},
            },
            "required": ["target"],
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_recall_scene",
        "description": "Activate a Philips Hue scene by UUID or case-insensitive name (with optional room filter to disambiguate duplicate scene names).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "scene": {"type": "string", "description": "Scene UUID or case-insensitive name."},
                "room": {"type": "string", "description": "Optional room/zone UUID or case-insensitive name to disambiguate."},
            },
            "required": ["scene"],
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_apply_theme",
        "description": "Apply a built-in or custom Light Theme (e.g. 'Cyberpunk', 'Reading', 'Fireplace') to a single light or every light in a room/zone.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "theme": {"type": "string", "description": "Theme ID or case-insensitive name."},
                "target": {"type": "string", "description": "Light, room, or zone UUID or case-insensitive name."},
            },
            "required": ["theme", "target"],
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_all_on",
        "description": "Turn all Philips Hue lights on, optionally excluding specific rooms by UUID or case-insensitive name.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "exclude_rooms": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional list of room UUIDs or case-insensitive names to leave unchanged.",
                },
            },
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_all_off",
        "description": "Turn all Philips Hue lights off.",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_set_sensor",
        "description": "Enable or disable a physical Hue motion or contact sensor by UUID or case-insensitive name.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": "Motion or contact sensor UUID or case-insensitive name."},
                "enabled": {"type": "boolean", "description": "Whether the sensor should be enabled."},
            },
            "required": ["target", "enabled"],
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_set_motion_area",
        "description": "Update a Hue Bridge Pro MotionAware area by UUID or case-insensitive name (enable/disable and/or adjust sensitivity).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": {"type": "string", "description": "MotionAware area UUID or case-insensitive name."},
                "enabled": {"type": "boolean", "description": "Enable or disable this MotionAware area."},
                "sensitivity": {"type": "integer", "minimum": 0, "description": "Motion sensitivity integer (within 0..sensitivity_max)."},
            },
            "required": ["target"],
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_create_rule",
        "description": "Create and persist a dependency or sensor automation rule (validates structure and rejects contradictory cycles or chains deeper than 3).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "trigger": {"type": "string", "description": "Trigger light, room, sensor, or MotionAware area UUID or case-insensitive name."},
                "target": {"type": "string", "description": "Target light or room UUID or case-insensitive name."},
                "kind": {
                    "type": "string",
                    "enum": ["follow", "exclusive", "companion", "sensor"],
                    "description": "Rule kind (default 'sensor' for sensor/MotionAware triggers, else 'follow').",
                },
                "trigger_state": {
                    "type": "string",
                    "description": "Trigger state: 'on'|'off' for light/room, 'motion'|'clear' for motion/motion_area, 'open'|'closed' for contact, 'press'|'long_press' for button.",
                },
                "target_action": {
                    "type": "string",
                    "enum": ["on", "off"],
                    "description": "Action on target (default 'off' for exclusive rules, else 'on').",
                },
                "target_brightness": {
                    "type": "number",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Optional brightness (1..100) when turning target on.",
                },
                "mirror_off": {"type": "boolean", "description": "Also turn target off when trigger clears/turns off."},
                "match_bri": {"type": "boolean", "description": "Match trigger brightness on target."},
                "only_if_off": {"type": "boolean", "description": "Only act if target is currently off."},
                "name": {"type": "string", "description": "Optional human-readable rule name."},
                "time_start": {"type": "string", "description": "Optional active window start time in HH:MM (24h)."},
                "time_end": {"type": "string", "description": "Optional active window end time in HH:MM (24h)."},
                "enabled": {"type": "boolean", "description": "Whether the rule starts enabled (default true)."},
            },
            "required": ["trigger", "target"],
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_toggle_rule",
        "description": "Enable or disable an existing automation rule by UUID or case-insensitive name.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "rule": {"type": "string", "description": "Rule UUID or case-insensitive name."},
                "enabled": {"type": "boolean", "description": "Whether the rule should be enabled."},
            },
            "required": ["rule", "enabled"],
            "additionalProperties": False,
        },
    },
    {
        "name": "hue_delete_rule",
        "description": "Delete an existing automation rule by UUID or case-insensitive name.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "rule": {"type": "string", "description": "Rule UUID or case-insensitive name."},
            },
            "required": ["rule"],
            "additionalProperties": False,
        },
    },
]

TOOL_NAMES = frozenset(t["name"] for t in TOOLS)


# ---------------------------------------------------------------------------
# MCP Server

class HueMCPServer:
    """MCP JSON-RPC 2.0 server backed by hue_lights.Hue."""

    def __init__(self, hue=None, hue_module=None, stale_seconds=DEFAULT_STALE_SECONDS, reload_local_files=None):
        self.hue_mod = hue_module if hue_module is not None else load_hue_module()
        self._owns_hue = hue is None
        self.hue = hue if hue is not None else self.hue_mod.Hue()
        self.stale_seconds = float(stale_seconds)
        self.reload_local_files = self._owns_hue if reload_local_files is None else bool(reload_local_files)

    # -- security / redaction

    def redact(self, text):
        raw = str(text)
        if hasattr(self.hue, "redact"):
            raw = self.hue.redact(raw)
        cfg = getattr(self.hue, "config", None) or {}
        for key_name in ("app_key", "client_key", "clientkey"):
            secret = cfg.get(key_name)
            if isinstance(secret, str) and len(secret) >= 4:
                raw = raw.replace(secret, "***")
        return raw

    def _safe_error(self, exc):
        return self.hue_mod.clean(self.redact(exc), 200) or "Bridge operation failed"

    # -- state refresh helpers

    def _ensure_fresh(self, force=False):
        if self.reload_local_files:
            with self.hue.lock:
                rules_data = self.hue_mod.load_rules()
                self.hue.rules = rules_data["rules"]
                self.hue.rules_paused = rules_data["paused"]
                self.hue.custom_themes = self.hue_mod.load_themes()
                fp = self.hue_mod.load_floorplan()
                if fp is not None:
                    self.hue.floorplan = fp
                local_sched = self.hue_mod.load_local_schedules()
                self.hue.schedule_mode = local_sched["mode"]
                self.hue.local_schedules = local_sched["schedules"]
                if hasattr(self.hue, "_rebuild_schedules"):
                    self.hue._rebuild_schedules()
        with self.hue.lock:
            paired = bool(self.hue.config)
            last_refresh = float(getattr(self.hue, "last_refresh", 0.0) or 0.0)
        if not paired:
            return
        if force or last_refresh <= 0.0 or (time.time() - last_refresh) > self.stale_seconds:
            self.hue.refresh()

    def _require_paired(self, force_refresh=False):
        with self.hue.lock:
            paired = bool(self.hue.config)
        if not paired:
            raise ToolError("Hue Bridge is not paired. Pair a Hue Bridge in Maprisma Lights first.")
        self._ensure_fresh(force=force_refresh)

    def _api(self, method, path, body=None):
        try:
            return self.hue.api(method, path, body)
        except self.hue_mod.PinError:
            raise ToolError("Bridge certificate changed. Unpair and pair again if you replaced the bridge.")
        except PermissionError:
            raise ToolError("Bridge rejected the application key. Unpair and pair again.")
        except self.hue_mod.BridgeError:
            raise ToolError("Bridge reported an error for %s %s." % (method, path))
        except Exception as exc:
            raise ToolError("Bridge request failed: %s" % self._safe_error(exc))

    # -- model indexing helpers

    def _room_name_map(self):
        mapping = {}
        for room in getattr(self.hue, "rooms", []):
            rid = room.get("id")
            if rid:
                mapping[rid] = room.get("name") or ""
        return mapping

    def _all_lights_dedup(self):
        """Return deduplicated list of (light_dict, room_name, room_id), preferring rooms over zones."""
        seen = set()
        out = []
        rooms = list(getattr(self.hue, "rooms", []))
        # Pass 1: primary rooms and 'other'
        for room in rooms:
            if room.get("kind") == "zone":
                continue
            rname = room.get("name") or ""
            rid = room.get("id") or ""
            for light in room.get("lights") or []:
                lid = light.get("id")
                if lid and lid not in seen:
                    seen.add(lid)
                    out.append((light, rname, rid))
        # Pass 2: zone-only lights (if any)
        for room in rooms:
            if room.get("kind") != "zone":
                continue
            rname = room.get("name") or ""
            rid = room.get("id") or ""
            for light in room.get("lights") or []:
                lid = light.get("id")
                if lid and lid not in seen:
                    seen.add(lid)
                    out.append((light, rname, rid))
        return out

    def _format_light(self, light, room_name="", room_id=""):
        return {
            "id": light["id"],
            "name": light["name"],
            "room": room_name,
            "room_id": room_id,
            "on": bool(light.get("on", False)),
            "bri": light.get("bri", 0.0),
            "dimmable": bool(light.get("dimmable", True)),
            "color": light.get("color", "#ffd27a"),
            "mirek": light.get("mirek"),
            "mirek_min": light.get("mirek_min", 153),
            "mirek_max": light.get("mirek_max", 500),
            "supports_color": bool(light.get("supports_color", False)),
            "supports_ct": bool(light.get("supports_ct", False)),
            "effect": light.get("effect", "no_effect"),
            "available_effects": list(light.get("available_effects") or []),
            "reachable": bool(light.get("reachable", True)),
            "motion_aware": bool(light.get("motion_aware", False)),
            "motion_active": bool(light.get("motion_active", False)),
        }

    def _format_room(self, room, motion_areas_by_room=None):
        ma = None
        if motion_areas_by_room and room.get("id"):
            raw_ma = motion_areas_by_room.get(room["id"])
            if raw_ma is not None:
                ma = {
                    "id": raw_ma["id"],
                    "name": raw_ma["name"],
                    "enabled": bool(raw_ma.get("enabled", False)),
                    "motion": bool(raw_ma.get("motion", False)),
                    "sensitivity": raw_ma.get("sensitivity", 0),
                    "sensitivity_max": raw_ma.get("sensitivity_max", 4),
                    "health": raw_ma.get("health", "healthy"),
                }
        return {
            "id": room.get("id", ""),
            "kind": room.get("kind", "room"),
            "name": room.get("name", ""),
            "grouped_light": room.get("grouped_light", ""),
            "on": bool(room.get("on", False)),
            "on_count": int(room.get("on_count", 0)),
            "bri": room.get("bri", 0.0),
            "color": room.get("color", "#ffd27a"),
            "archetype": room.get("archetype", ""),
            "lights": [
                {
                    "id": l["id"],
                    "name": l["name"],
                    "on": bool(l.get("on", False)),
                    "bri": l.get("bri", 0.0),
                    "color": l.get("color", "#ffd27a"),
                    "reachable": bool(l.get("reachable", True)),
                    "motion_aware": bool(l.get("motion_aware", False)),
                    "motion_active": bool(l.get("motion_active", False)),
                }
                for l in (room.get("lights") or [])
            ],
            "scenes": [
                {"id": s["id"], "name": s["name"], "active": bool(s.get("active", False))}
                for s in (room.get("scenes") or [])
            ],
            "motion_area": ma,
        }

    def _format_sensor(self, sensor, room_names):
        rid = sensor.get("room_id", "")
        return {
            "id": sensor["id"],
            "device_id": sensor.get("device_id", ""),
            "kind": sensor["kind"],
            "name": sensor["name"],
            "room_id": rid,
            "room": room_names.get(rid, ""),
            "enabled": bool(sensor.get("enabled", True)),
            "motion": bool(sensor.get("motion", False)),
            "open": bool(sensor.get("open", False)),
            "temperature": sensor.get("temperature"),
            "lux": sensor.get("lux"),
            "battery": sensor.get("battery"),
            "battery_state": sensor.get("battery_state", ""),
            "sensitivity": sensor.get("sensitivity"),
            "sensitivity_max": sensor.get("sensitivity_max"),
            "last_event": sensor.get("last_event", ""),
        }

    def _format_motion_area(self, area, room_names):
        rid = area.get("room_id", "")
        return {
            "id": area["id"],
            "name": area["name"],
            "enabled": bool(area.get("enabled", False)),
            "room_id": rid,
            "room": room_names.get(rid, ""),
            "motion": bool(area.get("motion", False)),
            "motion_valid": bool(area.get("motion_valid", False)),
            "sensitivity": area.get("sensitivity", 0),
            "sensitivity_max": area.get("sensitivity_max", 4),
            "convenience_id": area.get("convenience_id", ""),
            "light_ids": list(area.get("light_ids") or []),
            "health": area.get("health", "healthy"),
        }

    def _bridge_summary(self, snap):
        b = snap.get("bridge") or {}
        if not b:
            return {}
        return {
            "name": b.get("name", "Hue Bridge"),
            "ip": b.get("ip", ""),
            "model_id": b.get("model_id") or b.get("model", ""),
            "is_pro": bool(b.get("is_pro", False)),
        }

    def _sanitize_state_snapshot(self):
        snap = self.hue.snapshot()
        room_names = self._room_name_map()
        ma_by_room = {}
        for area in snap.get("motion_areas") or []:
            if area.get("room_id"):
                ma_by_room.setdefault(area["room_id"], area)
        rooms = [self._format_room(r, ma_by_room) for r in (snap.get("rooms") or [])]
        lights = [self._format_light(l, rname, rid) for l, rname, rid in self._all_lights_dedup()]
        sensors = [self._format_sensor(s, room_names) for s in (snap.get("sensors") or [])]
        motion_areas = [self._format_motion_area(a, room_names) for a in (snap.get("motion_areas") or [])]
        return {
            "phase": snap.get("phase", "unpaired"),
            "paired": bool(self.hue.config),
            "error": self.redact(snap.get("error") or ""),
            "bridge": self._bridge_summary(snap),
            "light_count": int(snap.get("light_count", 0)),
            "on_count": int(snap.get("on_count", 0)),
            "room_count": sum(1 for r in (snap.get("rooms") or []) if r.get("kind") in ("room", "zone")),
            "sensor_count": len(sensors),
            "motion_area_count": len(motion_areas),
            "rooms": rooms,
            "lights": lights,
            "sensors": sensors,
            "motion_areas": motion_areas,
            "themes": list(snap.get("themes") or []),
            "rules_paused": bool(snap.get("rules_paused", False)),
            "rules": list(snap.get("rules") or []),
            "rule_log": list(snap.get("rule_log") or []),
            "schedule_mode": snap.get("schedule_mode", "bridge"),
            "schedules": list(snap.get("schedules") or []),
            "floorplan": snap.get("floorplan"),
        }

    # -- resolvers

    def _resolve_light(self, target):
        entries = []
        for light, rname, rid in self._all_lights_dedup():
            entries.append({"id": light["id"], "name": light["name"], "room": rname, "room_id": rid, "_light": light})
        match = _resolve_item(
            target,
            entries,
            "light",
            extra_fn=lambda e: ("room: %s" % e["room"]) if e.get("room") else "",
        )
        return match["_light"], match["room"], match["room_id"]

    def _resolve_room(self, target, allowed_kinds=("room", "zone")):
        candidates = [
            r for r in getattr(self.hue, "rooms", [])
            if r.get("kind") in allowed_kinds and (r.get("kind") == "other" or r.get("id"))
        ]
        return _resolve_item(
            target,
            candidates,
            "room" if allowed_kinds == ("room",) else "room/zone",
            extra_fn=lambda r: r.get("kind", ""),
        )

    def _resolve_theme(self, query):
        with self.hue.lock:
            themes = [dict(t) for t in self.hue_mod.BUILTIN_THEMES] + list(getattr(self.hue, "custom_themes", []))
        return _resolve_item(query, themes, "theme")

    def _resolve_sensor(self, target, allowed_kinds=("motion", "contact", "button")):
        with self.hue.lock:
            all_sensors = list(getattr(self.hue, "sensors", []))
        room_names = self._room_name_map()
        candidates = [s for s in all_sensors if s.get("kind") in allowed_kinds]
        return _resolve_item(
            target,
            candidates,
            "sensor",
            extra_fn=lambda s: "%s%s" % (s.get("kind", ""), (", " + room_names[s["room_id"]]) if s.get("room_id") in room_names else ""),
        )

    def _resolve_motion_area(self, target):
        with self.hue.lock:
            areas = list(getattr(self.hue, "motion_areas", []))
        return _resolve_item(target, areas, "motion area")

    def _resolve_rule(self, target):
        with self.hue.lock:
            rules = list(getattr(self.hue, "rules", []))
        return _resolve_item(target, rules, "rule")

    # -- light payload builder

    def _kelvin_to_mirek(self, kelvin, light=None):
        if not _is_number(kelvin):
            raise ToolError("kelvin must be a finite number between 2000 and 6500.")
        k = float(kelvin)
        if not (2000.0 <= k <= 6500.0):
            raise ToolError("kelvin must be between 2000 and 6500 (got %s)." % kelvin)
        raw_mirek = int(round(1_000_000.0 / k))
        m_min = int((light or {}).get("mirek_min", 153))
        m_max = int((light or {}).get("mirek_max", 500))
        return int(self.hue_mod.clamp(raw_mirek, m_min, m_max))

    def _validate_brightness(self, val, field="brightness"):
        if not _is_number(val):
            raise ToolError("%s must be a finite number between 1 and 100." % field)
        b = float(val)
        if not (1.0 <= b <= 100.0):
            raise ToolError("%s must be between 1 and 100 (got %s)." % (field, val))
        return round(b, 1)

    def _validate_color_hex(self, color_hex):
        xy = self.hue_mod.hex_to_xy(color_hex)
        if xy is None:
            raise ToolError("color_hex must be a 6-digit hex color like '#ff2e93' (got %r)." % (color_hex,))
        return xy

    def _build_theme_body(self, theme, light):
        body = {"on": {"on": True}}
        if light.get("dimmable", True):
            bri = round(self.hue_mod.clamp(float(theme["bri"]), 1.0, 100.0), 1)
            body["dimming"] = {"brightness": bri}
        if theme.get("mirek") is not None and light.get("supports_ct"):
            m_min = light.get("mirek_min", 153)
            m_max = light.get("mirek_max", 500)
            m = int(round(self.hue_mod.clamp(float(theme["mirek"]), float(m_min), float(m_max))))
            body["color_temperature"] = {"mirek": m}
        elif light.get("supports_color"):
            xy = self.hue_mod.hex_to_xy(theme.get("hex"))
            if xy is not None:
                body["color"] = {"xy": {"x": xy[0], "y": xy[1]}}
        eff = theme.get("effect") or "no_effect"
        avail = light.get("available_effects") or []
        if eff == "no_effect" or eff in avail:
            body["effects"] = {"effect": eff}
        return body

    # -- read tools

    def _tool_hue_status(self, args):
        self._ensure_fresh(force=bool(args.get("refresh", False)))
        snap = self.hue.snapshot()
        rooms = snap.get("rooms") or []
        return {
            "bridge": self._bridge_summary(snap),
            "phase": snap.get("phase", "unpaired"),
            "paired": bool(self.hue.config),
            "error": self.redact(snap.get("error") or ""),
            "light_count": int(snap.get("light_count", 0)),
            "on_count": int(snap.get("on_count", 0)),
            "room_count": sum(1 for r in rooms if r.get("kind") in ("room", "zone")),
            "sensor_count": len(snap.get("sensors") or []),
            "motion_area_count": len(snap.get("motion_areas") or []),
            "rules_paused": bool(snap.get("rules_paused", False)),
        }

    def _tool_hue_list_rooms(self, args):
        self._ensure_fresh(force=bool(args.get("refresh", False)))
        kind_filter = args.get("kind")
        if kind_filter is not None and kind_filter not in ("room", "zone", "other"):
            raise ToolError("kind must be one of 'room', 'zone', or 'other'.")
        with self.hue.lock:
            ma_by_room = {}
            for area in getattr(self.hue, "motion_areas", []):
                if area.get("room_id"):
                    ma_by_room.setdefault(area["room_id"], area)
            rooms = [
                self._format_room(r, ma_by_room)
                for r in getattr(self.hue, "rooms", [])
                if kind_filter is None or r.get("kind") == kind_filter
            ]
        return {"rooms": rooms}

    def _tool_hue_list_lights(self, args):
        self._ensure_fresh(force=bool(args.get("refresh", False)))
        only_on = bool(args.get("only_on", False))
        room_filter = args.get("room")
        with self.hue.lock:
            if room_filter is not None and str(room_filter).strip() != "":
                room = self._resolve_room(str(room_filter), allowed_kinds=("room", "zone", "other"))
                rname = room.get("name") or ""
                rid = room.get("id") or ""
                lights = [self._format_light(l, rname, rid) for l in (room.get("lights") or [])]
            else:
                lights = [self._format_light(l, rname, rid) for l, rname, rid in self._all_lights_dedup()]
        if only_on:
            lights = [l for l in lights if l["on"]]
        return {"lights": lights}

    def _tool_hue_list_sensors(self, args):
        self._ensure_fresh(force=bool(args.get("refresh", False)))
        kind_filter = args.get("kind")
        if kind_filter is not None and kind_filter not in ("motion", "contact", "button", "motion_area"):
            raise ToolError("kind must be one of 'motion', 'contact', 'button', or 'motion_area'.")
        with self.hue.lock:
            room_names = self._room_name_map()
            raw_sensors = list(getattr(self.hue, "sensors", []))
            raw_areas = list(getattr(self.hue, "motion_areas", []))
        if kind_filter == "motion_area":
            sensors = []
            motion_areas = [self._format_motion_area(a, room_names) for a in raw_areas]
        elif kind_filter in ("motion", "contact", "button"):
            sensors = [self._format_sensor(s, room_names) for s in raw_sensors if s.get("kind") == kind_filter]
            motion_areas = []
        else:
            sensors = [self._format_sensor(s, room_names) for s in raw_sensors]
            motion_areas = [self._format_motion_area(a, room_names) for a in raw_areas]
        return {"sensors": sensors, "motion_areas": motion_areas}

    def _tool_hue_list_themes(self, _args):
        if self.reload_local_files:
            with self.hue.lock:
                self.hue.custom_themes = self.hue_mod.load_themes()
        with self.hue.lock:
            themes = [dict(t) for t in self.hue_mod.BUILTIN_THEMES] + list(getattr(self.hue, "custom_themes", []))
        return {"themes": themes}

    def _tool_hue_list_rules(self, args):
        self._ensure_fresh(force=bool(args.get("refresh", False)))
        with self.hue.lock:
            return {
                "rules_paused": bool(getattr(self.hue, "rules_paused", False)),
                "rules": [dict(r) for r in getattr(self.hue, "rules", [])],
                "rule_log": [dict(e) for e in getattr(self.hue, "rule_log", [])],
            }

    def _tool_hue_list_schedules(self, args):
        self._ensure_fresh(force=bool(args.get("refresh", False)))
        with self.hue.lock:
            return {
                "schedule_mode": getattr(self.hue, "schedule_mode", "bridge"),
                "schedules": [dict(s) for s in getattr(self.hue, "schedules", [])],
            }

    def _tool_hue_get_floorplan(self, args):
        self._ensure_fresh(force=bool(args.get("refresh", False)))
        snap = self.hue.snapshot()
        return {"floorplan": snap.get("floorplan")}

    # -- control tools

    def _tool_hue_set_light(self, args):
        self._require_paired()
        target = args.get("target")
        with self.hue.lock:
            light, room_name, room_id = self._resolve_light(target)
            light_snap = dict(light)

        on = args.get("on")
        brightness = args.get("brightness")
        kelvin = args.get("kelvin")
        color_hex = args.get("color_hex")
        effect = args.get("effect")

        if on is None and brightness is None and kelvin is None and color_hex is None and effect is None:
            raise ToolError("Specify at least one of: on, brightness, kelvin, color_hex, effect.")
        if on is not None and not isinstance(on, bool):
            raise ToolError("on must be a boolean.")
        if kelvin is not None and color_hex is not None:
            raise ToolError("Specify either kelvin or color_hex, not both.")

        body = {}
        if on is not None:
            body["on"] = {"on": on}
        if brightness is not None:
            bri = self._validate_brightness(brightness)
            if not light_snap.get("dimmable", True):
                raise ToolError("Light '%s' does not support dimming." % light_snap["name"])
            body["dimming"] = {"brightness": bri}
            if on is None:
                body["on"] = {"on": True}
        if kelvin is not None:
            if not light_snap.get("supports_ct", False):
                raise ToolError("Light '%s' does not support color temperature." % light_snap["name"])
            mirek = self._kelvin_to_mirek(kelvin, light_snap)
            body["color_temperature"] = {"mirek": mirek}
            if on is None:
                body["on"] = {"on": True}
        if color_hex is not None:
            if not light_snap.get("supports_color", False):
                raise ToolError("Light '%s' does not support RGB color." % light_snap["name"])
            xy = self._validate_color_hex(color_hex)
            body["color"] = {"xy": {"x": xy[0], "y": xy[1]}}
            if on is None:
                body["on"] = {"on": True}
        if effect is not None:
            if not isinstance(effect, str) or self.hue_mod.EFFECT_RE.match(effect) is None:
                raise ToolError("Invalid effect name %r." % (effect,))
            avail = light_snap.get("available_effects") or []
            if effect != "no_effect" and avail and effect not in avail:
                raise ToolError(
                    "Effect '%s' is not supported by '%s'. Available effects: %s."
                    % (effect, light_snap["name"], ", ".join(avail))
                )
            body["effects"] = {"effect": effect}
            if effect != "no_effect" and on is None:
                body["on"] = {"on": True}

        lid = light_snap["id"]
        self._api("PUT", "/clip/v2/resource/light/%s" % lid, body)
        with self.hue.lock:
            self.hue.apply_optimistic("light", lid, body)
            updated = self.hue._find_light(lid) or light_snap
            formatted = self._format_light(updated, room_name, room_id)
        return {"ok": True, "light": formatted, "sent": body}

    def _tool_hue_set_room(self, args):
        self._require_paired()
        target = args.get("target")
        with self.hue.lock:
            room = self._resolve_room(target, allowed_kinds=("room", "zone"))
            room_id = room["id"]
            room_kind = room["kind"]
            room_name = room["name"]
            gl_id = room.get("grouped_light", "")
            member_lights = [dict(l) for l in (room.get("lights") or [])]

        on = args.get("on")
        brightness = args.get("brightness")
        kelvin = args.get("kelvin")
        color_hex = args.get("color_hex")

        if on is None and brightness is None and kelvin is None and color_hex is None:
            raise ToolError("Specify at least one of: on, brightness, kelvin, color_hex.")
        if on is not None and not isinstance(on, bool):
            raise ToolError("on must be a boolean.")
        if kelvin is not None and color_hex is not None:
            raise ToolError("Specify either kelvin or color_hex, not both.")

        bri = self._validate_brightness(brightness) if brightness is not None else None
        xy = self._validate_color_hex(color_hex) if color_hex is not None else None
        if kelvin is not None:
            self._kelvin_to_mirek(kelvin)  # range check up front

        needs_per_light = (kelvin is not None) or (xy is not None) or (not self.hue_mod.valid_uuid(gl_id))
        calls = 0
        if not needs_per_light:
            body = {}
            if on is not None:
                body["on"] = {"on": on}
            if bri is not None:
                body["dimming"] = {"brightness": bri}
                if on is None:
                    body["on"] = {"on": True}
            self._api("PUT", "/clip/v2/resource/grouped_light/%s" % gl_id, body)
            with self.hue.lock:
                self.hue.apply_optimistic("grouped_light", gl_id, body)
            calls = 1
        else:
            if not member_lights:
                raise ToolError("Room '%s' has no lights." % room_name)
            for l in member_lights:
                lbody = {}
                if on is not None:
                    lbody["on"] = {"on": on}
                if bri is not None and l.get("dimmable", True):
                    lbody["dimming"] = {"brightness": bri}
                    if on is None:
                        lbody["on"] = {"on": True}
                if kelvin is not None and l.get("supports_ct", False):
                    lbody["color_temperature"] = {"mirek": self._kelvin_to_mirek(kelvin, l)}
                    if on is None:
                        lbody["on"] = {"on": True}
                if xy is not None and l.get("supports_color", False):
                    lbody["color"] = {"xy": {"x": xy[0], "y": xy[1]}}
                    if on is None:
                        lbody["on"] = {"on": True}
                if not lbody:
                    continue
                self._api("PUT", "/clip/v2/resource/light/%s" % l["id"], lbody)
                with self.hue.lock:
                    self.hue.apply_optimistic("light", l["id"], lbody)
                calls += 1
            if calls == 0:
                raise ToolError("No lights in '%s' support the requested parameters." % room_name)

        with self.hue.lock:
            ma_by_room = {a["room_id"]: a for a in getattr(self.hue, "motion_areas", []) if a.get("room_id")}
            updated_room = next(
                (r for r in self.hue.rooms if r.get("id") == room_id and r.get("kind") == room_kind),
                room,
            )
            formatted = self._format_room(updated_room, ma_by_room)
        return {"ok": True, "room": formatted, "calls": calls}

    def _tool_hue_recall_scene(self, args):
        self._require_paired()
        scene_query = args.get("scene")
        room_query = args.get("room")
        with self.hue.lock:
            if room_query is not None and str(room_query).strip() != "":
                target_rooms = [self._resolve_room(str(room_query), allowed_kinds=("room", "zone"))]
            else:
                target_rooms = [r for r in getattr(self.hue, "rooms", []) if r.get("kind") in ("room", "zone")]
            scene_entries = []
            for r in target_rooms:
                for s in r.get("scenes") or []:
                    scene_entries.append({
                        "id": s["id"],
                        "name": s["name"],
                        "room": r["name"],
                        "room_id": r["id"],
                    })
            match = _resolve_item(
                scene_query,
                scene_entries,
                "scene",
                extra_fn=lambda e: "room: %s" % e["room"],
            )

        sid = match["id"]
        self._api("PUT", "/clip/v2/resource/scene/%s" % sid, {"recall": {"action": "active"}})
        with self.hue.lock:
            for r in self.hue.rooms:
                if r.get("id") == match["room_id"]:
                    for s in r.get("scenes") or []:
                        s["active"] = (s["id"] == sid)
            self.hue.last_refresh = 0.0
        return {
            "ok": True,
            "scene": {
                "id": sid,
                "name": match["name"],
                "room": match["room"],
                "room_id": match["room_id"],
            },
        }

    def _tool_hue_apply_theme(self, args):
        self._require_paired()
        theme = self._resolve_theme(args.get("theme"))
        target_q = args.get("target")
        if not isinstance(target_q, str) or not target_q.strip():
            raise ToolError("Invalid target: must be a non-empty light or room ID/name.")
        q = target_q.strip()
        q_low = q.lower()

        with self.hue.lock:
            all_lights = self._all_lights_dedup()
            all_rooms = [r for r in getattr(self.hue, "rooms", []) if r.get("kind") in ("room", "zone") and r.get("id")]

            # 1. Exact ID match
            light_by_id = next(((l, rname, rid) for l, rname, rid in all_lights if l["id"].lower() == q_low), None)
            room_by_id = next((r for r in all_rooms if r["id"].lower() == q_low), None)

            target_kind = None
            target_id = ""
            target_name = ""
            lights_to_update = []

            if light_by_id is not None:
                l, _, _ = light_by_id
                target_kind, target_id, target_name = "light", l["id"], l["name"]
                lights_to_update = [dict(l)]
            elif room_by_id is not None:
                target_kind, target_id, target_name = room_by_id["kind"], room_by_id["id"], room_by_id["name"]
                lights_to_update = [dict(l) for l in (room_by_id.get("lights") or [])]
            else:
                # 2. Exact case-insensitive name match across lights and rooms/zones
                combined = []
                for l, rname, _ in all_lights:
                    combined.append({"id": l["id"], "name": l["name"], "_kind": "light", "_room": rname, "_obj": l})
                for r in all_rooms:
                    combined.append({"id": r["id"], "name": r["name"], "_kind": r["kind"], "_room": "", "_obj": r})
                picked = _resolve_item(
                    q,
                    combined,
                    "light or room",
                    extra_fn=lambda e: (
                        ("light in %s" % e["_room"]) if (e["_kind"] == "light" and e["_room"]) else e["_kind"]
                    ),
                )
                if picked["_kind"] == "light":
                    l = picked["_obj"]
                    target_kind, target_id, target_name = "light", l["id"], l["name"]
                    lights_to_update = [dict(l)]
                else:
                    r = picked["_obj"]
                    target_kind, target_id, target_name = r["kind"], r["id"], r["name"]
                    lights_to_update = [dict(l) for l in (r.get("lights") or [])]

        if not lights_to_update:
            raise ToolError("Target '%s' has no lights." % target_name)

        updated_ids = []
        for l in lights_to_update:
            body = self._build_theme_body(theme, l)
            self._api("PUT", "/clip/v2/resource/light/%s" % l["id"], body)
            with self.hue.lock:
                self.hue.apply_optimistic("light", l["id"], body)
            updated_ids.append({"id": l["id"], "name": l["name"]})

        return {
            "ok": True,
            "theme": {"id": theme["id"], "name": theme["name"]},
            "target_kind": target_kind,
            "target_id": target_id,
            "target_name": target_name,
            "lights_updated": updated_ids,
        }

    def _tool_hue_all_on(self, args):
        self._require_paired()
        raw_exclude = args.get("exclude_rooms")
        if raw_exclude is None:
            raw_exclude = []
        if not isinstance(raw_exclude, list):
            raise ToolError("exclude_rooms must be a list of room IDs or names.")

        excluded_room_ids = set()
        excluded_light_ids = set()
        excluded_names = []
        with self.hue.lock:
            for item in raw_exclude:
                room = self._resolve_room(item, allowed_kinds=("room", "zone"))
                excluded_room_ids.add(room["id"])
                excluded_names.append(room["name"])
                for l in room.get("lights") or []:
                    excluded_light_ids.add(l["id"])
            home_gl = getattr(self.hue, "bridge_home_gl", "")
            rooms_copy = [
                {
                    "id": r.get("id", ""),
                    "kind": r.get("kind", ""),
                    "grouped_light": r.get("grouped_light", ""),
                    "light_ids": [l["id"] for l in (r.get("lights") or [])],
                }
                for r in getattr(self.hue, "rooms", [])
            ]

        calls = 0
        body = {"on": {"on": True}}
        if not excluded_room_ids and self.hue_mod.valid_uuid(home_gl):
            self._api("PUT", "/clip/v2/resource/grouped_light/%s" % home_gl, body)
            calls += 1
            with self.hue.lock:
                for room in self.hue.rooms:
                    for light in room["lights"]:
                        light["on"] = True
                    lit = [l for l in room["lights"] if l["on"]]
                    room["on_count"] = len(lit)
                    room["on"] = bool(lit)
                    room["color"] = self.hue_mod.blend_colors([l["color"] for l in lit])
                self.hue.on_count = sum(
                    1 for r in self.hue.rooms if r["kind"] != "zone" for l in r["lights"] if l["on"]
                )
        else:
            for r in rooms_copy:
                if r["kind"] == "room":
                    if r["id"] in excluded_room_ids:
                        continue
                    has_excluded_light = any(lid in excluded_light_ids for lid in r["light_ids"])
                    if not has_excluded_light and self.hue_mod.valid_uuid(r["grouped_light"]):
                        self._api("PUT", "/clip/v2/resource/grouped_light/%s" % r["grouped_light"], body)
                        with self.hue.lock:
                            self.hue.apply_optimistic("grouped_light", r["grouped_light"], body)
                        calls += 1
                    else:
                        for lid in r["light_ids"]:
                            if lid in excluded_light_ids:
                                continue
                            self._api("PUT", "/clip/v2/resource/light/%s" % lid, body)
                            with self.hue.lock:
                                self.hue.apply_optimistic("light", lid, body)
                            calls += 1
                elif r["kind"] == "other":
                    for lid in r["light_ids"]:
                        if lid in excluded_light_ids:
                            continue
                        self._api("PUT", "/clip/v2/resource/light/%s" % lid, body)
                        with self.hue.lock:
                            self.hue.apply_optimistic("light", lid, body)
                        calls += 1

        with self.hue.lock:
            on_count = int(self.hue.on_count)
        return {"ok": True, "excluded_rooms": excluded_names, "calls": calls, "on_count": on_count}

    def _tool_hue_all_off(self, _args):
        self._require_paired()
        with self.hue.lock:
            items = []
            for room in getattr(self.hue, "rooms", []):
                if room.get("kind") == "room" and room.get("on"):
                    if self.hue_mod.valid_uuid(room.get("grouped_light")):
                        items.append(("grouped_light", room["grouped_light"]))
                    else:
                        items.extend(("light", l["id"]) for l in (room.get("lights") or []) if l.get("on"))
                elif room.get("kind") == "other":
                    items.extend(("light", l["id"]) for l in (room.get("lights") or []) if l.get("on"))

        body = {"on": {"on": False}}
        calls = 0
        for kind, rid in items:
            self._api("PUT", "/clip/v2/resource/%s/%s" % (kind, rid), body)
            with self.hue.lock:
                self.hue.apply_optimistic(kind, rid, body)
            calls += 1

        with self.hue.lock:
            on_count = int(self.hue.on_count)
        return {"ok": True, "calls": calls, "on_count": on_count}

    def _tool_hue_set_sensor(self, args):
        self._require_paired()
        target = args.get("target")
        enabled = args.get("enabled")
        if not isinstance(enabled, bool):
            raise ToolError("enabled must be a boolean.")
        sensor = self._resolve_sensor(target, allowed_kinds=("motion", "contact", "button"))
        if sensor["kind"] not in ("motion", "contact"):
            raise ToolError(
                "Sensor '%s' is a button/switch and cannot be enabled or disabled." % sensor["name"]
            )
        self._api("PUT", "/clip/v2/resource/%s/%s" % (sensor["kind"], sensor["id"]), {"enabled": enabled})
        with self.hue.lock:
            for s in getattr(self.hue, "sensors", []):
                if s.get("kind") == sensor["kind"] and s.get("id") == sensor["id"]:
                    s["enabled"] = enabled
                    updated = s
                    break
            else:
                updated = dict(sensor, enabled=enabled)
            room_names = self._room_name_map()
            formatted = self._format_sensor(updated, room_names)
        return {"ok": True, "sensor": formatted}

    def _tool_hue_set_motion_area(self, args):
        self._require_paired()
        target = args.get("target")
        has_enabled = "enabled" in args and args["enabled"] is not None
        has_sens = "sensitivity" in args and args["sensitivity"] is not None
        if not has_enabled and not has_sens:
            raise ToolError("Specify at least one of: enabled, sensitivity.")
        if has_enabled and not isinstance(args["enabled"], bool):
            raise ToolError("enabled must be a boolean.")
        if has_sens and not _is_int(args["sensitivity"]):
            raise ToolError("sensitivity must be an integer.")

        area = self._resolve_motion_area(target)
        area_id = area["id"]
        smax = int(area.get("sensitivity_max", 4))
        conv_id = area.get("convenience_id", "")

        if has_sens:
            if not (0 <= args["sensitivity"] <= smax):
                raise ToolError(
                    "sensitivity for '%s' must be between 0 and %d (got %d)."
                    % (area["name"], smax, args["sensitivity"])
                )
            if not self.hue_mod.valid_uuid(conv_id):
                raise ToolError(
                    "Motion area '%s' has no linked convenience_area_motion resource for sensitivity."
                    % area["name"]
                )

        if has_enabled:
            self._api(
                "PUT",
                "/clip/v2/resource/motion_area_configuration/%s" % area_id,
                {"enabled": args["enabled"]},
            )
        if has_sens:
            self._api(
                "PUT",
                "/clip/v2/resource/convenience_area_motion/%s" % conv_id,
                {"sensitivity": {"sensitivity": args["sensitivity"]}},
            )

        with self.hue.lock:
            for a in getattr(self.hue, "motion_areas", []):
                if a.get("id") == area_id:
                    if has_enabled:
                        a["enabled"] = args["enabled"]
                    if has_sens:
                        a["sensitivity"] = args["sensitivity"]
                    updated = a
                    break
            else:
                updated = dict(area)
                if has_enabled:
                    updated["enabled"] = args["enabled"]
                if has_sens:
                    updated["sensitivity"] = args["sensitivity"]
            if has_enabled:
                self.hue_mod.annotate_motion_aware_lights(self.hue.rooms, self.hue.motion_areas)
            room_names = self._room_name_map()
            formatted = self._format_motion_area(updated, room_names)
        return {"ok": True, "motion_area": formatted}

    def _resolve_rule_trigger(self, query):
        """Resolve a rule trigger across lights, rooms, physical sensors, and MotionAware areas."""
        if not isinstance(query, str) or not query.strip():
            raise ToolError("trigger must be a non-empty UUID or name.")
        entries = []
        with self.hue.lock:
            for l, rname, _ in self._all_lights_dedup():
                entries.append({"id": l["id"], "name": l["name"], "_kind": "light", "_extra": "light in %s" % rname if rname else "light"})
            for r in getattr(self.hue, "rooms", []):
                if r.get("kind") == "room" and r.get("id"):
                    entries.append({"id": r["id"], "name": r["name"], "_kind": "room", "_extra": "room"})
            for s in getattr(self.hue, "sensors", []):
                if s.get("kind") in ("motion", "contact", "button") and s.get("id"):
                    entries.append({"id": s["id"], "name": s["name"], "_kind": s["kind"], "_extra": "%s sensor" % s["kind"]})
            for a in getattr(self.hue, "motion_areas", []):
                if a.get("id"):
                    entries.append({"id": a["id"], "name": a["name"], "_kind": "motion_area", "_extra": "motion area"})
        match = _resolve_item(query, entries, "rule trigger", extra_fn=lambda e: e["_extra"])
        return match["_kind"], match["id"], match["name"]

    def _resolve_rule_target(self, query):
        """Resolve a rule target across lights and rooms."""
        if not isinstance(query, str) or not query.strip():
            raise ToolError("target must be a non-empty UUID or name.")
        entries = []
        with self.hue.lock:
            for l, rname, _ in self._all_lights_dedup():
                entries.append({"id": l["id"], "name": l["name"], "_kind": "light", "_extra": "light in %s" % rname if rname else "light"})
            for r in getattr(self.hue, "rooms", []):
                if r.get("kind") == "room" and r.get("id"):
                    entries.append({"id": r["id"], "name": r["name"], "_kind": "room", "_extra": "room"})
        match = _resolve_item(query, entries, "rule target", extra_fn=lambda e: e["_extra"])
        return match["_kind"], match["id"], match["name"]

    def _tool_hue_create_rule(self, args):
        self._ensure_fresh()
        trig_kind, trig_id, trig_name = self._resolve_rule_trigger(args.get("trigger"))
        targ_kind, targ_id, targ_name = self._resolve_rule_target(args.get("target"))

        is_sensor_trigger = trig_kind in ("motion", "motion_area", "contact", "button")
        kind = args.get("kind")
        if kind is None or kind == "":
            kind = "sensor" if is_sensor_trigger else "follow"
        if kind not in ("follow", "exclusive", "companion", "sensor"):
            raise ToolError("kind must be one of: follow, exclusive, companion, sensor.")

        default_trigger_state = {
            "light": "on",
            "room": "on",
            "motion": "motion",
            "motion_area": "motion",
            "contact": "open",
            "button": "press",
        }[trig_kind]
        trigger_state = args.get("trigger_state") or default_trigger_state

        target_action = args.get("target_action")
        if target_action is None or target_action == "":
            target_action = "off" if kind == "exclusive" else "on"
        if target_action not in ("on", "off"):
            raise ToolError("target_action must be 'on' or 'off'.")

        target_bri = args.get("target_brightness")
        if target_bri is not None:
            target_bri = int(round(self._validate_brightness(target_bri, field="target_brightness")))

        mirror_off = args.get("mirror_off", False)
        match_bri = args.get("match_bri", False)
        only_if_off = args.get("only_if_off", False)
        enabled = args.get("enabled", True)
        if not all(isinstance(v, bool) for v in (mirror_off, match_bri, only_if_off, enabled)):
            raise ToolError("mirror_off, match_bri, only_if_off, and enabled must be booleans.")

        time_start = args.get("time_start") or ""
        time_end = args.get("time_end") or ""
        name = args.get("name") or ("%s -> %s" % (trig_name, targ_name))

        raw_rule = {
            "name": name,
            "enabled": enabled,
            "kind": kind,
            "trigger_kind": trig_kind,
            "trigger_id": trig_id,
            "trigger_state": trigger_state,
            "targets": [
                {
                    "kind": targ_kind,
                    "id": targ_id,
                    "action": target_action,
                    "bri": target_bri,
                    "scene_id": "",
                }
            ],
            "mirror_off": mirror_off,
            "match_bri": match_bri,
            "only_if_off": only_if_off,
            "time_start": time_start,
            "time_end": time_end,
        }
        rule = self.hue_mod.validate_rule(raw_rule)
        if rule is None:
            raise ToolError(
                "Invalid rule parameters (check trigger_state for %s, time_start/time_end HH:MM, and that trigger != target)."
                % trig_kind
            )

        with self.hue.lock:
            if len(self.hue.rules) >= self.hue_mod.MAX_RULES:
                raise ToolError("Maximum number of rules (%d) reached." % self.hue_mod.MAX_RULES)
            candidate = list(self.hue.rules) + [rule]
            cycle_err = self.hue_mod.detect_rule_cycles(candidate)
            if cycle_err:
                raise ToolError("Rule rejected: %s." % cycle_err)
            try:
                self.hue_mod.save_rules({"paused": self.hue.rules_paused, "rules": candidate})
            except (OSError, ValueError) as exc:
                raise ToolError("Failed to save rules: %s" % self._safe_error(exc))
            self.hue.rules = candidate
        return {"ok": True, "rule": rule}

    def _tool_hue_toggle_rule(self, args):
        self._ensure_fresh()
        enabled = args.get("enabled")
        if not isinstance(enabled, bool):
            raise ToolError("enabled must be a boolean.")
        target = self._resolve_rule(args.get("rule"))
        rid = target["id"]
        with self.hue.lock:
            candidate = [dict(r, enabled=enabled) if r["id"] == rid else r for r in self.hue.rules]
            if enabled:
                cycle_err = self.hue_mod.detect_rule_cycles(candidate)
                if cycle_err:
                    raise ToolError("Cannot enable rule: %s." % cycle_err)
            try:
                self.hue_mod.save_rules({"paused": self.hue.rules_paused, "rules": candidate})
            except (OSError, ValueError) as exc:
                raise ToolError("Failed to save rules: %s" % self._safe_error(exc))
            self.hue.rules = candidate
            updated = next(r for r in self.hue.rules if r["id"] == rid)
        return {"ok": True, "rule": dict(updated)}

    def _tool_hue_delete_rule(self, args):
        self._ensure_fresh()
        target = self._resolve_rule(args.get("rule"))
        rid = target["id"]
        with self.hue.lock:
            new_rules = [r for r in self.hue.rules if r["id"] != rid]
            try:
                self.hue_mod.save_rules({"paused": self.hue.rules_paused, "rules": new_rules})
            except (OSError, ValueError) as exc:
                raise ToolError("Failed to save rules: %s" % self._safe_error(exc))
            self.hue.rules = new_rules
            if hasattr(self.hue, "rule_last_fired"):
                self.hue.rule_last_fired.pop(rid, None)
        return {"ok": True, "deleted": {"id": rid, "name": target["name"]}}

    # -- MCP method dispatch

    def call_tool(self, name, arguments=None):
        if name not in TOOL_NAMES:
            raise RpcError(-32602, "Unknown tool: %s" % name)
        if arguments is None:
            arguments = {}
        if not isinstance(arguments, dict):
            raise RpcError(-32602, "Tool arguments must be a JSON object.")

        handler = getattr(self, "_tool_" + name, None)
        if handler is None:
            raise RpcError(-32602, "Unknown tool: %s" % name)

        try:
            payload = handler(arguments)
            text = json.dumps(payload, allow_nan=False)
            text = self.redact(text)
            return {
                "content": [{"type": "text", "text": text}],
                "isError": False,
            }
        except ToolError as exc:
            err_text = json.dumps({"error": self.redact(str(exc))}, allow_nan=False)
            return {
                "content": [{"type": "text", "text": err_text}],
                "isError": True,
            }
        except Exception as exc:
            err_text = json.dumps({"error": self._safe_error(exc)}, allow_nan=False)
            return {
                "content": [{"type": "text", "text": err_text}],
                "isError": True,
            }

    def handle_message(self, msg):
        """Process one parsed JSON-RPC 2.0 message dict. Returns response dict or None for notifications."""
        if not isinstance(msg, dict):
            return self._error_response(None, -32600, "Invalid Request: expected JSON object.")

        has_id = "id" in msg
        req_id = msg.get("id")
        if has_id and req_id is not None and (isinstance(req_id, bool) or not isinstance(req_id, (str, int))):
            return self._error_response(None, -32600, "Invalid Request: id must be string, integer, or null.")

        method = msg.get("method")
        if not isinstance(method, str) or not method:
            return self._error_response(req_id if has_id else None, -32600, "Invalid Request: missing method.")

        params = msg.get("params")
        if params is not None and not isinstance(params, dict):
            if not has_id:
                return None
            return self._error_response(req_id, -32602, "Invalid params: expected object.")
        params = params or {}

        # Notifications (no id) never get a JSON-RPC response
        if not has_id:
            return None

        try:
            if method == "initialize":
                client_ver = params.get("protocolVersion")
                proto = client_ver if client_ver in SUPPORTED_PROTOCOL_VERSIONS else DEFAULT_PROTOCOL_VERSION
                result = {
                    "protocolVersion": proto,
                    "capabilities": {
                        "tools": {},
                        "resources": {},
                    },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION,
                    },
                }
            elif method == "ping":
                result = {}
            elif method == "tools/list":
                result = {"tools": TOOLS}
            elif method == "tools/call":
                tname = params.get("name")
                if not isinstance(tname, str) or not tname:
                    raise RpcError(-32602, "Invalid params: missing tool name.")
                result = self.call_tool(tname, params.get("arguments"))
            elif method == "resources/list":
                result = {
                    "resources": [
                        {
                            "uri": STATE_RESOURCE_URI,
                            "name": "Philips Hue State",
                            "description": "Live Philips Hue state snapshot (rooms, lights, sensors, MotionAware areas, rules, schedules — no credentials)",
                            "mimeType": "application/json",
                        }
                    ]
                }
            elif method == "resources/read":
                uri = params.get("uri")
                if not isinstance(uri, str) or not uri:
                    raise RpcError(-32602, "Invalid params: missing resource uri.")
                if uri != STATE_RESOURCE_URI:
                    raise RpcError(-32002, "Resource not found: %s" % uri)
                self._ensure_fresh()
                state_json = self.redact(json.dumps(self._sanitize_state_snapshot(), allow_nan=False))
                result = {
                    "contents": [
                        {
                            "uri": STATE_RESOURCE_URI,
                            "mimeType": "application/json",
                            "text": state_json,
                        }
                    ]
                }
            else:
                raise RpcError(-32601, "Method not found: %s" % method)

            return {"jsonrpc": "2.0", "id": req_id, "result": result}
        except RpcError as exc:
            return self._error_response(req_id, exc.code, self.redact(exc.message), exc.data)
        except Exception as exc:
            return self._error_response(req_id, -32603, "Internal error: %s" % self._safe_error(exc))

    def _error_response(self, req_id, code, message, data=None):
        err = {"code": int(code), "message": str(message)}
        if data is not None:
            err["data"] = data
        return {"jsonrpc": "2.0", "id": req_id, "error": err}

    # -- stdio transport

    def serve_stdio(self, stdin=None, stdout=None):
        """Read newline-delimited JSON-RPC 2.0 requests from stdin and write responses to stdout."""
        in_stream = stdin if stdin is not None else sys.stdin.buffer
        out_stream = stdout if stdout is not None else sys.stdout.buffer

        def write_response(resp):
            line = json.dumps(resp, separators=(",", ":"), allow_nan=False).encode("utf-8") + b"\n"
            out_stream.write(line)
            out_stream.flush()

        while True:
            raw_line = in_stream.readline(MAX_LINE_BYTES + 1)
            if not raw_line:
                return
            if len(raw_line) > MAX_LINE_BYTES:
                while not raw_line.endswith(b"\n"):
                    raw_line = in_stream.readline(MAX_LINE_BYTES + 1)
                    if not raw_line:
                        break
                write_response(self._error_response(None, -32600, "Request line exceeds maximum size."))
                continue
            stripped = raw_line.strip()
            if not stripped:
                continue
            try:
                text = stripped.decode("utf-8")
            except UnicodeDecodeError:
                write_response(self._error_response(None, -32700, "Parse error: invalid UTF-8."))
                continue
            if not self.hue_mod._json_depth_ok(text):
                write_response(self._error_response(None, -32700, "Parse error: JSON nesting too deep."))
                continue
            try:
                msg = json.loads(text)
            except (ValueError, TypeError, RecursionError):
                write_response(self._error_response(None, -32700, "Parse error: malformed JSON."))
                continue
            resp = self.handle_message(msg)
            if resp is not None:
                write_response(resp)


def _terminate(signum, _frame):
    sys.exit(128 + signum)


def main():
    signal.signal(signal.SIGTERM, _terminate)
    server = HueMCPServer()
    try:
        server.serve_stdio()
    except (KeyboardInterrupt, BrokenPipeError):
        pass


if __name__ == "__main__":
    main()
