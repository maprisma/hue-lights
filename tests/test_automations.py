"""Unit tests for richer light control, all_on, SSE eventstream, schedules, dependency rules, themes, and audio synth."""
import datetime
import importlib.util
import json
import math
import os
import shutil
import socket
import ssl
import struct
import subprocess
import tempfile
import threading
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("hue_auto", os.path.join(HERE, "..", "scripts", "hue-lights.py"))
hue = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hue)

BRIDGE_ID = "001788FFFE5A3C21"
PIN = {
    "ip": "192.168.1.50", "bridge_id": BRIDGE_ID, "fingerprint": "a" * 64,
    "app_key": "abcDEF-123_x", "name": "Hue", "model": "BSB003",
}

L1 = "11111111-1111-1111-1111-111111111111"
L2 = "22222222-2222-2222-2222-222222222222"
L3 = "33333333-3333-3333-3333-333333333333"
L4 = "44444444-4444-4444-4444-444444444444"
L5 = "55555555-5555-5555-5555-555555555555"
GL1 = "bbbbbbbb-1111-1111-1111-111111111111"
GL2 = "bbbbbbbb-2222-2222-2222-222222222222"
HOME_GL = "bbbbbbbb-0000-0000-0000-000000000000"
HOME_ID = "eeeeeeee-0000-0000-0000-000000000000"
R1 = "cccccccc-1111-1111-1111-111111111111"
R2 = "cccccccc-2222-2222-2222-222222222222"
Z1 = "cccccccc-9999-9999-9999-999999999999"
SCENE1 = "dddddddd-1111-1111-1111-111111111111"
SCHED1 = "ffffffff-1111-1111-1111-111111111111"
SCHED2 = "ffffffff-2222-2222-2222-222222222222"
SCRIPT_CUSTOM = "99999999-aaaa-bbbb-cccc-111111111111"


def sample_resources(include_home=True):
    res = [
        {
            "id": L1, "type": "light", "metadata": {"name": "Desk"},
            "on": {"on": False}, "dimming": {"brightness": 50.0},
            "color_temperature": {
                "mirek": 320, "mirek_valid": True,
                "mirek_schema": {"mirek_minimum": 153, "mirek_maximum": 454},
            },
            "color": {"xy": {"x": 0.4, "y": 0.4}},
            "effects": {
                "status": "candle",
                "effect_values": ["no_effect", "candle", "fire", "<script>", "BAD CAPS", "candle"],
            },
        },
        {
            "id": L2, "type": "light", "metadata": {"name": "Floor"},
            "on": {"on": False}, "dimming": {"brightness": 30.0},
            "color_temperature": {"mirek": None, "mirek_valid": False},
        },
        {
            "id": L3, "type": "light", "metadata": {"name": "Bedside"},
            "on": {"on": False}, "dimming": {"brightness": 40.0},
        },
        {
            "id": L4, "type": "light", "metadata": {"name": "Loose Lamp"},
            "on": {"on": False}, "dimming": {"brightness": 100.0},
        },
        {"id": GL1, "type": "grouped_light", "on": {"on": False}, "dimming": {"brightness": 50.0}},
        {"id": GL2, "type": "grouped_light", "on": {"on": False}, "dimming": {"brightness": 40.0}},
        {
            "id": R1, "type": "room", "metadata": {"name": "Office"},
            "children": [{"rid": L1, "rtype": "light"}, {"rid": L2, "rtype": "light"}],
            "services": [{"rid": GL1, "rtype": "grouped_light"}],
        },
        {
            "id": R2, "type": "room", "metadata": {"name": "Bedroom"},
            "children": [{"rid": L3, "rtype": "light"}],
            "services": [{"rid": GL2, "rtype": "grouped_light"}],
        },
        {
            "id": Z1, "type": "zone", "metadata": {"name": "Upstairs"},
            "children": [{"rid": L1, "rtype": "light"}, {"rid": L3, "rtype": "light"}],
        },
        {
            "id": SCENE1, "type": "scene", "metadata": {"name": "Focus"},
            "group": {"rid": R1, "rtype": "room"}, "status": {"active": "inactive"},
        },
    ]
    if include_home:
        res.append({"id": HOME_GL, "type": "grouped_light", "on": {"on": False}})
        res.append({
            "id": HOME_ID, "type": "bridge_home",
            "services": [{"rid": HOME_GL, "rtype": "grouped_light"}],
        })
    return res


class Quiet(hue.Hue):
    def __init__(self, config=None):
        self.emitted = []
        hue.load_config = lambda: dict(config or {})
        super().__init__()

    def emit(self):
        self.emitted.append(self.snapshot())

    def discover_now(self):
        return []


class Base(unittest.TestCase):
    def setUp(self):
        self.real = (hue.https_request, hue.load_config, hue.save_config, hue.remove_config,
                     hue.CONFIG_DIR, hue.CONFIG_PATH)
        self.dir = tempfile.mkdtemp()
        hue.CONFIG_DIR = os.path.join(self.dir, "hue-lights")
        hue.CONFIG_PATH = os.path.join(hue.CONFIG_DIR, "config.json")

    def tearDown(self):
        (hue.https_request, hue.load_config, hue.save_config, hue.remove_config,
         hue.CONFIG_DIR, hue.CONFIG_PATH) = self.real
        shutil.rmtree(self.dir)

    def make_hue(self, resources=None):
        h = Quiet(PIN)
        res = sample_resources() if resources is None else resources
        hue.https_request = lambda *a, **k: (200, {"data": res}, None)
        h.refresh()
        h.emitted.clear()
        return h


class RichLightControlTests(Base):
    def test_build_model_rich_fields(self):
        rooms, total, on = hue.build_model(sample_resources())
        self.assertEqual((total, on), (4, 0))
        office = next(r for r in rooms if r["id"] == R1)
        desk = next(l for l in office["lights"] if l["id"] == L1)
        floor = next(l for l in office["lights"] if l["id"] == L2)
        bedroom = next(r for r in rooms if r["id"] == R2)
        bedside = bedroom["lights"][0]

        self.assertTrue(desk["supports_color"])
        self.assertTrue(desk["supports_ct"])
        self.assertEqual(desk["mirek"], 320)
        self.assertEqual(desk["mirek_min"], 153)
        self.assertEqual(desk["mirek_max"], 454)
        self.assertEqual(desk["effect"], "candle")
        self.assertEqual(desk["available_effects"], ["no_effect", "candle", "fire"])

        self.assertFalse(floor["supports_color"])
        self.assertTrue(floor["supports_ct"])
        self.assertIsNone(floor["mirek"])
        self.assertEqual((floor["mirek_min"], floor["mirek_max"]), (153, 500))
        self.assertEqual(floor["effect"], "no_effect")
        self.assertEqual(floor["available_effects"], [])

        self.assertFalse(bedside["supports_color"])
        self.assertFalse(bedside["supports_ct"])

    def test_mirek_xy_effect_commands_and_optimistic(self):
        h = self.make_hue()
        h.handle({"cmd": "light", "id": L1, "mirek": 250})
        self.assertEqual(h.pending[("light", L1)], {"color_temperature": {"mirek": 250}, "on": {"on": True}})
        office = next(r for r in h.rooms if r["id"] == R1)
        desk = next(l for l in office["lights"] if l["id"] == L1)
        self.assertTrue(desk["on"])
        self.assertEqual(desk["mirek"], 250)
        self.assertEqual(desk["color"], hue.mirek_to_hex(250))

        # Clamping mirek to 153..500
        h.pending.clear()
        h.handle({"cmd": "light", "id": L1, "mirek": 50})
        self.assertEqual(h.pending[("light", L1)]["color_temperature"]["mirek"], 153)
        h.pending.clear()
        h.handle({"cmd": "light", "id": L1, "mirek": 900})
        self.assertEqual(h.pending[("light", L1)]["color_temperature"]["mirek"], 500)

        # XY on group
        h.pending.clear()
        h.handle({"cmd": "group", "id": GL1, "xy": [0.651234, 0.312345]})
        self.assertEqual(h.pending[("grouped_light", GL1)], {
            "color": {"xy": {"x": 0.6512, "y": 0.3123}},
            "on": {"on": True},
        })
        self.assertIsNone(desk["mirek"])
        self.assertEqual(desk["color"], hue.xy_to_hex(0.6512, 0.3123))

        # Effect turns on unless no_effect
        h.pending.clear()
        h.handle({"cmd": "light", "id": L2, "effect": "fireplace"})
        self.assertEqual(h.pending[("light", L2)], {"effects": {"effect": "fireplace"}, "on": {"on": True}})
        h.pending.clear()
        h.handle({"cmd": "light", "id": L2, "effect": "no_effect"})
        self.assertEqual(h.pending[("light", L2)], {"effects": {"effect": "no_effect"}})

    def test_hostile_mirek_xy_effect_rejected(self):
        h = self.make_hue()
        hostile = [
            {"cmd": "light", "id": L1, "mirek": float("nan")},
            {"cmd": "light", "id": L1, "mirek": float("inf")},
            {"cmd": "light", "id": L1, "mirek": True},
            {"cmd": "light", "id": L1, "mirek": "300"},
            {"cmd": "light", "id": L1, "xy": [-0.1, 0.5]},
            {"cmd": "light", "id": L1, "xy": [0.5, 1.2]},
            {"cmd": "light", "id": L1, "xy": [float("nan"), 0.5]},
            {"cmd": "light", "id": L1, "xy": [True, 0.5]},
            {"cmd": "light", "id": L1, "xy": [0.5]},
            {"cmd": "light", "id": L1, "xy": "0.5,0.5"},
            {"cmd": "light", "id": L1, "effect": "bad-dash"},
            {"cmd": "light", "id": L1, "effect": "UPPER"},
            {"cmd": "light", "id": L1, "effect": "<script>"},
            {"cmd": "light", "id": L1, "effect": "a" * 33},
            {"cmd": "light", "id": L1, "effect": ""},
            {"cmd": "light", "id": L1, "effect": 123},
        ]
        for cmd in hostile:
            h.handle(cmd)
            self.assertEqual(h.pending, {}, cmd)


class AllOnTests(Base):
    def test_all_on_uses_bridge_home_when_no_exclusions(self):
        h = self.make_hue(sample_resources(include_home=True))
        self.assertEqual(h.bridge_home_gl, HOME_GL)
        h.handle({"cmd": "all_on"})
        self.assertEqual(h.pending, {("grouped_light", HOME_GL): {"on": {"on": True}}})
        self.assertEqual(h.on_count, 4)
        self.assertTrue(all(r["on"] for r in h.rooms))
        self.assertEqual(len(h.emitted), 1)

    def test_all_on_with_exclude_rooms(self):
        h = self.make_hue(sample_resources(include_home=True))
        h.handle({"cmd": "all_on", "exclude_rooms": [R2, "not-a-uuid"]})
        self.assertEqual(h.pending, {
            ("grouped_light", GL1): {"on": {"on": True}},
            ("light", L4): {"on": {"on": True}},
        })
        office = next(r for r in h.rooms if r["id"] == R1)
        bedroom = next(r for r in h.rooms if r["id"] == R2)
        other = next(r for r in h.rooms if r["kind"] == "other")
        self.assertTrue(office["on"])
        self.assertFalse(bedroom["on"])
        self.assertTrue(other["on"])
        self.assertEqual(h.on_count, 3)

    def test_all_on_without_bridge_home_queues_rooms_and_other(self):
        h = self.make_hue(sample_resources(include_home=False))
        self.assertEqual(h.bridge_home_gl, "")
        h.handle({"cmd": "all_on", "exclude_rooms": []})
        self.assertEqual(h.pending, {
            ("grouped_light", GL1): {"on": {"on": True}},
            ("grouped_light", GL2): {"on": {"on": True}},
            ("light", L4): {"on": {"on": True}},
        })
        self.assertEqual(h.on_count, 4)


class SseTests(Base):
    def test_parse_sse_data_line_and_model_update(self):
        h = self.make_hue()
        line = "data: " + json.dumps([{
            "type": "update",
            "data": [
                {
                    "id": L1, "type": "light",
                    "on": {"on": True},
                    "dimming": {"brightness": 77.5},
                    "color_temperature": {"mirek": 280, "mirek_valid": True},
                },
                {
                    "id": GL2, "type": "grouped_light",
                    "on": {"on": True}, "dimming": {"brightness": 65.0},
                },
                {"id": "bad-id", "type": "light", "on": {"on": True}},
            ],
        }]) + "\n"
        self.assertTrue(h.handle_sse_line(line))
        office = next(r for r in h.rooms if r["id"] == R1)
        desk = next(l for l in office["lights"] if l["id"] == L1)
        bedroom = next(r for r in h.rooms if r["id"] == R2)
        self.assertTrue(desk["on"])
        self.assertEqual(desk["bri"], 77.5)
        self.assertEqual(desk["mirek"], 280)
        self.assertTrue(office["on"])
        self.assertEqual(office["on_count"], 1)
        self.assertTrue(bedroom["on"])
        self.assertEqual(bedroom["bri"], 65.0)
        self.assertEqual(len(h.emitted), 1)

        # XY update clears mirek and updates swatch
        xy_line = "data: " + json.dumps([{
            "type": "update",
            "data": [{"id": L1, "type": "light", "color": {"xy": {"x": 0.65, "y": 0.3}}}],
        }])
        self.assertTrue(h.handle_sse_line(xy_line))
        self.assertIsNone(desk["mirek"])
        self.assertEqual(desk["color"], hue.xy_to_hex(0.65, 0.3))

        # Non-data / invalid / stale generation ignored
        self.assertFalse(h.handle_sse_line(": hi\n"))
        self.assertFalse(h.handle_sse_line("id: 123:0\n"))
        self.assertFalse(h.handle_sse_line("data: not-json\n"))
        self.assertFalse(h.handle_sse_line("data: " + json.dumps([{"type": "add", "data": []}])) )
        self.assertFalse(h.handle_sse_line(line, generation=h.conn_generation + 99))

    def test_iter_bounded_lines_drops_oversized(self):
        chunks = [b"data: ok1\n", b"data: " + b"x" * 200 + b"\n", b"data: ok2\n"]
        it = iter(chunks)
        lines = list(hue.iter_bounded_lines(lambda: next(it, b""), max_line=50))
        self.assertEqual(lines, [b"data: ok1\n", b"data: ok2\n"])

    def test_open_sse_stream_enforces_tls_pin(self):
        cert = os.path.join(self.dir, "c.pem")
        key = os.path.join(self.dir, "k.pem")
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                        "-subj", "/CN=" + BRIDGE_ID, "-keyout", key, "-out", cert],
                       check=True, capture_output=True)
        with open(cert, "rb") as fh:
            pem = fh.read().decode("ascii")
        der = ssl.PEM_cert_to_DER_cert(pem)
        fp = hue.fingerprint(der)

        srv_sock = socket.socket()
        srv_sock.bind(("127.0.0.1", 0))
        srv_sock.listen(4)
        port = srv_sock.getsockname()[1]
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.load_cert_chain(cert, key)
        stop = threading.Event()
        received_headers = []

        def serve():
            srv_sock.settimeout(0.2)
            while not stop.is_set():
                try:
                    c, _ = srv_sock.accept()
                except OSError:
                    continue
                sc = None
                try:
                    sc = ctx.wrap_socket(c, server_side=True)
                    req = sc.recv(4096)
                    if req:
                        received_headers.append(req)
                        payload = b'data: [{"type":"update","data":[{"id":"' + L1.encode() + b'","type":"light","on":{"on":true}}]}]\n\n'
                        sc.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n" + payload)
                except OSError:
                    pass
                finally:
                    try:
                        (sc or c).close()
                    except OSError:
                        pass

        t = threading.Thread(target=serve, daemon=True)
        t.start()
        try:
            # Wrong pin rejected BEFORE sending app key
            with self.assertRaises(hue.PinError):
                hue.open_sse_stream("127.0.0.1", "secret_key_123",
                                    {"fingerprint": "0" * 64, "bridge_id": BRIDGE_ID}, port=port)
            self.assertEqual(received_headers, [])

            # Correct pin succeeds and streams SSE data
            conn, resp, sock = hue.open_sse_stream("127.0.0.1", "secret_key_123",
                                                   {"fingerprint": fp, "bridge_id": BRIDGE_ID}, port=port)
            try:
                lines = list(hue.iter_bounded_lines(lambda: resp.read1(4096)))
            finally:
                resp.close()
                conn.close()
                if sock is not None:
                    sock.close()
            self.assertTrue(any(b"text/event-stream" in h and b"secret_key_123" in h for h in received_headers))
            updates = [u for line in lines for u in hue.parse_sse_data_line(line)]
            self.assertEqual(updates, [("light", L1, {"on": True})])
        finally:
            stop.set()
            srv_sock.close()


class ScheduleTests(Base):
    def test_parse_behavior_scripts_and_instances(self):
        res = sample_resources() + [
            {"id": SCRIPT_CUSTOM, "type": "behavior_script", "metadata": {"name": "Custom Schedule", "category": "schedule"}},
            {
                "id": SCHED1, "type": "behavior_instance", "script_id": SCRIPT_CUSTOM,
                "enabled": True, "metadata": {"name": "<b>Wake Up</b>"},
                "configuration": {
                    "where": [{"group": {"rid": R1, "rtype": "room"}}],
                    "when": {
                        "time_point": {"type": "time", "time": {"hour": 7, "minute": 15}},
                        "recurrence_days": ["monday", "wednesday", "friday"],
                    },
                    "what": [{"on": {"on": True}, "dimming": {"brightness": 85}}],
                    "fade_in_duration": {"seconds": 600},
                },
            },
            {
                "id": SCHED2, "type": "behavior_instance", "script_id": SCRIPT_CUSTOM,
                "enabled": False, "metadata": {"name": "Sunset Lamp"},
                "configuration": {
                    "where": [{"items": [{"rid": L1, "rtype": "light"}]}],
                    "when": {
                        "time_point": {"type": "sunset", "offset": {"minutes": -15}},
                        "recurrence_days": ["saturday", "sunday"],
                    },
                    "what": [{"recall": {"rid": SCENE1, "rtype": "scene"}}],
                },
            },
        ]
        rooms, _, _ = hue.build_model(res)
        schedules, script_id = hue.parse_schedules(res, rooms)
        self.assertEqual(script_id, SCRIPT_CUSTOM)
        self.assertEqual(len(schedules), 2)
        wake, sunset = schedules  # "bWake Up/b" sorts before "Sunset Lamp"
        self.assertEqual(wake["id"], SCHED1)
        self.assertEqual(wake["name"], "bWake Up/b")
        self.assertEqual(wake["target_kind"], "room")
        self.assertEqual(wake["target_id"], R1)
        self.assertEqual(wake["target_name"], "Office")
        self.assertEqual(wake["trigger_kind"], "time")
        self.assertEqual(wake["time"], "07:15")
        self.assertEqual(wake["days"], ["mon", "wed", "fri"])
        self.assertEqual(wake["action"], "on")
        self.assertEqual(wake["bri"], 85)
        self.assertEqual(wake["fade_min"], 10)
        self.assertEqual(wake["source"], "bridge")

        self.assertEqual(sunset["target_kind"], "light")
        self.assertEqual(sunset["target_name"], "Desk")
        self.assertEqual(sunset["trigger_kind"], "sunset")
        self.assertEqual(sunset["offset_min"], -15)
        self.assertEqual(sunset["days"], ["sat", "sun"])
        self.assertEqual(sunset["action"], "scene")
        self.assertEqual(sunset["scene_id"], SCENE1)

    def test_schedule_payload_roundtrip_and_fallback_script_id(self):
        _, fallback = hue.parse_schedules(sample_resources())
        self.assertEqual(fallback, hue.DEFAULT_SCHEDULE_SCRIPT_ID)

        cmd = {
            "cmd": "schedule_create",
            "name": "Night Off",
            "enabled": True,
            "target_kind": "room",
            "target_id": R1,
            "trigger_kind": "time",
            "time": "23:30",
            "days": ["mon", "fri"],
            "action": "off",
            "fade_min": 5,
        }
        payload = hue.build_schedule_payload(cmd, fallback)
        self.assertIsNotNone(payload)
        self.assertEqual(payload["script_id"], hue.DEFAULT_SCHEDULE_SCRIPT_ID)
        parsed, _ = hue.parse_schedules(sample_resources() + [dict(payload, id=SCHED1)])
        self.assertEqual(len(parsed), 1)
        self.assertEqual(parsed[0]["name"], "Night Off")
        self.assertEqual(parsed[0]["time"], "23:30")
        self.assertEqual(parsed[0]["days"], ["mon", "fri"])
        self.assertEqual(parsed[0]["action"], "off")
        self.assertEqual(parsed[0]["fade_min"], 5)

        # Invalid schedule_create inputs rejected
        self.assertIsNone(hue.build_schedule_payload(dict(cmd, time="25:00")))
        self.assertIsNone(hue.build_schedule_payload(dict(cmd, target_id="bad")))
        self.assertIsNone(hue.build_schedule_payload(dict(cmd, days=["funday"])))
        self.assertIsNone(hue.build_schedule_payload(dict(cmd, action="scene", scene_id="bad")))
        self.assertIsNone(hue.build_schedule_payload(dict(cmd, fade_min=float("nan"))))

    def test_schedule_cap_and_commands(self):
        many = sample_resources() + [
            {
                "id": "%08x-ffff-1111-1111-111111111111" % i,
                "type": "behavior_instance", "enabled": True,
                "metadata": {"name": "S%d" % i},
                "configuration": {
                    "where": [{"group": {"rid": R1, "rtype": "room"}}],
                    "when": {"time_point": {"type": "time", "time": {"hour": 8, "minute": 0}}},
                    "what": [{"on": {"on": True}}],
                },
            }
            for i in range(150)
        ]
        schedules, _ = hue.parse_schedules(many)
        self.assertEqual(len(schedules), hue.MAX_SCHEDULES)

        # End-to-end schedule_create, schedule_toggle, schedule_delete via api_queue
        res = sample_resources() + [{
            "id": SCHED1, "type": "behavior_instance", "enabled": True,
            "metadata": {"name": "Morning"},
            "configuration": {
                "where": [{"group": {"rid": R1, "rtype": "room"}}],
                "when": {"time_point": {"type": "time", "time": {"hour": 7, "minute": 0}}},
                "what": [{"on": {"on": True}}],
            },
        }]
        h = self.make_hue(res)
        sent = []
        hue.https_request = lambda ip, method, path, body=None, headers=None, **k: sent.append((method, path, body)) or (200, {"data": []}, None)

        # Unknown target rejected
        h.handle({"cmd": "schedule_create", "name": "Bad", "target_kind": "room",
                  "target_id": "99999999-9999-9999-9999-999999999999",
                  "trigger_kind": "time", "time": "08:00", "action": "on"})
        self.assertEqual(h.api_queue, [])

        # Valid create, toggle, delete
        h.handle({"cmd": "schedule_create", "name": "Good", "target_kind": "light", "target_id": L1,
                  "trigger_kind": "sunrise", "offset_min": 10, "days": ["mon"], "action": "on", "bri": 60})
        h.handle({"cmd": "schedule_toggle", "id": SCHED1, "enabled": False})
        h.handle({"cmd": "schedule_delete", "id": SCHED1})
        # Unknown schedule ID ignored
        h.handle({"cmd": "schedule_delete", "id": SCHED2})

        while h.send_api_queue():
            pass
        self.assertEqual([(m, p) for m, p, _ in sent], [
            ("POST", "/clip/v2/resource/behavior_instance"),
            ("PUT", "/clip/v2/resource/behavior_instance/" + SCHED1),
            ("DELETE", "/clip/v2/resource/behavior_instance/" + SCHED1),
        ])
        self.assertEqual(sent[1][2], {"enabled": False})
        self.assertEqual(h.last_refresh, 0.0)


class RuleEngineTests(Base):
    def make_rule(self, **overrides):
        base = {
            "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa1",
            "name": "Desk -> Floor",
            "enabled": True,
            "kind": "follow",
            "trigger_kind": "light",
            "trigger_id": L1,
            "trigger_state": "on",
            "targets": [{"kind": "light", "id": L2, "action": "on", "bri": None, "scene_id": ""}],
            "mirror_off": False,
            "match_bri": False,
            "time_start": "",
            "time_end": "",
            "only_if_off": False,
        }
        base.update(overrides)
        return base

    def test_follow_mirror_off_and_match_bri(self):
        h = self.make_hue()
        r = self.make_rule(mirror_off=True, match_bri=True)
        h.handle({"cmd": "rule_save", "rule": r})
        self.assertEqual(len(h.rules), 1)
        self.assertEqual(os.stat(os.path.join(hue.CONFIG_DIR, "rules.json")).st_mode & 0o777, 0o600)

        # L1 turns on at bri 72 via SSE -> L2 turns on at bri 72
        ev_on = "data: " + json.dumps([{"type": "update", "data": [
            {"id": L1, "type": "light", "on": {"on": True}, "dimming": {"brightness": 72.0}},
        ]}])
        h.handle_sse_line(ev_on, now_mono=100.0)
        self.assertEqual(h.pending[("light", L2)], {"on": {"on": True}, "dimming": {"brightness": 72.0}})
        self.assertEqual(len(h.rule_log), 1)

        # L1 dims to 45 after cooldown -> L2 matches brightness 45
        h.pending.clear()
        ev_dim = "data: " + json.dumps([{"type": "update", "data": [
            {"id": L1, "type": "light", "dimming": {"brightness": 45.0}},
        ]}])
        h.handle_sse_line(ev_dim, now_mono=105.0)
        self.assertEqual(h.pending[("light", L2)], {"on": {"on": True}, "dimming": {"brightness": 45.0}})

        # L1 turns off after cooldown -> mirror_off turns L2 off
        h.pending.clear()
        ev_off = "data: " + json.dumps([{"type": "update", "data": [
            {"id": L1, "type": "light", "on": {"on": False}},
        ]}])
        h.handle_sse_line(ev_off, now_mono=110.0)
        self.assertEqual(h.pending[("light", L2)], {"on": {"on": False}})

    def test_exclusive_rule(self):
        h = self.make_hue()
        # Start with L2 on
        h.apply_optimistic("light", L2, {"on": {"on": True}})
        r = self.make_rule(kind="exclusive", targets=[{"kind": "light", "id": L2, "action": "off", "bri": None, "scene_id": ""}])
        h.handle({"cmd": "rule_save", "rule": r})
        ev = "data: " + json.dumps([{"type": "update", "data": [{"id": L1, "type": "light", "on": {"on": True}}]}])
        h.handle_sse_line(ev, now_mono=200.0)
        self.assertEqual(h.pending[("light", L2)], {"on": {"on": False}})

    def test_time_window_including_overnight(self):
        # Daytime window 08:00-18:00
        self.assertFalse(hue.in_time_window("08:00", "18:00", now=(7, 59)))
        self.assertTrue(hue.in_time_window("08:00", "18:00", now=(8, 0)))
        self.assertTrue(hue.in_time_window("08:00", "18:00", now=(17, 59)))
        self.assertFalse(hue.in_time_window("08:00", "18:00", now=(18, 0)))

        # Overnight window 22:00-06:00
        self.assertFalse(hue.in_time_window("22:00", "06:00", now=(21, 59)))
        self.assertTrue(hue.in_time_window("22:00", "06:00", now=(22, 0)))
        self.assertTrue(hue.in_time_window("22:00", "06:00", now=(0, 30)))
        self.assertTrue(hue.in_time_window("22:00", "06:00", now=(5, 59)))
        self.assertFalse(hue.in_time_window("22:00", "06:00", now=(6, 0)))
        self.assertFalse(hue.in_time_window("22:00", "06:00", now=(14, 0)))

        h = self.make_hue()
        h.handle({"cmd": "rule_save", "rule": self.make_rule(time_start="22:00", time_end="06:00")})
        ev_on = "data: " + json.dumps([{"type": "update", "data": [{"id": L1, "type": "light", "on": {"on": True}}]}])
        # Outside window (14:00) -> does not fire
        h.handle_sse_line(ev_on, now_mono=300.0, now_time=(14, 0))
        self.assertEqual(h.pending, {})
        # Reset L1 off, then turn on inside overnight window (01:30) -> fires
        h.apply_optimistic("light", L1, {"on": {"on": False}})
        h.handle_sse_line(ev_on, now_mono=310.0, now_time=(1, 30))
        self.assertEqual(h.pending[("light", L2)], {"on": {"on": True}})

    def test_only_if_off(self):
        h = self.make_hue()
        h.apply_optimistic("light", L2, {"on": {"on": True}})
        r = self.make_rule(only_if_off=True, targets=[
            {"kind": "light", "id": L2, "action": "on", "bri": 90, "scene_id": ""},
            {"kind": "light", "id": L3, "action": "on", "bri": 50, "scene_id": ""},
        ])
        h.handle({"cmd": "rule_save", "rule": r})
        ev_on = "data: " + json.dumps([{"type": "update", "data": [{"id": L1, "type": "light", "on": {"on": True}}]}])
        h.handle_sse_line(ev_on, now_mono=400.0)
        self.assertNotIn(("light", L2), h.pending)  # L2 was already on -> skipped
        self.assertEqual(h.pending[("light", L3)], {"on": {"on": True}, "dimming": {"brightness": 50.0}})

    def test_desktop_wake_and_sleep(self):
        h = self.make_hue()
        r_wake = self.make_rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa2", name="Wake Office",
            kind="desktop", trigger_kind="desktop", trigger_id="wake", trigger_state="",
            targets=[{"kind": "room", "id": R1, "action": "on", "bri": None, "scene_id": SCENE1}],
        )
        r_sleep = self.make_rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa3", name="Sleep All",
            kind="desktop", trigger_kind="desktop", trigger_id="sleep", trigger_state="",
            targets=[{"kind": "room", "id": R2, "action": "off", "bri": None, "scene_id": ""}],
        )
        h.handle({"cmd": "rule_save", "rule": r_wake})
        h.handle({"cmd": "rule_save", "rule": r_sleep})
        h.handle({"cmd": "desktop_event", "event": "wake"})
        self.assertEqual(h.pending[("scene", SCENE1)], {"recall": {"action": "active"}})
        h.pending.clear()
        h.handle({"cmd": "desktop_event", "event": "sleep"})
        self.assertEqual(h.pending[("grouped_light", GL2)], {"on": {"on": False}})

    def test_symmetric_pair_echo_suppression_no_loop(self):
        h = self.make_hue()
        r_ab = self.make_rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa4", name="A->B",
            trigger_id=L1, targets=[{"kind": "light", "id": L2, "action": "on", "bri": None, "scene_id": ""}],
            mirror_off=True,
        )
        r_ba = self.make_rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa5", name="B->A",
            trigger_id=L2, targets=[{"kind": "light", "id": L1, "action": "on", "bri": None, "scene_id": ""}],
            mirror_off=True,
        )
        h.handle({"cmd": "rule_save", "rule": r_ab})
        h.handle({"cmd": "rule_save", "rule": r_ba})
        self.assertEqual(len(h.rules), 2)
        self.assertEqual(h.error_code, "")

        # L1 turns on at t=500.0 -> A->B fires and suppresses L2 until 501.5
        ev_a = "data: " + json.dumps([{"type": "update", "data": [{"id": L1, "type": "light", "on": {"on": True}}]}])
        h.handle_sse_line(ev_a, now_mono=500.0)
        self.assertEqual(list(h.pending), [("light", L2)])
        self.assertEqual(len(h.rule_log), 1)

        # Simulate L2 SSE echo arriving 0.2s later even if L2's optimistic state were reset
        h.pending.clear()
        for r in h.rooms:
            for l in r["lights"]:
                if l["id"] == L2:
                    l["on"] = False
        ev_b = "data: " + json.dumps([{"type": "update", "data": [{"id": L2, "type": "light", "on": {"on": True}}]}])
        h.handle_sse_line(ev_b, now_mono=500.2)
        self.assertEqual(h.pending, {})  # Echo suppressed! B->A did NOT loop back!
        self.assertEqual(len(h.rule_log), 1)

    def test_cooldown_and_pause_and_log_cap(self):
        h = self.make_hue()
        h.handle({"cmd": "rule_save", "rule": self.make_rule()})
        ev_on = "data: " + json.dumps([{"type": "update", "data": [{"id": L1, "type": "light", "on": {"on": True}}]}])
        h.handle_sse_line(ev_on, now_mono=600.0)
        self.assertEqual(len(h.pending), 1)

        # Within 2.0s cooldown -> ignored
        h.pending.clear()
        h.apply_optimistic("light", L1, {"on": {"on": False}})
        h.handle_sse_line(ev_on, now_mono=601.0)
        self.assertEqual(h.pending, {})

        # Pause rules -> ignored even after cooldown
        h.handle({"cmd": "rules_pause", "paused": True})
        self.assertTrue(h.snapshot()["rules_paused"])
        h.apply_optimistic("light", L1, {"on": {"on": False}})
        h.handle_sse_line(ev_on, now_mono=605.0)
        self.assertEqual(h.pending, {})

        # Unpause -> fires again, and rule_log caps at 20
        h.handle({"cmd": "rules_pause", "paused": False})
        for i in range(25):
            h.pending.clear()
            h.apply_optimistic("light", L1, {"on": {"on": False}})
            h.handle_sse_line(ev_on, now_mono=700.0 + i * 5.0)
        self.assertEqual(len(h.rule_log), hue.MAX_RULE_LOG)

    def test_cycle_detection_and_hostile_rules(self):
        # Contradictory cycle: A ON -> B OFF and B OFF -> A OFF
        r1 = self.make_rule(id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaa11", trigger_id=L1, trigger_state="on",
                            targets=[{"kind": "light", "id": L2, "action": "off", "bri": None, "scene_id": ""}])
        r2 = self.make_rule(id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaa12", trigger_id=L2, trigger_state="off",
                            targets=[{"kind": "light", "id": L1, "action": "off", "bri": None, "scene_id": ""}])
        self.assertTrue(hue.detect_rule_cycles([r1, r2]))

        # Safe mutual exclusion: A ON -> B OFF and B ON -> A OFF (allowed!)
        r2_excl = self.make_rule(id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaa13", trigger_id=L2, trigger_state="on",
                                 targets=[{"kind": "light", "id": L1, "action": "off", "bri": None, "scene_id": ""}])
        self.assertEqual(hue.detect_rule_cycles([r1, r2_excl]), "")

        # Chain depth 4 (> 3) rejected; depth 3 allowed
        c1 = self.make_rule(id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaa21", trigger_id=L1,
                            targets=[{"kind": "light", "id": L2, "action": "on", "bri": None, "scene_id": ""}])
        c2 = self.make_rule(id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaa22", trigger_id=L2,
                            targets=[{"kind": "light", "id": L3, "action": "on", "bri": None, "scene_id": ""}])
        c3 = self.make_rule(id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaa23", trigger_id=L3,
                            targets=[{"kind": "light", "id": L4, "action": "on", "bri": None, "scene_id": ""}])
        c4 = self.make_rule(id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaa24", trigger_id=L4,
                            targets=[{"kind": "light", "id": L5, "action": "on", "bri": None, "scene_id": ""}])
        self.assertEqual(hue.detect_rule_cycles([c1, c2, c3]), "")
        self.assertTrue(hue.detect_rule_cycles([c1, c2, c3, c4]))

        # Saving contradictory cycle via Hue rejects and keeps existing rules intact
        h = self.make_hue()
        h.handle({"cmd": "rule_save", "rule": r1})
        h.handle({"cmd": "rule_save", "rule": r2})
        self.assertEqual(len(h.rules), 1)
        self.assertEqual(h.error_code, "errRuleCycle")

        # Hostile / invalid rules rejected
        for bad in (
            None, "nope", {},
            self.make_rule(id="not-a-uuid"),
            self.make_rule(kind="evil"),
            self.make_rule(trigger_kind="bridge"),
            self.make_rule(trigger_kind="desktop", trigger_id="reboot"),
            self.make_rule(targets=[]),
            self.make_rule(targets=[{"kind": "light", "id": L1, "action": "on", "bri": None, "scene_id": ""}]),  # self-target
            self.make_rule(targets=[{"kind": "light", "id": "%08x-1111-1111-1111-111111111111" % i,
                                     "action": "on", "bri": None, "scene_id": ""} for i in range(17)]),
            self.make_rule(time_start="22:00", time_end=""),
            self.make_rule(time_start="99:99", time_end="06:00"),
            self.make_rule(mirror_off="yes"),
            self.make_rule(targets=[{"kind": "light", "id": L2, "action": "on", "bri": float("nan"), "scene_id": ""}]),
        ):
            self.assertIsNone(hue.validate_rule(bad), bad)

    def test_rules_persistence_hardening(self):
        # Symlinked rules.json rejected on load
        os.makedirs(hue.CONFIG_DIR, mode=0o700)
        other = os.path.join(self.dir, "other.json")
        with open(other, "w") as fh:
            json.dump({"paused": True, "rules": [self.make_rule()]}, fh)
        os.symlink(other, os.path.join(hue.CONFIG_DIR, "rules.json"))
        self.assertEqual(hue.load_rules(), {"paused": False, "rules": []})
        os.unlink(os.path.join(hue.CONFIG_DIR, "rules.json"))

        # Oversized rules.json rejected
        with open(os.path.join(hue.CONFIG_DIR, "rules.json"), "w") as fh:
            fh.write(" " * (hue.MAX_CONFIG + 1))
        self.assertEqual(hue.load_rules(), {"paused": False, "rules": []})

        # Toggle and delete persist cleanly
        h = self.make_hue()
        r = self.make_rule()
        h.handle({"cmd": "rule_save", "rule": r})
        h.handle({"cmd": "rule_toggle", "id": r["id"], "enabled": False})
        self.assertFalse(hue.load_rules()["rules"][0]["enabled"])
        h.handle({"cmd": "rule_delete", "id": r["id"]})
        self.assertEqual(hue.load_rules()["rules"], [])

    def test_state_aware_echo_suppression(self):
        h = self.make_hue()
        # A ON -> B ON, B ON -> C ON (should be suppressed on B's ON echo),
        # B OFF -> C ON (opposite external toggle within 1.5s MUST fire!)
        r_ab = self.make_rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaab01", name="A->B",
            trigger_id=L1, trigger_state="on",
            targets=[{"kind": "light", "id": L2, "action": "on", "bri": None, "scene_id": ""}],
        )
        r_bc_on = self.make_rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaab02", name="B ON->C",
            trigger_id=L2, trigger_state="on",
            targets=[{"kind": "light", "id": L3, "action": "on", "bri": None, "scene_id": ""}],
        )
        r_bc_off = self.make_rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaab03", name="B OFF->C",
            trigger_id=L2, trigger_state="off",
            targets=[{"kind": "light", "id": L3, "action": "on", "bri": None, "scene_id": ""}],
        )
        for r in (r_ab, r_bc_on, r_bc_off):
            h.handle({"cmd": "rule_save", "rule": r})

        # L1 turns on at t=800.0 -> A->B fires and stores (801.5, True)
        ev_a = "data: " + json.dumps([{"type": "update", "data": [{"id": L1, "type": "light", "on": {"on": True}}]}])
        h.handle_sse_line(ev_a, now_mono=800.0)
        self.assertEqual(h.pending, {("light", L2): {"on": {"on": True}}})
        self.assertEqual(h.echo_suppress[("light", L2)], (800.0 + hue.RULE_ECHO_WINDOW, True))

        # Same-state ON echo 0.2s later -> suppressed (B ON->C does NOT fire)
        h.pending.clear()
        h.apply_optimistic("light", L2, {"on": {"on": False}})
        ev_b_on = "data: " + json.dumps([{"type": "update", "data": [{"id": L2, "type": "light", "on": {"on": True}}]}])
        h.handle_sse_line(ev_b_on, now_mono=800.2)
        self.assertEqual(h.pending, {})

        # Opposite external toggle (B OFF) 0.4s later -> NOT suppressed (B OFF->C FIRES!)
        ev_b_off = "data: " + json.dumps([{"type": "update", "data": [{"id": L2, "type": "light", "on": {"on": False}}]}])
        h.handle_sse_line(ev_b_off, now_mono=800.4)
        self.assertEqual(h.pending, {("light", L3): {"on": {"on": True}}})

        # Backward compatibility: legacy bare float timestamp still suppresses any transition
        h.pending.clear()
        h.rule_last_fired.clear()
        h.apply_optimistic("light", L2, {"on": {"on": True}})
        h.echo_suppress[("light", L2)] = 905.0
        h.handle_sse_line(ev_b_off, now_mono=900.0)
        self.assertEqual(h.pending, {})


class LightThemeTests(Base):
    def test_hex_to_xy_and_builtins(self):
        self.assertEqual(len(hue.BUILTIN_THEMES), 8)
        ids = [t["id"] for t in hue.BUILTIN_THEMES]
        self.assertEqual(ids, ["reading", "relax", "focus", "sunset", "ocean", "cyberpunk", "fireplace", "nightlight"])
        self.assertTrue(all(t["built_in"] is True for t in hue.BUILTIN_THEMES))

        for color in ("#ff0000", "#00ff00", "#0000ff", "#ffffff", "#000000", "ff6b35", "#FFD9A0"):
            xy = hue.hex_to_xy(color)
            self.assertIsNotNone(xy, color)
            self.assertEqual(len(xy), 2)
            self.assertTrue(0.0 <= xy[0] <= 1.0 and 0.0 <= xy[1] <= 1.0)
        # Red has higher x than blue; blue has lower y than green
        red = hue.hex_to_xy("#ff0000")
        green = hue.hex_to_xy("#00ff00")
        blue = hue.hex_to_xy("#0000ff")
        self.assertGreater(red[0], blue[0])
        self.assertGreater(green[1], blue[1])

        for bad in (None, "", "#123", "#1234567", "#gggggg", "not-hex", 12345):
            self.assertIsNone(hue.hex_to_xy(bad), bad)

    def test_theme_save_delete_persistence_and_builtin_protection(self):
        h = self.make_hue()
        self.assertEqual(len(h.snapshot()["themes"]), 8)

        # Save custom theme (auto-UUID)
        h.handle({"cmd": "theme_save", "theme": {
            "name": "Cozy Purple", "hex": "#9b59b6", "mirek": None, "bri": 68, "effect": "no_effect",
        }})
        self.assertEqual(len(h.custom_themes), 1)
        tid = h.custom_themes[0]["id"]
        self.assertFalse(h.custom_themes[0]["built_in"])
        self.assertEqual(len(h.snapshot()["themes"]), 9)
        self.assertEqual(os.stat(os.path.join(hue.CONFIG_DIR, "themes.json")).st_mode & 0o777, 0o600)

        # Upsert existing custom theme by id
        h.handle({"cmd": "theme_save", "theme": {
            "id": tid, "name": "Cozy Violet", "hex": "#8e44ad", "mirek": None, "bri": 75, "effect": "candle",
        }})
        self.assertEqual(len(h.custom_themes), 1)
        self.assertEqual(h.custom_themes[0]["name"], "Cozy Violet")
        self.assertEqual(hue.load_themes()[0]["bri"], 75)

        # Cannot overwrite or delete a built-in theme
        h.handle({"cmd": "theme_save", "theme": {
            "id": "reading", "name": "Hacked", "hex": "#000000", "mirek": 200, "bri": 50, "effect": "no_effect",
        }})
        self.assertEqual(len(h.custom_themes), 1)
        h.handle({"cmd": "theme_delete", "id": "reading"})
        self.assertEqual(len(h.snapshot()["themes"]), 9)

        # Delete custom theme persists
        h.handle({"cmd": "theme_delete", "id": tid})
        self.assertEqual(h.custom_themes, [])
        self.assertEqual(hue.load_themes(), [])

        # Symlinked themes.json rejected
        other = os.path.join(self.dir, "other_themes.json")
        with open(other, "w") as fh:
            json.dump([{"id": "custom1", "name": "Sneaky", "hex": "#112233", "bri": 50}], fh)
        os.unlink(os.path.join(hue.CONFIG_DIR, "themes.json"))
        os.symlink(other, os.path.join(hue.CONFIG_DIR, "themes.json"))
        self.assertEqual(hue.load_themes(), [])

    def test_theme_apply_ct_vs_color_and_effects(self):
        h = self.make_hue()

        # Nightlight (mirek=500) on L1 (supports_ct=True, mirek_max=454) clamps mirek to 454
        h.handle({"cmd": "theme_apply", "theme_id": "nightlight", "light_id": L1})
        self.assertEqual(h.pending[("light", L1)], {
            "on": {"on": True},
            "dimming": {"brightness": 12.0},
            "color_temperature": {"mirek": 454},
            "effects": {"effect": "no_effect"},
        })

        # Fireplace (mirek=450, effect="candle") on L1 includes candle effect
        h.pending.clear()
        h.handle({"cmd": "theme_apply", "theme_id": "fireplace", "light_id": L1})
        self.assertEqual(h.pending[("light", L1)], {
            "on": {"on": True},
            "dimming": {"brightness": 60.0},
            "color_temperature": {"mirek": 450},
            "effects": {"effect": "candle"},
        })

        # Fireplace on L2 (supports_ct=True, available_effects=[]) omits unsupported "candle" effect
        h.pending.clear()
        h.handle({"cmd": "theme_apply", "theme_id": "fireplace", "light_id": L2})
        self.assertEqual(h.pending[("light", L2)], {
            "on": {"on": True},
            "dimming": {"brightness": 60.0},
            "color_temperature": {"mirek": 450},
        })

        # Cyberpunk (mirek=None) on L1 (supports_color=True) uses color.xy
        h.pending.clear()
        h.handle({"cmd": "theme_apply", "theme_id": "cyberpunk", "light_id": L1})
        xy = hue.hex_to_xy("#ff2e93")
        self.assertEqual(h.pending[("light", L1)], {
            "on": {"on": True},
            "dimming": {"brightness": 85.0},
            "color": {"xy": {"x": xy[0], "y": xy[1]}},
            "effects": {"effect": "no_effect"},
        })

        # Cyberpunk on L3 (dimmable-only: no CT, no color) omits color and CT
        h.pending.clear()
        h.handle({"cmd": "theme_apply", "theme_id": "cyberpunk", "light_id": L3})
        self.assertEqual(h.pending[("light", L3)], {
            "on": {"on": True},
            "dimming": {"brightness": 85.0},
            "effects": {"effect": "no_effect"},
        })

        # Unknown light or unknown theme ignored
        h.pending.clear()
        h.handle({"cmd": "theme_apply", "theme_id": "nonexistent", "light_id": L1})
        h.handle({"cmd": "theme_apply", "theme_id": "reading", "light_id": "99999999-9999-9999-9999-999999999999"})
        self.assertEqual(h.pending, {})


class LocalScheduleTests(Base):
    def test_schedule_mode_persistence_and_local_crud(self):
        res = sample_resources() + [{
            "id": SCHED1, "type": "behavior_instance", "enabled": True,
            "metadata": {"name": "Bridge Morning"},
            "configuration": {
                "where": [{"group": {"rid": R1, "rtype": "room"}}],
                "when": {"time_point": {"type": "time", "time": {"hour": 7, "minute": 0}}},
                "what": [{"on": {"on": True}}],
            },
        }]
        h = self.make_hue(res)
        self.assertEqual(h.snapshot()["schedule_mode"], "bridge")
        self.assertEqual(len(h.schedules), 1)

        # Switch schedule_mode to local and verify persistence + 0600 mode
        h.handle({"cmd": "schedule_mode", "mode": "local"})
        self.assertEqual(h.snapshot()["schedule_mode"], "local")
        self.assertEqual(hue.load_local_schedules()["mode"], "local")
        self.assertEqual(os.stat(os.path.join(hue.CONFIG_DIR, "local_schedules.json")).st_mode & 0o777, 0o600)
        # Invalid mode ignored
        h.handle({"cmd": "schedule_mode", "mode": "cloud"})
        self.assertEqual(h.schedule_mode, "local")

        # Create local schedule -> saved locally, combined in h.schedules, api_queue stays empty
        h.handle({
            "cmd": "schedule_create", "name": "Desk Wake", "target_kind": "light", "target_id": L1,
            "trigger_kind": "time", "time": "08:15", "days": ["mon", "wed", "fri"],
            "action": "on", "bri": 80, "fade_min": 2,
        })
        self.assertEqual(h.api_queue, [])
        self.assertEqual(len(h.local_schedules), 1)
        self.assertEqual(len(h.schedules), 2)
        local_s = h.local_schedules[0]
        self.assertEqual(local_s["source"], "local")
        self.assertEqual(local_s["target_name"], "Desk")
        self.assertEqual(local_s["time"], "08:15")
        self.assertEqual(local_s["days"], ["mon", "wed", "fri"])

        # Toggle local schedule locally (does NOT touch bridge api_queue)
        h.handle({"cmd": "schedule_toggle", "id": local_s["id"], "enabled": False})
        self.assertEqual(h.api_queue, [])
        self.assertFalse(h.local_schedules[0]["enabled"])
        self.assertFalse(hue.load_local_schedules()["schedules"][0]["enabled"])

        # Toggle bridge schedule still queues PUT to Bridge
        h.handle({"cmd": "schedule_toggle", "id": SCHED1, "enabled": False})
        self.assertEqual(len(h.api_queue), 1)
        self.assertEqual(h.api_queue[0][0], "PUT")

        # Delete local schedule locally
        h.api_queue.clear()
        h.handle({"cmd": "schedule_delete", "id": local_s["id"]})
        self.assertEqual(h.api_queue, [])
        self.assertEqual(h.local_schedules, [])
        self.assertEqual(len(h.schedules), 1)
        self.assertEqual(hue.load_local_schedules()["schedules"], [])

    def test_check_local_schedules_weekday_and_dedup(self):
        h = self.make_hue()
        h.handle({"cmd": "schedule_mode", "mode": "local"})
        # Schedule 1: Monday 08:30 -> L1 ON @ 75%
        h.handle({
            "cmd": "schedule_create", "name": "Mon Desk", "target_kind": "light", "target_id": L1,
            "time": "08:30", "days": ["mon"], "action": "on", "bri": 75,
        })
        # Schedule 2: Every day 22:00 -> R1 scene SCENE1
        h.handle({
            "cmd": "schedule_create", "name": "Night Scene", "target_kind": "room", "target_id": R1,
            "time": "22:00", "days": [], "action": "scene", "scene_id": SCENE1,
        })

        # Tuesday 08:30 -> Monday-only schedule must NOT fire
        tue_0830 = datetime.datetime(2026, 11, 24, 8, 30)  # 2026-11-24 is Tuesday
        self.assertEqual(tue_0830.weekday(), 1)
        self.assertEqual(h.check_local_schedules(now=tue_0830), 0)
        self.assertEqual(h.pending, {})

        # Monday 08:30 -> fires once
        mon_0830 = datetime.datetime(2026, 11, 23, 8, 30)  # Monday
        self.assertEqual(mon_0830.weekday(), 0)
        self.assertEqual(h.check_local_schedules(now=mon_0830), 1)
        self.assertEqual(h.pending[("light", L1)], {"on": {"on": True}, "dimming": {"brightness": 75.0}})

        # Calling again in the same minute is deduplicated!
        h.pending.clear()
        self.assertEqual(h.check_local_schedules(now=mon_0830), 0)
        self.assertEqual(h.pending, {})

        # Next Monday at 08:30 fires again
        next_mon_0830 = datetime.datetime(2026, 11, 30, 8, 30)
        self.assertEqual(h.check_local_schedules(now=next_mon_0830), 1)
        self.assertIn(("light", L1), h.pending)

        # Everyday 22:00 scene fires on Tuesday
        h.pending.clear()
        tue_2200 = datetime.datetime(2026, 11, 24, 22, 0)
        self.assertEqual(h.check_local_schedules(now=tue_2200), 1)
        self.assertEqual(h.pending[("scene", SCENE1)], {"recall": {"action": "active"}})

    def test_local_schedule_lock_fails_closed_and_preserves_claims(self):
        h1 = self.make_hue()
        h1.handle({"cmd": "schedule_mode", "mode": "local"})
        h1.handle({
            "cmd": "schedule_create", "name": "Desk 09:00", "target_kind": "light", "target_id": L1,
            "time": "09:00", "days": ["mon"], "action": "on", "bri": 80,
        })
        # Second independent helper loads the same persisted schedules
        h2 = self.make_hue()
        mon_0900 = datetime.datetime(2026, 11, 23, 9, 0)
        self.assertEqual(h1.check_local_schedules(now=mon_0900), 1)
        # Second helper in the same minute must NOT double-fire
        self.assertEqual(h2.check_local_schedules(now=mon_0900), 0)
        # Writing schedule_mode from stale h2 preserves h1's last_fired claim on disk
        h2.handle({"cmd": "schedule_mode", "mode": "local"})
        persisted = hue.load_local_schedules()["schedules"][0]
        self.assertEqual(persisted.get("last_fired"), "2026-11-23 09:00")

        # Symlinked lock file fails closed and never touches the symlink target
        victim = os.path.join(os.path.dirname(hue.CONFIG_DIR), "victim.txt")
        with open(victim, "w") as f:
            f.write("KEEP")
        lock_path = os.path.join(hue.CONFIG_DIR, ".local_schedules.lock")
        if os.path.exists(lock_path):
            os.unlink(lock_path)
        os.symlink(victim, lock_path)
        self.assertFalse(hue.claim_local_schedule_bucket(persisted["id"], "2026-11-30 09:00"))
        with open(victim) as f:
            self.assertEqual(f.read(), "KEEP")

    def test_sse_iter_bounded_lines_times_out_on_dripping_partial_line(self):
        chunks = [b"data: partial", None, None]
        def reader():
            if not chunks:
                return b""
            c = chunks.pop(0)
            if c is None:
                time.sleep(0.04)
            return c
        with self.assertRaises(OSError):
            list(hue.iter_bounded_lines(reader, max_line=1024, line_timeout=0.03))


class AudioSynthTests(Base):
    def _sine_samples(self, freq, n=512, sr=16000, amp=0.9):
        return [amp * math.sin(2.0 * math.pi * freq * i / sr) for i in range(n)]

    def test_analyze_pcm_bands_and_beat_tracker(self):
        # 100 Hz sine -> bass dominates
        b1, m1, t1 = hue.analyze_pcm_bands(self._sine_samples(100.0), sample_rate=16000)
        self.assertGreater(b1, 0.7)
        self.assertGreater(b1, m1 * 3.0)
        self.assertGreater(b1, t1 * 3.0)

        # 4000 Hz sine (raw s16le bytes) -> treble dominates
        s4k = [int(28000 * math.sin(2.0 * math.pi * 4000.0 * i / 16000.0)) for i in range(512)]
        raw_bytes = struct.pack("<%dh" % len(s4k), *s4k)
        b2, m2, t2 = hue.analyze_pcm_bands(raw_bytes, sample_rate=16000)
        self.assertGreater(t2, 0.3)
        self.assertGreater(t2, b2 * 3.0)
        self.assertGreater(t2, m2 * 2.0)

        # Empty -> zeros
        self.assertEqual(hue.analyze_pcm_bands([]), (0.0, 0.0, 0.0))

        # AudioBeatTracker detects sudden bass hit and respects min_beat_gap
        tracker = hue.AudioBeatTracker()
        for i in range(6):
            _, _, _, beat = tracker.update(0.05, 0.05, 0.05, now_mono=10.0 + i * 0.05)
            self.assertFalse(beat)
        b, m, t, beat = tracker.update(0.9, 0.2, 0.1, now_mono=10.5)
        self.assertTrue(beat)
        self.assertTrue(0.0 <= b <= 1.0 and 0.0 <= m <= 1.0 and 0.0 <= t <= 1.0)
        # Immediate second hit within min_beat_gap suppressed
        _, _, _, beat2 = tracker.update(0.95, 0.2, 0.1, now_mono=10.55)
        self.assertFalse(beat2)

    def test_demo_beat_levels_and_map_synth_frame(self):
        b0, m0, t0, beat0 = hue.demo_beat_levels(0.0, bpm=120.0)
        b1, m1, t1, beat1 = hue.demo_beat_levels(0.25, bpm=120.0)
        b2, m2, t2, beat2 = hue.demo_beat_levels(0.50, bpm=120.0)
        self.assertTrue(beat0)
        self.assertFalse(beat1)
        self.assertTrue(beat2)
        self.assertGreater(b0, b1)
        for v in (b0, m0, t0, b1, m1, t1):
            self.assertTrue(0.0 <= v <= 1.0)

        for mode in hue.SYNTH_MODES:
            for palette in hue.SYNTH_PALETTES:
                for band in hue.SYNTH_BANDS:
                    frame = hue.map_synth_frame(1, (0.8, 0.4, 0.6, True), mode, palette, band, 1.2, 0.4, 1.5)
                    self.assertTrue(1 <= frame["bri"] <= 100)
                    self.assertEqual(len(frame["xy"]), 2)
                    self.assertTrue(0.0 <= frame["xy"][0] <= 1.0 and 0.0 <= frame["xy"][1] <= 1.0)
                    self.assertRegex(frame["hex"], r"^#[0-9a-f]{6}$")

    def test_synth_demo_lifecycle_and_missing_parec(self):
        h = self.make_hue()
        try:
            h.handle({
                "cmd": "synth_config", "source": "demo", "mode": "club", "palette": "aurora",
                "room_id": R1, "sensitivity": 1.3, "smoothness": 0.3,
                "light_bands": {L1: "bass", L2: "treble"},
            })
            snap = h.snapshot()["synth"]
            self.assertEqual(snap["source"], "demo")
            self.assertEqual(snap["palette"], "aurora")
            self.assertEqual(snap["room_id"], R1)
            self.assertEqual(snap["light_bands"], {L1: "bass", L2: "treble"})
            self.assertFalse(snap["running"])

            # Start demo synth -> queues lights in R1 (L1, L2) and suppresses echo
            h.handle({"cmd": "synth_start"})
            self.assertTrue(h.snapshot()["synth"]["running"])
            deadline = time.monotonic() + 1.5
            while time.monotonic() < deadline and ("light", L1) not in h.pending:
                time.sleep(0.03)
            self.assertIn(("light", L1), h.pending)
            self.assertIn(("light", L2), h.pending)
            self.assertNotIn(("light", L3), h.pending)  # outside room R1
            self.assertIn("color", h.pending[("light", L1)])  # L1 supports color
            self.assertNotIn("color", h.pending[("light", L2)])  # L2 is CT-only
            self.assertIn(("light", L1), h.echo_suppress)

            # Switching capture source while running MUST stop synth (never auto-launch parec)
            h.handle({"cmd": "synth_config", "source": "mic"})
            self.assertFalse(h.snapshot()["synth"]["running"])
            self.assertEqual(h.snapshot()["synth"]["source"], "mic")

            # Restart in demo and stop cleanly
            h.handle({"cmd": "synth_config", "source": "demo"})
            h.handle({"cmd": "synth_start"})
            self.assertTrue(h.snapshot()["synth"]["running"])
            h.handle({"cmd": "synth_stop"})
            snap_stopped = h.snapshot()["synth"]
            self.assertFalse(snap_stopped["running"])
            self.assertEqual((snap_stopped["bass"], snap_stopped["mids"], snap_stopped["treble"]), (0.0, 0.0, 0.0))

            # Missing `parec` sets error cleanly and stops without crashing
            real_which = hue.shutil.which
            try:
                hue.shutil.which = lambda _: None
                h.handle({"cmd": "synth_config", "source": "system"})
                h.handle({"cmd": "synth_start"})
                deadline = time.monotonic() + 1.5
                while time.monotonic() < deadline and h.snapshot()["synth"]["running"]:
                    time.sleep(0.02)
                snap_err = h.snapshot()["synth"]
                self.assertFalse(snap_err["running"])
                self.assertTrue(bool(snap_err["error"]))
            finally:
                hue.shutil.which = real_which
        finally:
            h.synth.stop(emit=False)


if __name__ == "__main__":
    unittest.main()
