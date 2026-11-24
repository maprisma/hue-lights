"""Unit tests for richer light control, all_on, SSE eventstream, bridge schedules, and dependency rules."""
import importlib.util
import json
import os
import shutil
import socket
import ssl
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


if __name__ == "__main__":
    unittest.main()
