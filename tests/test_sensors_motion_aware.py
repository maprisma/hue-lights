import importlib.util
import json
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("hue", os.path.join(HERE, "..", "scripts", "hue-lights.py"))
hue = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hue)

# Deterministic UUIDs for fixtures
L1 = "11111111-1111-1111-1111-111111111111"
L2 = "22222222-2222-2222-2222-222222222222"
L3 = "33333333-3333-3333-3333-333333333333"
D1 = "aaaaaaaa-1111-1111-1111-111111111111"
D2 = "aaaaaaaa-2222-2222-2222-222222222222"
D3 = "aaaaaaaa-3333-3333-3333-333333333333"
D_BRIDGE = "aaaaaaaa-0000-0000-0000-000000000001"
BRIDGE_SVC = "bbbbbbbb-0000-0000-0000-000000000001"
GL1 = "bbbbbbbb-1111-1111-1111-111111111111"
ROOM1 = "cccccccc-1111-1111-1111-111111111111"
ROOM2 = "cccccccc-2222-2222-2222-222222222222"

# MotionAware resources
CAND1 = "44444444-1111-1111-1111-111111111111"
CAND2 = "44444444-2222-2222-2222-222222222222"
CAND3 = "44444444-3333-3333-3333-333333333333"
MAC1 = "55555555-1111-1111-1111-111111111111"
MAC2 = "55555555-2222-2222-2222-222222222222"
CONV1 = "66666666-1111-1111-1111-111111111111"
SEC2 = "66666666-2222-2222-2222-222222222222"

# Physical sensor resources
D_MOT = "77777777-1111-1111-1111-111111111111"
S_MOT = "77777777-2222-2222-2222-222222222222"
S_TEMP = "77777777-3333-3333-3333-333333333333"
S_LUX = "77777777-4444-4444-4444-444444444444"
S_POW_MOT = "77777777-5555-5555-5555-555555555555"

D_CON = "88888888-1111-1111-1111-111111111111"
S_CON = "88888888-2222-2222-2222-222222222222"
S_POW_CON = "88888888-3333-3333-3333-333333333333"

D_BTN = "99999999-1111-1111-1111-111111111111"
S_BTN1 = "99999999-2222-2222-2222-222222222222"
S_BTN2 = "99999999-3333-3333-3333-333333333333"
S_POW_BTN = "99999999-4444-4444-4444-444444444444"

BRIDGE_ID = "001788FFFE112233"


def build_pro_resources(model_id="BSB003", area1_motion=False, area1_enabled=True):
    return [
        {
            "id": D_BRIDGE,
            "type": "device",
            "product_data": {"model_id": model_id, "product_name": "Hue Bridge Pro"},
            "metadata": {"name": "Hue Bridge Pro"},
            "services": [{"rid": BRIDGE_SVC, "rtype": "bridge"}],
        },
        {
            "id": L1,
            "type": "light",
            "owner": {"rid": D1, "rtype": "device"},
            "metadata": {"name": "Sofa Lamp"},
            "on": {"on": True},
            "dimming": {"brightness": 70.0},
        },
        {
            "id": L2,
            "type": "light",
            "owner": {"rid": D2, "rtype": "device"},
            "metadata": {"name": "Floor Lamp"},
            "on": {"on": False},
            "dimming": {"brightness": 50.0},
        },
        {
            "id": L3,
            "type": "light",
            "owner": {"rid": D3, "rtype": "device"},
            "metadata": {"name": "Desk Lamp"},
            "on": {"on": False},
            "dimming": {"brightness": 40.0},
        },
        {"id": D1, "type": "device", "services": [{"rid": L1, "rtype": "light"}]},
        {"id": D2, "type": "device", "services": [{"rid": L2, "rtype": "light"}]},
        {"id": D3, "type": "device", "services": [{"rid": L3, "rtype": "light"}]},
        {"id": GL1, "type": "grouped_light", "on": {"on": True}, "dimming": {"brightness": 70.0}},
        {
            "id": ROOM1,
            "type": "room",
            "metadata": {"name": "Living Room", "archetype": "living_room"},
            "children": [
                {"rid": D1, "rtype": "device"},
                {"rid": D2, "rtype": "device"},
                {"rid": D_MOT, "rtype": "device"},
                {"rid": D_BTN, "rtype": "device"},
            ],
            "services": [{"rid": GL1, "rtype": "grouped_light"}],
        },
        {
            "id": ROOM2,
            "type": "room",
            "metadata": {"name": "Office", "archetype": "office"},
            "children": [
                {"rid": D3, "rtype": "device"},
                {"rid": D_CON, "rtype": "device"},
            ],
            "services": [],
        },
        # MotionAware candidates + configurations + area motion services
        {"id": CAND1, "type": "motion_area_candidate", "owner": {"rid": D1, "rtype": "device"}},
        {"id": CAND2, "type": "motion_area_candidate", "owner": {"rid": D2, "rtype": "device"}},
        {"id": CAND3, "type": "motion_area_candidate", "owner": {"rid": D3, "rtype": "device"}},
        {
            "id": MAC1,
            "type": "motion_area_configuration",
            "name": "Living Room Motion",
            "enabled": area1_enabled,
            "group": {"rid": ROOM1, "rtype": "room"},
            "participants": [
                {"resource": {"rid": CAND1, "rtype": "motion_area_candidate"}, "status": {"health": "healthy"}},
                {"resource": {"rid": CAND2, "rtype": "motion_area_candidate"}, "status": {"health": "healthy"}},
            ],
        },
        {
            "id": CONV1,
            "type": "convenience_area_motion",
            "owner": {"rid": MAC1, "rtype": "motion_area_configuration"},
            "enabled": area1_enabled,
            "motion": {
                "motion": False,
                "motion_valid": True,
                "motion_report": {"motion": area1_motion, "changed": "2026-11-24T20:00:00Z"},
            },
            "sensitivity": {"sensitivity": 3, "sensitivity_max": 4},
        },
        {
            "id": MAC2,
            "type": "motion_area_configuration",
            "name": "Office Security",
            "enabled": False,
            "group": {"rid": ROOM2, "rtype": "room"},
            "participants": [
                {"resource": {"rid": CAND3, "rtype": "motion_area_candidate"}, "status": {"health": "degraded"}},
            ],
        },
        {
            "id": SEC2,
            "type": "security_area_motion",
            "owner": {"rid": MAC2, "rtype": "motion_area_configuration"},
            "enabled": False,
            "motion": {"motion": True, "motion_valid": False},
            "sensitivity": {"sensitivity": 2, "sensitivity_max": 4},
        },
        # Physical motion sensor + temp + lux + power
        {
            "id": D_MOT,
            "type": "device",
            "metadata": {"name": "Hallway Motion"},
            "services": [
                {"rid": S_MOT, "rtype": "motion"},
                {"rid": S_TEMP, "rtype": "temperature"},
                {"rid": S_LUX, "rtype": "light_level"},
                {"rid": S_POW_MOT, "rtype": "device_power"},
            ],
        },
        {
            "id": S_MOT,
            "type": "motion",
            "owner": {"rid": D_MOT, "rtype": "device"},
            "enabled": True,
            "motion": {
                "motion": False,
                "motion_valid": True,
                "motion_report": {"motion": False},
            },
            "sensitivity": {"sensitivity": 2, "sensitivity_max": 4},
        },
        {
            "id": S_TEMP,
            "type": "temperature",
            "owner": {"rid": D_MOT, "rtype": "device"},
            "enabled": True,
            "temperature": {
                "temperature": 21.38,
                "temperature_valid": True,
                "temperature_report": {"temperature": 21.38},
            },
        },
        {
            "id": S_LUX,
            "type": "light_level",
            "owner": {"rid": D_MOT, "rtype": "device"},
            "enabled": True,
            "light": {
                "light_level": 16233,
                "light_level_valid": True,
                "light_level_report": {"light_level": 16233},
            },
        },
        {
            "id": S_POW_MOT,
            "type": "device_power",
            "owner": {"rid": D_MOT, "rtype": "device"},
            "power_state": {"battery_level": 88, "battery_state": "normal"},
        },
        # Physical contact sensor + power
        {
            "id": D_CON,
            "type": "device",
            "metadata": {"name": "Office Window"},
            "services": [
                {"rid": S_CON, "rtype": "contact"},
                {"rid": S_POW_CON, "rtype": "device_power"},
            ],
        },
        {
            "id": S_CON,
            "type": "contact",
            "owner": {"rid": D_CON, "rtype": "device"},
            "enabled": True,
            "contact_report": {"state": "no_contact", "changed": "2026-11-24T19:55:00Z"},
        },
        {
            "id": S_POW_CON,
            "type": "device_power",
            "owner": {"rid": D_CON, "rtype": "device"},
            "power_state": {"battery_level": 95, "battery_state": "normal"},
        },
        # Physical switch / multi-button device + power
        {
            "id": D_BTN,
            "type": "device",
            "metadata": {"name": "Dimmer Switch"},
            "services": [
                {"rid": S_BTN1, "rtype": "button"},
                {"rid": S_BTN2, "rtype": "button"},
                {"rid": S_POW_BTN, "rtype": "device_power"},
            ],
        },
        {
            "id": S_BTN1,
            "type": "button",
            "owner": {"rid": D_BTN, "rtype": "device"},
            "button": {
                "last_event": "initial_press",
                "button_report": {"updated": "2026-11-24T19:00:00Z", "event": "initial_press"},
            },
        },
        {
            "id": S_BTN2,
            "type": "button",
            "owner": {"rid": D_BTN, "rtype": "device"},
            "button": {
                "last_event": "short_release",
                "button_report": {"updated": "2026-11-24T19:10:00Z", "event": "short_release"},
            },
        },
        {
            "id": S_POW_BTN,
            "type": "device_power",
            "owner": {"rid": D_BTN, "rtype": "device"},
            "power_state": {"battery_level": 64, "battery_state": "low"},
        },
    ]


class FakeHue(hue.Hue):
    """Hue instance with isolated config dir and captured snapshots/API calls."""

    def __init__(self, resources=None):
        super().__init__()
        self.emitted = []
        self.api_calls = []
        self._resources = resources if resources is not None else build_pro_resources()
        self.config = {
            "ip": "192.168.1.50",
            "bridge_id": BRIDGE_ID,
            "fingerprint": "a" * 64,
            "app_key": "test-secret-app-key",
            "name": "Hue Bridge",
            "model": "",
        }
        self.phase = "ok"

    def emit(self):
        self.emitted.append(self.snapshot())

    def api(self, method, path, body=None, config=None):
        self.api_calls.append((method, path, body))
        if method == "GET" and path == "/clip/v2/resource":
            return {"data": self._resources}
        return {"data": []}

    def start_discovery(self):
        pass


class BridgeProAndParsingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self._orig_cfg_dir = hue.CONFIG_DIR
        self._orig_cfg_path = hue.CONFIG_PATH
        hue.CONFIG_DIR = self.tmp.name
        hue.CONFIG_PATH = os.path.join(self.tmp.name, "config.json")

    def tearDown(self):
        hue.CONFIG_DIR = self._orig_cfg_dir
        hue.CONFIG_PATH = self._orig_cfg_path
        self.tmp.cleanup()

    def test_bridge_pro_detection_bsb003_and_v2_fallback(self):
        h = FakeHue(build_pro_resources(model_id="BSB003"))
        h.refresh()
        # Zero extra HTTP calls: exactly one GET /clip/v2/resource
        self.assertEqual(len(h.api_calls), 1)
        self.assertEqual(h.api_calls[0][:2], ("GET", "/clip/v2/resource"))

        snap = h.snapshot()
        self.assertEqual(snap["bridge"]["model_id"], "BSB003")
        self.assertTrue(snap["bridge"]["is_pro"])

        # Bridge v2 (BSB002) with no motion_areas -> is_pro is False
        res_v2 = [
            r for r in build_pro_resources(model_id="BSB002")
            if r.get("type") not in ("motion_area_configuration", "convenience_area_motion", "security_area_motion")
        ]
        h2 = FakeHue(res_v2)
        h2.refresh()
        snap2 = h2.snapshot()
        self.assertEqual(snap2["bridge"]["model_id"], "BSB002")
        self.assertFalse(snap2["bridge"]["is_pro"])

        # Even if model_id is unknown/BSB002, presence of motion_areas marks is_pro True
        h3 = FakeHue(build_pro_resources(model_id="BSB002"))
        h3.refresh()
        snap3 = h3.snapshot()
        self.assertEqual(snap3["bridge"]["model_id"], "BSB002")
        self.assertTrue(snap3["bridge"]["is_pro"])

        # Model detection via BSB prefix even without bridge service
        only_bsb = [{"id": D_BRIDGE, "type": "device", "product_data": {"model_id": "BSB003"}}]
        self.assertEqual(hue.extract_bridge_model_id(only_bsb), "BSB003")
        self.assertEqual(hue.extract_bridge_model_id([]), "")

    def test_motion_areas_parsing_and_light_annotations(self):
        h = FakeHue(build_pro_resources(area1_motion=True, area1_enabled=True))
        h.refresh()
        snap = h.snapshot()
        areas = {a["id"]: a for a in snap["motion_areas"]}
        self.assertEqual(set(areas.keys()), {MAC1, MAC2})

        a1 = areas[MAC1]
        self.assertEqual(a1["name"], "Living Room Motion")
        self.assertTrue(a1["enabled"])
        self.assertEqual(a1["room_id"], ROOM1)
        self.assertTrue(a1["motion"])
        self.assertTrue(a1["motion_valid"])
        self.assertEqual(a1["sensitivity"], 3)
        self.assertEqual(a1["sensitivity_max"], 4)
        self.assertEqual(a1["convenience_id"], CONV1)
        self.assertEqual(a1["light_ids"], [L1, L2])
        self.assertEqual(a1["health"], "healthy")

        # Fallback to security_area_motion + motion_valid=False uses motion.motion + degraded health
        a2 = areas[MAC2]
        self.assertEqual(a2["name"], "Office Security")
        self.assertFalse(a2["enabled"])
        self.assertEqual(a2["room_id"], ROOM2)
        self.assertTrue(a2["motion"])
        self.assertFalse(a2["motion_valid"])
        self.assertEqual(a2["convenience_id"], SEC2)
        self.assertEqual(a2["light_ids"], [L3])
        self.assertEqual(a2["health"], "degraded")

        # Per-light annotations: L1 & L2 in enabled active MAC1 -> both True; L3 in disabled MAC2 -> both False
        lights = {l["id"]: l for r in snap["rooms"] for l in r["lights"]}
        self.assertTrue(lights[L1]["motion_aware"])
        self.assertTrue(lights[L1]["motion_active"])
        self.assertTrue(lights[L2]["motion_aware"])
        self.assertTrue(lights[L2]["motion_active"])
        self.assertFalse(lights[L3]["motion_aware"])
        self.assertFalse(lights[L3]["motion_active"])

    def test_physical_sensors_parsing(self):
        h = FakeHue(build_pro_resources())
        h.refresh()
        sensors = {s["id"]: s for s in h.snapshot()["sensors"]}
        self.assertEqual(set(sensors.keys()), {S_MOT, S_CON, D_BTN})

        mot = sensors[S_MOT]
        self.assertEqual(mot["kind"], "motion")
        self.assertEqual(mot["device_id"], D_MOT)
        self.assertEqual(mot["name"], "Hallway Motion")
        self.assertEqual(mot["room_id"], ROOM1)
        self.assertTrue(mot["enabled"])
        self.assertFalse(mot["motion"])
        self.assertFalse(mot["open"])
        self.assertEqual(mot["temperature"], 21.4)
        self.assertEqual(mot["lux"], 42.0)
        self.assertEqual(mot["battery"], 88)
        self.assertEqual(mot["battery_state"], "normal")
        self.assertEqual(mot["sensitivity"], 2)
        self.assertEqual(mot["sensitivity_max"], 4)
        self.assertEqual(mot["last_event"], "")

        con = sensors[S_CON]
        self.assertEqual(con["kind"], "contact")
        self.assertEqual(con["device_id"], D_CON)
        self.assertEqual(con["name"], "Office Window")
        self.assertEqual(con["room_id"], ROOM2)
        self.assertTrue(con["enabled"])
        self.assertTrue(con["open"])
        self.assertFalse(con["motion"])
        self.assertEqual(con["battery"], 95)
        self.assertEqual(con["battery_state"], "normal")
        self.assertIsNone(con["temperature"])
        self.assertIsNone(con["lux"])
        self.assertIsNone(con["sensitivity"])
        self.assertIsNone(con["sensitivity_max"])

        btn = sensors[D_BTN]
        self.assertEqual(btn["kind"], "button")
        self.assertEqual(btn["device_id"], D_BTN)
        self.assertEqual(btn["name"], "Dimmer Switch")
        self.assertEqual(btn["room_id"], ROOM1)
        self.assertTrue(btn["enabled"])
        self.assertEqual(btn["battery"], 64)
        self.assertEqual(btn["battery_state"], "low")
        self.assertEqual(btn["button_ids"], [S_BTN1, S_BTN2])
        self.assertEqual(btn["last_event"], "short_release")


class SseAndRulesTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self._orig_cfg_dir = hue.CONFIG_DIR
        self._orig_cfg_path = hue.CONFIG_PATH
        hue.CONFIG_DIR = self.tmp.name
        hue.CONFIG_PATH = os.path.join(self.tmp.name, "config.json")

    def tearDown(self):
        hue.CONFIG_DIR = self._orig_cfg_dir
        hue.CONFIG_PATH = self._orig_cfg_path
        self.tmp.cleanup()

    def _rule(self, **kw):
        base = {
            "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaa001",
            "name": "Test Rule",
            "enabled": True,
            "kind": "follow",
            "trigger_kind": "motion",
            "trigger_id": S_MOT,
            "trigger_state": "motion",
            "targets": [{"kind": "light", "id": L2, "action": "on", "bri": 80, "scene_id": ""}],
            "mirror_off": False,
            "match_bri": False,
            "only_if_off": False,
            "time_start": "",
            "time_end": "",
        }
        base.update(kw)
        return base

    def test_validate_rule_and_cycle_detection_for_sensors(self):
        valid_cases = [
            self._rule(trigger_kind="motion", trigger_id=S_MOT, trigger_state="motion"),
            self._rule(trigger_kind="motion", trigger_id=S_MOT, trigger_state="clear"),
            self._rule(trigger_kind="motion_area", trigger_id=MAC1, trigger_state="motion"),
            self._rule(trigger_kind="motion_area", trigger_id=MAC1, trigger_state="clear"),
            self._rule(trigger_kind="contact", trigger_id=S_CON, trigger_state="open"),
            self._rule(trigger_kind="contact", trigger_id=S_CON, trigger_state="closed"),
            self._rule(trigger_kind="button", trigger_id=D_BTN, trigger_state="press"),
            self._rule(trigger_kind="button", trigger_id=D_BTN, trigger_state="long_press"),
            self._rule(kind="sensor", trigger_kind="motion", trigger_id=S_MOT, trigger_state="motion"),
        ]
        for case in valid_cases:
            res = hue.validate_rule(case)
            self.assertIsNotNone(res, case)
        # No false cycles for sensor rules
        self.assertEqual(hue.detect_rule_cycles([hue.validate_rule(c) for c in valid_cases]), "")

        invalid_cases = [
            self._rule(trigger_kind="motion", trigger_id=S_MOT, trigger_state="on"),
            self._rule(trigger_kind="motion", trigger_id="bad-uuid", trigger_state="motion"),
            self._rule(trigger_kind="motion_area", trigger_id=MAC1, trigger_state="open"),
            self._rule(trigger_kind="contact", trigger_id=S_CON, trigger_state="motion"),
            self._rule(trigger_kind="button", trigger_id=D_BTN, trigger_state="short_release"),
        ]
        for case in invalid_cases:
            self.assertIsNone(hue.validate_rule(case), case)

    def test_sse_sensor_updates_and_rule_firing(self):
        h = FakeHue(build_pro_resources())
        h.refresh()

        # Save rules for motion (with mirror_off), contact (with mirror_off), button press & long_press
        h.handle({"cmd": "rule_save", "rule": self._rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaa001",
            trigger_kind="motion", trigger_id=S_MOT, trigger_state="motion",
            mirror_off=True,
            targets=[{"kind": "light", "id": L2, "action": "on", "bri": 75, "scene_id": ""}],
        )})
        h.handle({"cmd": "rule_save", "rule": self._rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaa002",
            trigger_kind="contact", trigger_id=S_CON, trigger_state="open",
            mirror_off=True,
            targets=[{"kind": "light", "id": L3, "action": "on", "bri": None, "scene_id": ""}],
        )})
        h.handle({"cmd": "rule_save", "rule": self._rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaa003",
            trigger_kind="button", trigger_id=D_BTN, trigger_state="press",
            targets=[{"kind": "light", "id": L2, "action": "on", "bri": 50, "scene_id": ""}],
        )})
        h.handle({"cmd": "rule_save", "rule": self._rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaa004",
            trigger_kind="button", trigger_id=D_BTN, trigger_state="long_press",
            targets=[{"kind": "light", "id": L1, "action": "off", "bri": None, "scene_id": ""}],
        )})
        self.assertEqual(len(h.rules), 4)

        # 1. Motion sensor triggers ON at t=100
        line_mot_on = "data: " + json.dumps([{
            "type": "update",
            "data": [{
                "type": "motion",
                "id": S_MOT,
                "motion": {"motion_valid": True, "motion_report": {"motion": True}},
                "sensitivity": {"sensitivity": 4},
            }],
        }])
        self.assertTrue(h.handle_sse_line(line_mot_on, now_mono=100.0))
        mot = next(s for s in h.sensors if s["id"] == S_MOT)
        self.assertTrue(mot["motion"])
        self.assertEqual(mot["sensitivity"], 4)
        self.assertEqual(h.pending.get(("light", L2)), {"on": {"on": True}, "dimming": {"brightness": 75.0}})

        # 2. Motion clears at t=110 -> mirror_off turns L2 OFF
        h.pending.clear()
        line_mot_off = "data: " + json.dumps([{
            "type": "update",
            "data": [{
                "type": "motion",
                "id": S_MOT,
                "motion": {"motion": False, "motion_valid": False},
            }],
        }])
        self.assertTrue(h.handle_sse_line(line_mot_off, now_mono=110.0))
        self.assertEqual(h.pending.get(("light", L2)), {"on": {"on": False}})

        # 3. Contact sensor closes (was open) at t=120 -> mirror_off turns L3 OFF
        h.pending.clear()
        line_con_close = "data: " + json.dumps([{
            "type": "update",
            "data": [{
                "type": "contact",
                "id": S_CON,
                "contact_report": {"state": "contact"},
            }],
        }])
        self.assertTrue(h.handle_sse_line(line_con_close, now_mono=120.0))
        con = next(s for s in h.sensors if s["id"] == S_CON)
        self.assertFalse(con["open"])
        self.assertEqual(h.pending.get(("light", L3)), {"on": {"on": False}})

        # 4. Contact sensor opens at t=130 -> triggers L3 ON
        h.pending.clear()
        line_con_open = "data: " + json.dumps([{
            "type": "update",
            "data": [{
                "type": "contact",
                "id": S_CON,
                "contact_report": {"state": "no_contact"},
            }],
        }])
        self.assertTrue(h.handle_sse_line(line_con_open, now_mono=130.0))
        self.assertTrue(con["open"])
        self.assertEqual(h.pending.get(("light", L3)), {"on": {"on": True}})

        # 5. Button short_release -> normalized to "press" -> L2 ON at 50%
        h.pending.clear()
        line_btn_press = "data: " + json.dumps([{
            "type": "update",
            "data": [{
                "type": "button",
                "id": S_BTN1,
                "button": {"button_report": {"event": "short_release"}},
            }],
        }])
        self.assertTrue(h.handle_sse_line(line_btn_press, now_mono=140.0))
        btn = next(s for s in h.sensors if s["id"] == D_BTN)
        self.assertEqual(btn["last_event"], "short_release")
        self.assertEqual(h.pending.get(("light", L2)), {"on": {"on": True}, "dimming": {"brightness": 50.0}})

        # 6. Button long_release -> normalized to "long_press" -> L1 OFF
        h.pending.clear()
        line_btn_long = "data: " + json.dumps([{
            "type": "update",
            "data": [{
                "type": "button",
                "id": S_BTN2,
                "button": {"button_report": {"event": "long_release"}},
            }],
        }])
        self.assertTrue(h.handle_sse_line(line_btn_long, now_mono=150.0))
        self.assertEqual(btn["last_event"], "long_release")
        self.assertEqual(h.pending.get(("light", L1)), {"on": {"on": False}})

        # 7. Companion SSE updates (temperature, light_level, device_power) WITHOUT owner field
        line_companion = "data: " + json.dumps([{
            "type": "update",
            "data": [
                {"type": "temperature", "id": S_TEMP, "temperature": {"temperature_valid": True, "temperature_report": {"temperature": 23.66}}},
                {"type": "light_level", "id": S_LUX, "light": {"light_level_valid": True, "light_level_report": {"light_level": 20001}}},
                {"type": "device_power", "id": S_POW_MOT, "power_state": {"battery_level": 72, "battery_state": "normal"}},
            ],
        }])
        self.assertTrue(h.handle_sse_line(line_companion, now_mono=160.0))
        self.assertEqual(mot["temperature"], 23.7)
        self.assertEqual(mot["lux"], 100.0)
        self.assertEqual(mot["battery"], 72)

    def test_sse_motion_area_updates_light_flags_and_rules(self):
        h = FakeHue(build_pro_resources(area1_motion=False, area1_enabled=True))
        h.refresh()

        h.handle({"cmd": "rule_save", "rule": self._rule(
            id="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaa010",
            trigger_kind="motion_area", trigger_id=MAC1, trigger_state="motion",
            mirror_off=True,
            targets=[{"kind": "light", "id": L3, "action": "on", "bri": 90, "scene_id": ""}],
        )})

        lights = lambda: {l["id"]: l for r in h.rooms for l in r["lights"]}
        self.assertTrue(lights()[L1]["motion_aware"])
        self.assertFalse(lights()[L1]["motion_active"])

        # Motion detected on convenience_area_motion -> light motion_active flips True + rule fires
        line_on = "data: " + json.dumps([{
            "type": "update",
            "data": [{
                "type": "convenience_area_motion",
                "id": CONV1,
                "motion": {"motion_valid": True, "motion_report": {"motion": True}},
                "sensitivity": {"sensitivity": 4},
            }],
        }])
        self.assertTrue(h.handle_sse_line(line_on, now_mono=200.0))
        a1 = next(a for a in h.motion_areas if a["id"] == MAC1)
        self.assertTrue(a1["motion"])
        self.assertEqual(a1["sensitivity"], 4)
        self.assertTrue(lights()[L1]["motion_active"])
        self.assertTrue(lights()[L2]["motion_active"])
        self.assertEqual(h.pending.get(("light", L3)), {"on": {"on": True}, "dimming": {"brightness": 90.0}})

        # Motion clears -> light motion_active flips False + mirror_off fires
        h.pending.clear()
        line_off = "data: " + json.dumps([{
            "type": "update",
            "data": [{
                "type": "convenience_area_motion",
                "id": CONV1,
                "motion": {"motion_valid": True, "motion_report": {"motion": False}},
            }],
        }])
        self.assertTrue(h.handle_sse_line(line_off, now_mono=210.0))
        self.assertFalse(lights()[L1]["motion_active"])
        self.assertEqual(h.pending.get(("light", L3)), {"on": {"on": False}})

        # motion_area_configuration disables area and renames it -> motion_aware flips False
        line_cfg = "data: " + json.dumps([{
            "type": "update",
            "data": [{
                "type": "motion_area_configuration",
                "id": MAC1,
                "enabled": False,
                "name": "Lounge Motion",
            }],
        }])
        self.assertTrue(h.handle_sse_line(line_cfg, now_mono=220.0))
        self.assertFalse(a1["enabled"])
        self.assertEqual(a1["name"], "Lounge Motion")
        self.assertFalse(lights()[L1]["motion_aware"])
        self.assertFalse(lights()[L2]["motion_aware"])


class CommandsAndFloorplanTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self._orig_cfg_dir = hue.CONFIG_DIR
        self._orig_cfg_path = hue.CONFIG_PATH
        hue.CONFIG_DIR = self.tmp.name
        hue.CONFIG_PATH = os.path.join(self.tmp.name, "config.json")

    def tearDown(self):
        hue.CONFIG_DIR = self._orig_cfg_dir
        hue.CONFIG_PATH = self._orig_cfg_path
        self.tmp.cleanup()

    def test_sensor_toggle_command(self):
        h = FakeHue(build_pro_resources())
        h.refresh()
        h.api_queue.clear()
        h.emitted.clear()

        h.handle({"cmd": "sensor_toggle", "kind": "motion", "id": S_MOT, "enabled": False})
        self.assertEqual(h.api_queue, [("PUT", "/clip/v2/resource/motion/" + S_MOT, {"enabled": False})])
        mot = next(s for s in h.sensors if s["id"] == S_MOT)
        self.assertFalse(mot["enabled"])
        self.assertEqual(len(h.emitted), 1)

        h.api_queue.clear()
        h.handle({"cmd": "sensor_toggle", "kind": "contact", "id": S_CON, "enabled": False})
        self.assertEqual(h.api_queue, [("PUT", "/clip/v2/resource/contact/" + S_CON, {"enabled": False})])
        con = next(s for s in h.sensors if s["id"] == S_CON)
        self.assertFalse(con["enabled"])

        # Reject invalid kinds, unknown IDs, non-bool enabled
        h.api_queue.clear()
        h.handle({"cmd": "sensor_toggle", "kind": "button", "id": D_BTN, "enabled": False})
        h.handle({"cmd": "sensor_toggle", "kind": "motion", "id": L1, "enabled": False})
        h.handle({"cmd": "sensor_toggle", "kind": "motion", "id": S_MOT, "enabled": "no"})
        self.assertEqual(h.api_queue, [])

    def test_motion_area_update_and_unpair(self):
        h = FakeHue(build_pro_resources(area1_motion=True, area1_enabled=True))
        h.refresh()
        h.api_queue.clear()
        h.emitted.clear()

        # Update both enabled and sensitivity in one call
        h.handle({"cmd": "motion_area_update", "id": MAC1, "enabled": False, "sensitivity": 1})
        self.assertEqual(h.api_queue, [
            ("PUT", "/clip/v2/resource/motion_area_configuration/" + MAC1, {"enabled": False}),
            ("PUT", "/clip/v2/resource/convenience_area_motion/" + CONV1, {"sensitivity": {"sensitivity": 1}}),
        ])
        a1 = next(a for a in h.motion_areas if a["id"] == MAC1)
        self.assertFalse(a1["enabled"])
        self.assertEqual(a1["sensitivity"], 1)
        # Per-light flags updated optimistically
        lights = {l["id"]: l for r in h.rooms for l in r["lights"]}
        self.assertFalse(lights[L1]["motion_aware"])
        self.assertFalse(lights[L1]["motion_active"])

        # Reject out-of-range sensitivity, bool sensitivity, unknown area id
        h.api_queue.clear()
        h.handle({"cmd": "motion_area_update", "id": MAC1, "sensitivity": 99})
        h.handle({"cmd": "motion_area_update", "id": MAC1, "sensitivity": -1})
        h.handle({"cmd": "motion_area_update", "id": MAC1, "sensitivity": True})
        h.handle({"cmd": "motion_area_update", "id": L1, "enabled": True})
        self.assertEqual(h.api_queue, [])

        # Unpair clears sensors, motion_areas, and bridge_model_id
        h.handle({"cmd": "unpair"})
        self.assertEqual(h.sensors, [])
        self.assertEqual(h.motion_areas, [])
        self.assertEqual(h.bridge_model_id, "")
        snap = h.snapshot()
        self.assertEqual(snap["sensors"], [])
        self.assertEqual(snap["motion_areas"], [])
        self.assertEqual(snap["bridge"], {})

    def test_floorplan_sensors_validation(self):
        rooms, _, _ = hue.build_model(build_pro_resources())
        fp = hue.build_floorplan_template("auto_bridge", rooms, bridge_id=BRIDGE_ID)
        self.assertIsNotNone(fp)
        self.assertEqual(fp["sensors"], {})

        # Legacy floorplan without "sensors" key stays valid and defaults to {}
        legacy = dict(fp)
        del legacy["sensors"]
        validated_legacy = hue.validate_floorplan(legacy)
        self.assertIsNotNone(validated_legacy)
        self.assertEqual(validated_legacy["sensors"], {})

        # Valid sensor placements accepted and rounded
        with_sensors = dict(fp, sensors={S_MOT: {"x": 123.456, "y": 456.789}, S_CON: {"x": 0, "y": 3000}})
        validated = hue.validate_floorplan(with_sensors)
        self.assertIsNotNone(validated)
        self.assertEqual(validated["sensors"], {S_MOT: {"x": 123.5, "y": 456.8}, S_CON: {"x": 0.0, "y": 3000.0}})

        # Out-of-range finite coordinates clamp to [0..4000] x [0..3000] (matching lamps)
        clamped = hue.validate_floorplan(dict(fp, sensors={S_MOT: {"x": -1, "y": 3001}, S_CON: {"x": 4001, "y": -5}}))
        self.assertIsNotNone(clamped)
        self.assertEqual(clamped["sensors"], {S_MOT: {"x": 0.0, "y": 3000.0}, S_CON: {"x": 4000.0, "y": 0.0}})

        # Hostile/invalid sensor entries rejected
        self.assertIsNone(hue.validate_floorplan(dict(fp, sensors="bad")))
        self.assertIsNone(hue.validate_floorplan(dict(fp, sensors={"not-a-uuid": {"x": 10, "y": 10}})))
        self.assertIsNone(hue.validate_floorplan(dict(fp, sensors={S_MOT: "bad"})))
        self.assertIsNone(hue.validate_floorplan(dict(fp, sensors={S_MOT: {"x": True, "y": 10}})))
        self.assertIsNone(hue.validate_floorplan(dict(fp, sensors={S_MOT: {"x": float("nan"), "y": 10}})))
        self.assertIsNone(hue.validate_floorplan(dict(fp, sensors={S_MOT: {"x": float("inf"), "y": 10}})))
        too_many = {
            "%08x-1111-1111-1111-111111111111" % i: {"x": 10.0, "y": 10.0}
            for i in range(129)
        }
        self.assertIsNone(hue.validate_floorplan(dict(fp, sensors=too_many)))

    def test_motion_area_candidates_count_and_eligible_rooms(self):
        # Default pro fixture has 3 candidates across 2 rooms (2 in Living Room, 1 in Office)
        h = FakeHue(build_pro_resources(model_id="BSB003"))
        h.refresh()
        snap = h.snapshot()
        self.assertEqual(snap["bridge"]["motion_area_candidates"], 3)
        self.assertEqual(snap["bridge"]["motion_area_candidate_rooms"], [])

        # Room with 3+ candidate devices is reported as eligible
        res = build_pro_resources(model_id="BSB003")
        for r in res:
            if r.get("id") == ROOM1:
                r["children"].append({"rid": D3, "rtype": "device"})
        h2 = FakeHue(res)
        h2.refresh()
        snap2 = h2.snapshot()
        self.assertEqual(snap2["bridge"]["motion_area_candidates"], 3)
        self.assertEqual(snap2["bridge"]["motion_area_candidate_rooms"], [ROOM1])

        # Unpair clears candidate count and rooms
        h2.handle({"cmd": "unpair"})
        self.assertEqual(h2.motion_area_candidates, 0)
        self.assertEqual(h2.motion_area_candidate_rooms, [])


if __name__ == "__main__":
    unittest.main()
