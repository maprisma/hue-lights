import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest


HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPTS_DIR = os.path.join(HERE, "..", "scripts")
MCP_SCRIPT = os.path.join(SCRIPTS_DIR, "hue-mcp.py")
HUE_SCRIPT = os.path.join(SCRIPTS_DIR, "hue-lights.py")

_hue_spec = importlib.util.spec_from_file_location("hue_lights_test", HUE_SCRIPT)
hue_mod = importlib.util.module_from_spec(_hue_spec)
_hue_spec.loader.exec_module(hue_mod)

_mcp_spec = importlib.util.spec_from_file_location("hue_mcp_test", MCP_SCRIPT)
mcp_mod = importlib.util.module_from_spec(_mcp_spec)
_mcp_spec.loader.exec_module(mcp_mod)


# Deterministic UUIDs for fixtures
L_SOFA = "11111111-1111-1111-1111-111111111111"
L_FLOOR = "22222222-2222-2222-2222-222222222222"
L_DESK = "33333333-3333-3333-3333-333333333333"
L_PLUG = "44444444-4444-4444-4444-444444444444"

D_SOFA = "aaaaaaaa-1111-1111-1111-111111111111"
D_FLOOR = "aaaaaaaa-2222-2222-2222-222222222222"
D_DESK = "aaaaaaaa-3333-3333-3333-333333333333"
D_PLUG = "aaaaaaaa-4444-4444-4444-444444444444"
D_BRIDGE = "aaaaaaaa-0000-0000-0000-000000000001"
SVC_BRIDGE = "bbbbbbbb-0000-0000-0000-000000000001"

GL_HOME = "bbbbbbbb-0000-0000-0000-000000000099"
GL_LIVING = "bbbbbbbb-1111-1111-1111-111111111111"
GL_ZONE = "bbbbbbbb-3333-3333-3333-333333333333"
BRIDGE_HOME = "bbbbbbbb-0000-0000-0000-000000000088"

ROOM_LIVING = "cccccccc-1111-1111-1111-111111111111"
ROOM_OFFICE = "cccccccc-2222-2222-2222-222222222222"
ZONE_DOWN = "cccccccc-3333-3333-3333-333333333333"

SCENE_RELAX_LIVING = "dddddddd-1111-1111-1111-111111111111"
SCENE_RELAX_OFFICE = "dddddddd-2222-2222-2222-222222222222"
SCENE_CONCENTRATE = "dddddddd-3333-3333-3333-333333333333"

CAND1 = "44444444-1111-1111-1111-111111111111"
CAND2 = "44444444-2222-2222-2222-222222222222"
MAC_LIVING = "55555555-1111-1111-1111-111111111111"
MAC_OFFICE = "55555555-2222-2222-2222-222222222222"
CONV_LIVING = "66666666-1111-1111-1111-111111111111"
CONV_OFFICE = "66666666-2222-2222-2222-222222222222"

D_MOT = "77777777-1111-1111-1111-111111111111"
S_MOT = "77777777-2222-2222-2222-222222222222"
S_TEMP = "77777777-3333-3333-3333-333333333333"
S_LUX = "77777777-4444-4444-4444-444444444444"
S_POW_MOT = "77777777-5555-5555-5555-555555555555"

D_CON = "88888888-1111-1111-1111-111111111111"
S_CON = "88888888-2222-2222-2222-222222222222"
S_POW_CON = "88888888-3333-3333-3333-333333333333"

D_BTN = "99999999-1111-1111-1111-111111111111"
S_BTN = "99999999-2222-2222-2222-222222222222"
S_POW_BTN = "99999999-3333-3333-3333-333333333333"

SCHED_BRIDGE = "eeeeeeee-1111-1111-1111-111111111111"
SCHED_LOCAL = "eeeeeeee-2222-2222-2222-222222222222"
RULE_EXISTING = "ffffffff-1111-1111-1111-111111111111"

SECRET_APP_KEY = "SUPER_SECRET_HUE_APP_KEY_9876543210"
SECRET_CLIENT_KEY = "DEADBEEFCAFEBABE1234567890ABCDEF"
SECRET_FINGERPRINT = "a" * 64
BRIDGE_ID = "001788FFFE112233"


def build_bridge_resources():
    return [
        {
            "id": D_BRIDGE,
            "type": "device",
            "product_data": {"model_id": "BSB003", "product_name": "Hue Bridge Pro"},
            "metadata": {"name": "Hue Bridge Pro"},
            "services": [{"rid": SVC_BRIDGE, "rtype": "bridge"}],
        },
        {
            "id": BRIDGE_HOME,
            "type": "bridge_home",
            "services": [{"rid": GL_HOME, "rtype": "grouped_light"}],
        },
        {"id": GL_HOME, "type": "grouped_light", "on": {"on": True}, "dimming": {"brightness": 70.0}},
        # Lights
        {
            "id": L_SOFA,
            "type": "light",
            "owner": {"rid": D_SOFA, "rtype": "device"},
            "metadata": {"name": "Sofa Lamp"},
            "on": {"on": True},
            "dimming": {"brightness": 75.0},
            "color_temperature": {"mirek": 320, "mirek_valid": True, "mirek_schema": {"mirek_minimum": 153, "mirek_maximum": 500}},
            "color": {"xy": {"x": 0.45, "y": 0.41}},
            "effects": {"status": "no_effect", "effect_values": ["no_effect", "candle", "fire", "prism"]},
        },
        {
            "id": L_FLOOR,
            "type": "light",
            "owner": {"rid": D_FLOOR, "rtype": "device"},
            "metadata": {"name": "Floor Lamp"},
            "on": {"on": False},
            "dimming": {"brightness": 50.0},
            "color_temperature": {"mirek": 370, "mirek_valid": True, "mirek_schema": {"mirek_minimum": 200, "mirek_maximum": 454}},
        },
        {
            "id": L_DESK,
            "type": "light",
            "owner": {"rid": D_DESK, "rtype": "device"},
            "metadata": {"name": "Desk Lamp"},
            "on": {"on": True},
            "dimming": {"brightness": 80.0},
            "color_temperature": {"mirek": 250, "mirek_valid": True, "mirek_schema": {"mirek_minimum": 153, "mirek_maximum": 500}},
            "color": {"xy": {"x": 0.31, "y": 0.33}},
            "effects": {"status": "no_effect", "effect_values": ["no_effect", "candle"]},
        },
        {
            "id": L_PLUG,
            "type": "light",
            "owner": {"rid": D_PLUG, "rtype": "device"},
            "metadata": {"name": "Hallway Plug"},
            "on": {"on": True},
        },
        {"id": D_SOFA, "type": "device", "services": [{"rid": L_SOFA, "rtype": "light"}]},
        {"id": D_FLOOR, "type": "device", "services": [{"rid": L_FLOOR, "rtype": "light"}]},
        {"id": D_DESK, "type": "device", "services": [{"rid": L_DESK, "rtype": "light"}]},
        {"id": D_PLUG, "type": "device", "services": [{"rid": L_PLUG, "rtype": "light"}]},
        # Rooms & Zones
        {"id": GL_LIVING, "type": "grouped_light", "on": {"on": True}, "dimming": {"brightness": 75.0}},
        {"id": GL_ZONE, "type": "grouped_light", "on": {"on": True}, "dimming": {"brightness": 75.0}},
        {
            "id": ROOM_LIVING,
            "type": "room",
            "metadata": {"name": "Living Room", "archetype": "living_room"},
            "children": [
                {"rid": D_SOFA, "rtype": "device"},
                {"rid": D_FLOOR, "rtype": "device"},
                {"rid": D_MOT, "rtype": "device"},
                {"rid": D_BTN, "rtype": "device"},
            ],
            "services": [{"rid": GL_LIVING, "rtype": "grouped_light"}],
        },
        {
            "id": ROOM_OFFICE,
            "type": "room",
            "metadata": {"name": "Office", "archetype": "office"},
            "children": [
                {"rid": D_DESK, "rtype": "device"},
                {"rid": D_CON, "rtype": "device"},
            ],
            "services": [],
        },
        {
            "id": ZONE_DOWN,
            "type": "zone",
            "metadata": {"name": "Downstairs", "archetype": "downstairs"},
            "children": [
                {"rid": L_SOFA, "rtype": "light"},
                {"rid": L_DESK, "rtype": "light"},
            ],
            "services": [{"rid": GL_ZONE, "rtype": "grouped_light"}],
        },
        # Scenes (including duplicate "Relax" across rooms!)
        {
            "id": SCENE_RELAX_LIVING,
            "type": "scene",
            "metadata": {"name": "Relax"},
            "group": {"rid": ROOM_LIVING, "rtype": "room"},
            "status": {"active": "static"},
        },
        {
            "id": SCENE_RELAX_OFFICE,
            "type": "scene",
            "metadata": {"name": "Relax"},
            "group": {"rid": ROOM_OFFICE, "rtype": "room"},
            "status": {"active": "inactive"},
        },
        {
            "id": SCENE_CONCENTRATE,
            "type": "scene",
            "metadata": {"name": "Concentrate"},
            "group": {"rid": ROOM_OFFICE, "rtype": "room"},
            "status": {"active": "inactive"},
        },
        # MotionAware (Bridge Pro)
        {"id": CAND1, "type": "motion_area_candidate", "owner": {"rid": D_SOFA, "rtype": "device"}},
        {"id": CAND2, "type": "motion_area_candidate", "owner": {"rid": D_FLOOR, "rtype": "device"}},
        {
            "id": MAC_LIVING,
            "type": "motion_area_configuration",
            "name": "Living Room Motion",
            "enabled": True,
            "group": {"rid": ROOM_LIVING, "rtype": "room"},
            "participants": [
                {"resource": {"rid": CAND1, "rtype": "motion_area_candidate"}, "status": {"health": "healthy"}},
                {"resource": {"rid": CAND2, "rtype": "motion_area_candidate"}, "status": {"health": "healthy"}},
            ],
        },
        {
            "id": CONV_LIVING,
            "type": "convenience_area_motion",
            "owner": {"rid": MAC_LIVING, "rtype": "motion_area_configuration"},
            "enabled": True,
            "motion": {"motion_valid": True, "motion_report": {"motion": True}},
            "sensitivity": {"sensitivity": 2, "sensitivity_max": 4},
        },
        {
            "id": MAC_OFFICE,
            "type": "motion_area_configuration",
            "name": "Office Motion",
            "enabled": False,
            "group": {"rid": ROOM_OFFICE, "rtype": "room"},
            "participants": [
                {"resource": {"rid": L_DESK, "rtype": "light"}, "status": {"health": "healthy"}},
            ],
        },
        {
            "id": CONV_OFFICE,
            "type": "convenience_area_motion",
            "owner": {"rid": MAC_OFFICE, "rtype": "motion_area_configuration"},
            "enabled": False,
            "motion": {"motion_valid": True, "motion_report": {"motion": False}},
            "sensitivity": {"sensitivity": 1, "sensitivity_max": 4},
        },
        # Physical sensors
        {"id": D_MOT, "type": "device", "metadata": {"name": "Hallway Motion"}},
        {
            "id": S_MOT,
            "type": "motion",
            "owner": {"rid": D_MOT, "rtype": "device"},
            "enabled": True,
            "motion": {"motion_valid": True, "motion_report": {"motion": True}},
            "sensitivity": {"sensitivity": 3, "sensitivity_max": 4},
        },
        {
            "id": S_TEMP,
            "type": "temperature",
            "owner": {"rid": D_MOT, "rtype": "device"},
            "temperature": {"temperature_valid": True, "temperature_report": {"temperature": 21.4}},
        },
        {
            "id": S_LUX,
            "type": "light_level",
            "owner": {"rid": D_MOT, "rtype": "device"},
            "light": {"light_level_valid": True, "light_level_report": {"light_level": 20001}},
        },
        {
            "id": S_POW_MOT,
            "type": "device_power",
            "owner": {"rid": D_MOT, "rtype": "device"},
            "power_state": {"battery_level": 88, "battery_state": "normal"},
        },
        {"id": D_CON, "type": "device", "metadata": {"name": "Front Door"}},
        {
            "id": S_CON,
            "type": "contact",
            "owner": {"rid": D_CON, "rtype": "device"},
            "enabled": True,
            "contact_report": {"state": "no_contact"},
        },
        {
            "id": S_POW_CON,
            "type": "device_power",
            "owner": {"rid": D_CON, "rtype": "device"},
            "power_state": {"battery_level": 95, "battery_state": "normal"},
        },
        {"id": D_BTN, "type": "device", "metadata": {"name": "Dimmer Switch"}},
        {
            "id": S_BTN,
            "type": "button",
            "owner": {"rid": D_BTN, "rtype": "device"},
            "button": {"button_report": {"event": "short_release", "updated": "2026-11-24T20:00:00Z"}},
        },
        {
            "id": S_POW_BTN,
            "type": "device_power",
            "owner": {"rid": D_BTN, "rtype": "device"},
            "power_state": {"battery_level": 72, "battery_state": "normal"},
        },
        # Bridge schedule
        {
            "id": SCHED_BRIDGE,
            "type": "behavior_instance",
            "enabled": True,
            "metadata": {"name": "Morning Wake"},
            "configuration": {
                "where": [{"group": {"rid": ROOM_LIVING, "rtype": "room"}}],
                "when": {"time_point": {"type": "time", "time": {"hour": 7, "minute": 30}}, "recurrence_days": ["monday", "tuesday"]},
                "what": [{"on": {"on": True}, "dimming": {"brightness": 60}}],
            },
        },
    ]


class FakeHue(hue_mod.Hue):
    """Testable Hue subclass that intercepts API calls without touching a real bridge."""

    def __init__(self, paired=True, resources=None):
        super().__init__()
        self.api_calls = []
        self.refresh_count = 0
        self.api_error = None
        self._resources = resources if resources is not None else build_bridge_resources()
        if paired:
            self.config = {
                "ip": "192.168.1.50",
                "bridge_id": BRIDGE_ID,
                "fingerprint": SECRET_FINGERPRINT,
                "app_key": SECRET_APP_KEY,
                "name": "Home Bridge Pro",
                "model": "BSB003",
            }
            self.secrets = {SECRET_APP_KEY, SECRET_CLIENT_KEY}
            self.phase = "connecting"
        else:
            self.config = {}
            self.secrets = set()
            self.phase = "unpaired"
        self.custom_themes = [
            {
                "id": "neon-mint",
                "name": "Neon Mint",
                "hex": "#00ffaa",
                "mirek": None,
                "bri": 77,
                "effect": "no_effect",
                "built_in": False,
            }
        ]
        self.local_schedules = [
            {
                "id": SCHED_LOCAL,
                "name": "Bedtime Local",
                "enabled": True,
                "target_kind": "room",
                "target_id": ROOM_OFFICE,
                "target_name": "Office",
                "trigger_kind": "time",
                "time": "23:00",
                "offset_min": 0,
                "days": ["mon", "tue", "wed", "thu", "fri"],
                "action": "off",
                "bri": None,
                "scene_id": "",
                "fade_min": 0,
                "source": "local",
            }
        ]
        self.rules = [
            {
                "id": RULE_EXISTING,
                "name": "Sofa Follows Desk",
                "enabled": True,
                "kind": "follow",
                "trigger_kind": "light",
                "trigger_id": L_DESK,
                "trigger_state": "on",
                "targets": [{"kind": "light", "id": L_SOFA, "action": "on", "bri": None, "scene_id": ""}],
                "mirror_off": False,
                "match_bri": False,
                "time_start": "",
                "time_end": "",
                "only_if_off": False,
            }
        ]
        self.rules_paused = False
        self.last_refresh = 0.0

    def api(self, method, path, body=None, config=None):
        if self.api_error is not None:
            raise self.api_error
        self.api_calls.append((method, path, body))
        if method == "GET" and path == "/clip/v2/resource":
            return {"data": self._resources, "errors": []}
        return {"data": [], "errors": []}

    def refresh(self):
        self.refresh_count += 1
        return super().refresh()


class _MCPTestBase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self._orig_cfg_dir = hue_mod.CONFIG_DIR
        self._orig_cfg_path = hue_mod.CONFIG_PATH
        hue_mod.CONFIG_DIR = os.path.join(self._tmp.name, "hue-lights")
        hue_mod.CONFIG_PATH = os.path.join(hue_mod.CONFIG_DIR, "config.json")
        self.fake = FakeHue(paired=True)
        self.server = mcp_mod.HueMCPServer(hue=self.fake, hue_module=hue_mod, stale_seconds=1.5, reload_local_files=False)

    def tearDown(self):
        hue_mod.CONFIG_DIR = self._orig_cfg_dir
        hue_mod.CONFIG_PATH = self._orig_cfg_path
        self._tmp.cleanup()

    def _call_tool(self, name, arguments=None, req_id=1):
        resp = self.server.handle_message({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments or {}},
        })
        self.assertIn("result", resp, resp)
        res = resp["result"]
        self.assertEqual(len(res["content"]), 1)
        self.assertEqual(res["content"][0]["type"], "text")
        data = json.loads(res["content"][0]["text"])
        return data, res["isError"], res["content"][0]["text"]


class ProtocolLifecycleTests(_MCPTestBase):
    def test_initialize_default_and_2025_version(self):
        r1 = self.server.handle_message({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05"},
        })
        self.assertEqual(r1["result"]["protocolVersion"], "2024-11-05")
        self.assertEqual(r1["result"]["serverInfo"], {"name": "maprisma-hue-lights", "version": "0.5.1"})
        self.assertIn("tools", r1["result"]["capabilities"])
        self.assertIn("resources", r1["result"]["capabilities"])

        r2 = self.server.handle_message({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "initialize",
            "params": {"protocolVersion": "2025-03-26"},
        })
        self.assertEqual(r2["result"]["protocolVersion"], "2025-03-26")

        r3 = self.server.handle_message({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "initialize",
            "params": {"protocolVersion": "1999-01-01"},
        })
        self.assertEqual(r3["result"]["protocolVersion"], "2024-11-05")

    def test_notifications_initialized_and_ping(self):
        self.assertIsNone(self.server.handle_message({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }))
        ping = self.server.handle_message({"jsonrpc": "2.0", "id": "p1", "method": "ping"})
        self.assertEqual(ping, {"jsonrpc": "2.0", "id": "p1", "result": {}})

    def test_tools_list_complete_and_valid_schemas(self):
        resp = self.server.handle_message({"jsonrpc": "2.0", "id": 10, "method": "tools/list"})
        tools = resp["result"]["tools"]
        names = {t["name"] for t in tools}
        expected = {
            "hue_status", "hue_list_rooms", "hue_list_lights", "hue_list_sensors",
            "hue_list_themes", "hue_list_rules", "hue_list_schedules", "hue_get_floorplan",
            "hue_set_light", "hue_set_room", "hue_recall_scene", "hue_apply_theme",
            "hue_all_on", "hue_all_off", "hue_set_sensor", "hue_set_motion_area",
            "hue_create_rule", "hue_toggle_rule", "hue_delete_rule",
        }
        self.assertEqual(names, expected)
        for tool in tools:
            self.assertTrue(tool["description"])
            self.assertEqual(tool["inputSchema"]["type"], "object")
            self.assertIn("properties", tool["inputSchema"])

    def test_resources_list_and_read_and_unknown_uri(self):
        rl = self.server.handle_message({"jsonrpc": "2.0", "id": 11, "method": "resources/list"})
        resources = rl["result"]["resources"]
        self.assertEqual(len(resources), 1)
        self.assertEqual(resources[0]["uri"], "hue://state")
        self.assertEqual(resources[0]["mimeType"], "application/json")

        rr = self.server.handle_message({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "resources/read",
            "params": {"uri": "hue://state"},
        })
        contents = rr["result"]["contents"]
        self.assertEqual(len(contents), 1)
        raw_text = contents[0]["text"]
        self.assertNotIn(SECRET_APP_KEY, raw_text)
        self.assertNotIn(SECRET_CLIENT_KEY, raw_text)
        self.assertNotIn(SECRET_FINGERPRINT, raw_text)
        state = json.loads(raw_text)
        self.assertEqual(state["phase"], "ok")
        self.assertTrue(state["bridge"]["is_pro"])
        self.assertEqual(state["light_count"], 4)
        self.assertEqual(len(state["sensors"]), 3)
        self.assertEqual(len(state["motion_areas"]), 2)

        bad_rr = self.server.handle_message({
            "jsonrpc": "2.0",
            "id": 13,
            "method": "resources/read",
            "params": {"uri": "hue://unknown"},
        })
        self.assertEqual(bad_rr["error"]["code"], -32002)

    def test_stdio_errors_malformed_unknown_oversized_and_recovery(self):
        oversized = b'{"jsonrpc":"2.0","id":9,"method":"ping","x":"' + (b"a" * (mcp_mod.MAX_LINE_BYTES + 64)) + b'"}\n'
        deep = b'{"a":' * 40 + b'1' + b'}' * 40 + b'\n'
        stream = io.BytesIO(
            b"not-json\n"
            + b"\n"
            + deep
            + oversized
            + b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n'
            + b'{"jsonrpc":"2.0","id":1,"method":"unknown/method"}\n'
            + b'{"jsonrpc":"2.0","id":true,"method":"ping"}\n'
            + b'{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"does_not_exist"}}\n'
            + b'{"jsonrpc":"2.0","id":3,"method":"ping"}\n'
        )
        out = io.BytesIO()
        self.server.serve_stdio(stdin=stream, stdout=out)
        lines = [json.loads(line.decode("utf-8")) for line in out.getvalue().splitlines() if line.strip()]
        self.assertEqual(len(lines), 7)
        self.assertEqual(lines[0]["error"]["code"], -32700)  # malformed JSON
        self.assertEqual(lines[1]["error"]["code"], -32700)  # nesting too deep
        self.assertEqual(lines[2]["error"]["code"], -32600)  # oversized line
        self.assertEqual(lines[3]["error"]["code"], -32601)  # unknown method
        self.assertEqual(lines[4]["error"]["code"], -32600)  # bool id
        self.assertEqual(lines[5]["error"]["code"], -32602)  # unknown tool
        self.assertEqual(lines[6], {"jsonrpc": "2.0", "id": 3, "result": {}})  # recovered cleanly!

    def test_subprocess_isolated_mode_and_cwd_independence(self):
        self.assertTrue(os.access(MCP_SCRIPT, os.X_OK), "scripts/hue-mcp.py must be executable")
        env = dict(os.environ)
        env["XDG_CONFIG_HOME"] = self._tmp.name
        payload = (
            json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}) + "\n"
            + json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"
            + json.dumps({"jsonrpc": "2.0", "id": 2, "method": "ping"}) + "\n"
            + json.dumps({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "hue_status", "arguments": {}}}) + "\n"
        )
        proc = subprocess.run(
            [sys.executable, "-I", MCP_SCRIPT],
            input=payload,
            text=True,
            capture_output=True,
            cwd="/tmp",
            env=env,
            timeout=10,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out_lines = [json.loads(l) for l in proc.stdout.splitlines() if l.strip()]
        self.assertEqual(len(out_lines), 3)
        self.assertEqual(out_lines[0]["result"]["protocolVersion"], "2025-03-26")
        self.assertEqual(out_lines[1]["result"], {})
        status = json.loads(out_lines[2]["result"]["content"][0]["text"])
        self.assertEqual(status["phase"], "unpaired")
        self.assertFalse(status["paired"])


class ReadToolsAndSecurityTests(_MCPTestBase):
    def test_all_read_tools_and_no_secret_leakage(self):
        # 1. hue_status
        status, is_err, raw = self._call_tool("hue_status")
        self.assertFalse(is_err)
        self.assertEqual(status["phase"], "ok")
        self.assertEqual(status["bridge"]["name"], "Home Bridge Pro")
        self.assertEqual(status["bridge"]["ip"], "192.168.1.50")
        self.assertEqual(status["bridge"]["model_id"], "BSB003")
        self.assertTrue(status["bridge"]["is_pro"])
        self.assertEqual(status["light_count"], 4)
        self.assertEqual(status["on_count"], 3)
        self.assertEqual(status["room_count"], 3)  # 2 rooms + 1 zone
        self.assertEqual(status["sensor_count"], 3)
        self.assertEqual(status["motion_area_count"], 2)
        self.assertFalse(status["rules_paused"])
        self.assertEqual(self.fake.refresh_count, 1)

        # Fresh cache avoids redundant refresh; refresh=True forces it
        self._call_tool("hue_status")
        self.assertEqual(self.fake.refresh_count, 1)
        self._call_tool("hue_status", {"refresh": True})
        self.assertEqual(self.fake.refresh_count, 2)
        self.fake.last_refresh = time.time() - 5.0
        self._call_tool("hue_status")
        self.assertEqual(self.fake.refresh_count, 3)

        # 2. hue_list_rooms
        rooms_data, is_err, raw_rooms = self._call_tool("hue_list_rooms")
        self.assertFalse(is_err)
        by_name = {r["name"]: r for r in rooms_data["rooms"]}
        self.assertIn("Living Room", by_name)
        self.assertIn("Office", by_name)
        self.assertIn("Downstairs", by_name)
        self.assertIn("Other lights", by_name)
        self.assertEqual(by_name["Living Room"]["motion_area"]["name"], "Living Room Motion")
        self.assertTrue(by_name["Living Room"]["motion_area"]["motion"])
        self.assertEqual(len(by_name["Living Room"]["scenes"]), 1)

        zones_only, is_err, _ = self._call_tool("hue_list_rooms", {"kind": "zone"})
        self.assertFalse(is_err)
        self.assertEqual([r["name"] for r in zones_only["rooms"]], ["Downstairs"])

        # 3. hue_list_lights (dedup + room filter + only_on)
        all_lights, is_err, raw_lights = self._call_tool("hue_list_lights")
        self.assertFalse(is_err)
        self.assertEqual(len(all_lights["lights"]), 4)
        lmap = {l["name"]: l for l in all_lights["lights"]}
        self.assertTrue(lmap["Sofa Lamp"]["motion_aware"])
        self.assertTrue(lmap["Sofa Lamp"]["motion_active"])
        self.assertFalse(lmap["Desk Lamp"]["motion_aware"])

        living_lights, is_err, _ = self._call_tool("hue_list_lights", {"room": "living room"})
        self.assertFalse(is_err)
        self.assertEqual({l["name"] for l in living_lights["lights"]}, {"Sofa Lamp", "Floor Lamp"})

        living_on, is_err, _ = self._call_tool("hue_list_lights", {"room": ROOM_LIVING, "only_on": True})
        self.assertFalse(is_err)
        self.assertEqual([l["name"] for l in living_on["lights"]], ["Sofa Lamp"])

        other_lights, is_err, _ = self._call_tool("hue_list_lights", {"room": "Other lights"})
        self.assertFalse(is_err)
        self.assertEqual([l["name"] for l in other_lights["lights"]], ["Hallway Plug"])

        # 4. hue_list_sensors
        sensors_data, is_err, raw_sensors = self._call_tool("hue_list_sensors")
        self.assertFalse(is_err)
        smap = {s["name"]: s for s in sensors_data["sensors"]}
        self.assertEqual(smap["Hallway Motion"]["temperature"], 21.4)
        self.assertEqual(smap["Hallway Motion"]["battery"], 88)
        self.assertEqual(smap["Hallway Motion"]["room"], "Living Room")
        self.assertTrue(smap["Front Door"]["open"])
        self.assertEqual(smap["Dimmer Switch"]["last_event"], "short_release")
        self.assertEqual(len(sensors_data["motion_areas"]), 2)

        only_ma, is_err, _ = self._call_tool("hue_list_sensors", {"kind": "motion_area"})
        self.assertFalse(is_err)
        self.assertEqual(only_ma["sensors"], [])
        self.assertEqual(len(only_ma["motion_areas"]), 2)

        # 5. hue_list_themes
        themes_data, is_err, raw_themes = self._call_tool("hue_list_themes")
        self.assertFalse(is_err)
        tnames = {t["name"] for t in themes_data["themes"]}
        self.assertIn("Cyberpunk", tnames)
        self.assertIn("Reading", tnames)
        self.assertIn("Neon Mint", tnames)

        # 6. hue_list_rules
        rules_data, is_err, raw_rules = self._call_tool("hue_list_rules")
        self.assertFalse(is_err)
        self.assertFalse(rules_data["rules_paused"])
        self.assertEqual(len(rules_data["rules"]), 1)
        self.assertEqual(rules_data["rules"][0]["id"], RULE_EXISTING)

        # 7. hue_list_schedules
        scheds_data, is_err, raw_scheds = self._call_tool("hue_list_schedules")
        self.assertFalse(is_err)
        sched_names = {s["name"] for s in scheds_data["schedules"]}
        self.assertEqual(sched_names, {"Morning Wake", "Bedtime Local"})

        # 8. hue_get_floorplan
        fp_data, is_err, raw_fp = self._call_tool("hue_get_floorplan")
        self.assertFalse(is_err)
        fp = fp_data["floorplan"]
        self.assertEqual(fp["template_id"], "auto_bridge")
        self.assertIn("boxes", fp)
        self.assertIn("lamps", fp)
        self.assertIn("sensors", fp)

        # Verify app_key / client_key / fingerprint NEVER appear in any tool output
        for blob in (raw, raw_rooms, raw_lights, raw_sensors, raw_themes, raw_rules, raw_scheds, raw_fp):
            self.assertNotIn(SECRET_APP_KEY, blob)
            self.assertNotIn(SECRET_CLIENT_KEY, blob)
            self.assertNotIn(SECRET_FINGERPRINT, blob)
            self.assertNotIn("app_key", blob)

    def test_error_redacts_secrets(self):
        self.fake.refresh()
        self.fake.api_error = OSError("connection refused using key " + SECRET_APP_KEY + " and " + SECRET_CLIENT_KEY)
        data, is_err, raw = self._call_tool("hue_set_light", {"target": "Sofa Lamp", "on": False})
        self.assertTrue(is_err)
        self.assertNotIn(SECRET_APP_KEY, raw)
        self.assertNotIn(SECRET_CLIENT_KEY, raw)
        self.assertIn("***", data["error"])

    def test_unpaired_bridge_rejected_for_controls(self):
        unpaired = FakeHue(paired=False)
        srv = mcp_mod.HueMCPServer(hue=unpaired, hue_module=hue_mod, reload_local_files=False)
        resp = srv.handle_message({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "hue_set_light", "arguments": {"target": "Sofa Lamp", "on": True}},
        })
        self.assertTrue(resp["result"]["isError"])
        self.assertIn("not paired", resp["result"]["content"][0]["text"].lower())


class ControlToolsTests(_MCPTestBase):
    def setUp(self):
        super().setUp()
        self.fake.refresh()
        self.fake.api_calls.clear()

    def test_set_light_by_name_and_uuid_hex_kelvin_bri_effect(self):
        # By case-insensitive name with color_hex + brightness + effect
        data, is_err, _ = self._call_tool("hue_set_light", {
            "target": "sofa lamp",
            "brightness": 85,
            "color_hex": "#ff2e93",
            "effect": "candle",
        })
        self.assertFalse(is_err, data)
        self.assertEqual(len(self.fake.api_calls), 1)
        method, path, body = self.fake.api_calls[0]
        self.assertEqual((method, path), ("PUT", "/clip/v2/resource/light/" + L_SOFA))
        self.assertEqual(body["on"], {"on": True})
        self.assertEqual(body["dimming"], {"brightness": 85.0})
        self.assertEqual(body["effects"], {"effect": "candle"})
        self.assertEqual(body["color"]["xy"], {
            "x": hue_mod.hex_to_xy("#ff2e93")[0],
            "y": hue_mod.hex_to_xy("#ff2e93")[1],
        })

        # By UUID with kelvin clamping to Floor Lamp's [200..454]
        self.fake.api_calls.clear()
        data2, is_err, _ = self._call_tool("hue_set_light", {
            "target": L_FLOOR,
            "kelvin": 6500,  # 1_000_000 / 6500 = 154 -> clamped to 200
        })
        self.assertFalse(is_err, data2)
        self.assertEqual(self.fake.api_calls[0][1], "/clip/v2/resource/light/" + L_FLOOR)
        self.assertEqual(self.fake.api_calls[0][2]["color_temperature"]["mirek"], 200)

        # Warm clamp (2000K -> 500 -> clamped to 454)
        self.fake.api_calls.clear()
        data3, is_err, _ = self._call_tool("hue_set_light", {"target": "FLOOR LAMP", "kelvin": 2000})
        self.assertFalse(is_err, data3)
        self.assertEqual(self.fake.api_calls[0][2]["color_temperature"]["mirek"], 454)

    def test_set_light_validation_and_close_candidates(self):
        # Close candidate suggestion on typo
        data, is_err, _ = self._call_tool("hue_set_light", {"target": "Soffa Lamp", "on": True})
        self.assertTrue(is_err)
        self.assertIn("Sofa Lamp", data["error"])

        # Mutually exclusive kelvin + color_hex
        data, is_err, _ = self._call_tool("hue_set_light", {"target": L_SOFA, "kelvin": 3000, "color_hex": "#ffffff"})
        self.assertTrue(is_err)

        # Invalid effect syntax
        data, is_err, _ = self._call_tool("hue_set_light", {"target": L_SOFA, "effect": "BAD EFFECT!"})
        self.assertTrue(is_err)

        # Unsupported effect for light
        data, is_err, _ = self._call_tool("hue_set_light", {"target": L_DESK, "effect": "prism"})
        self.assertTrue(is_err)
        self.assertIn("candle", data["error"])

        # Non-color light rejects color_hex
        data, is_err, _ = self._call_tool("hue_set_light", {"target": "Floor Lamp", "color_hex": "#ff0000"})
        self.assertTrue(is_err)

        # Non-dimmable plug rejects brightness
        data, is_err, _ = self._call_tool("hue_set_light", {"target": "Hallway Plug", "brightness": 50})
        self.assertTrue(is_err)

    def test_set_room_grouped_light_and_per_light_color(self):
        # Power + brightness on room with grouped_light -> single grouped_light PUT
        data, is_err, _ = self._call_tool("hue_set_room", {"target": "living room", "brightness": 60})
        self.assertFalse(is_err, data)
        self.assertEqual(data["calls"], 1)
        self.assertEqual(self.fake.api_calls[0], (
            "PUT",
            "/clip/v2/resource/grouped_light/" + GL_LIVING,
            {"dimming": {"brightness": 60.0}, "on": {"on": True}},
        ))

        # Color hex on Living Room -> per-light PUT (only Sofa Lamp supports color)
        self.fake.api_calls.clear()
        data2, is_err, _ = self._call_tool("hue_set_room", {"target": ROOM_LIVING, "color_hex": "#1e88e5"})
        self.assertFalse(is_err, data2)
        self.assertEqual(data2["calls"], 1)
        self.assertEqual(self.fake.api_calls[0][1], "/clip/v2/resource/light/" + L_SOFA)

        # Room without grouped_light (Office) -> per-light PUT
        self.fake.api_calls.clear()
        data3, is_err, _ = self._call_tool("hue_set_room", {"target": "Office", "on": False})
        self.assertFalse(is_err, data3)
        self.assertEqual(self.fake.api_calls[0], ("PUT", "/clip/v2/resource/light/" + L_DESK, {"on": {"on": False}}))

    def test_recall_scene_unique_ambiguous_disambiguated_and_uuid(self):
        # Unique scene name
        data, is_err, _ = self._call_tool("hue_recall_scene", {"scene": "concentrate"})
        self.assertFalse(is_err, data)
        self.assertEqual(self.fake.api_calls[0], (
            "PUT",
            "/clip/v2/resource/scene/" + SCENE_CONCENTRATE,
            {"recall": {"action": "active"}},
        ))

        # Ambiguous scene name across two rooms (previous recall invalidated cache, so a GET refresh runs, but NO PUT!)
        self.fake.api_calls.clear()
        amb, is_err, _ = self._call_tool("hue_recall_scene", {"scene": "Relax"})
        self.assertTrue(is_err)
        self.assertIn("Living Room", amb["error"])
        self.assertIn("Office", amb["error"])
        self.assertEqual([c for c in self.fake.api_calls if c[0] != "GET"], [])

        # Disambiguated with room name
        self.fake.api_calls.clear()
        data2, is_err, _ = self._call_tool("hue_recall_scene", {"scene": "relax", "room": "office"})
        self.assertFalse(is_err, data2)
        puts2 = [c for c in self.fake.api_calls if c[0] == "PUT"]
        self.assertEqual(puts2, [("PUT", "/clip/v2/resource/scene/" + SCENE_RELAX_OFFICE, {"recall": {"action": "active"}})])

        # By exact UUID without room
        self.fake.api_calls.clear()
        data3, is_err, _ = self._call_tool("hue_recall_scene", {"scene": SCENE_RELAX_LIVING})
        self.assertFalse(is_err, data3)
        puts3 = [c for c in self.fake.api_calls if c[0] == "PUT"]
        self.assertEqual(puts3, [("PUT", "/clip/v2/resource/scene/" + SCENE_RELAX_LIVING, {"recall": {"action": "active"}})])

    def test_apply_theme_to_light_and_room(self):
        # Built-in theme "Cyberpunk" on single light by name
        data, is_err, _ = self._call_tool("hue_apply_theme", {"theme": "cyberpunk", "target": "Sofa Lamp"})
        self.assertFalse(is_err, data)
        self.assertEqual(data["target_kind"], "light")
        self.assertEqual(len(self.fake.api_calls), 1)
        self.assertEqual(self.fake.api_calls[0][1], "/clip/v2/resource/light/" + L_SOFA)
        self.assertEqual(self.fake.api_calls[0][2]["dimming"]["brightness"], 85.0)
        self.assertIn("color", self.fake.api_calls[0][2])

        # Built-in theme "Reading" on whole room by name -> updates both Sofa Lamp & Floor Lamp
        self.fake.api_calls.clear()
        data2, is_err, _ = self._call_tool("hue_apply_theme", {"theme": "Reading", "target": "Living Room"})
        self.assertFalse(is_err, data2)
        self.assertEqual(data2["target_kind"], "room")
        self.assertEqual(len(data2["lights_updated"]), 2)
        paths = {c[1] for c in self.fake.api_calls}
        self.assertEqual(paths, {"/clip/v2/resource/light/" + L_SOFA, "/clip/v2/resource/light/" + L_FLOOR})
        for _, _, body in self.fake.api_calls:
            self.assertEqual(body["color_temperature"]["mirek"], 320)

        # Custom theme by name + light by UUID
        self.fake.api_calls.clear()
        data3, is_err, _ = self._call_tool("hue_apply_theme", {"theme": "neon mint", "target": L_DESK})
        self.assertFalse(is_err, data3)
        self.assertEqual(self.fake.api_calls[0][2]["dimming"]["brightness"], 77.0)

    def test_all_off_and_all_on_with_and_without_exclusions(self):
        # all_off turns off Living Room (grouped_light), Office (Desk Lamp), and Hallway Plug
        off_data, is_err, _ = self._call_tool("hue_all_off")
        self.assertFalse(is_err, off_data)
        self.assertEqual(off_data["on_count"], 0)
        paths = {c[1] for c in self.fake.api_calls}
        self.assertEqual(paths, {
            "/clip/v2/resource/grouped_light/" + GL_LIVING,
            "/clip/v2/resource/light/" + L_DESK,
            "/clip/v2/resource/light/" + L_PLUG,
        })

        # all_on with exclude_rooms (by name) skips Living Room
        self.fake.api_calls.clear()
        on_excl, is_err, _ = self._call_tool("hue_all_on", {"exclude_rooms": ["living room"]})
        self.assertFalse(is_err, on_excl)
        self.assertEqual(on_excl["excluded_rooms"], ["Living Room"])
        paths = {c[1] for c in self.fake.api_calls}
        self.assertNotIn("/clip/v2/resource/grouped_light/" + GL_LIVING, paths)
        self.assertNotIn("/clip/v2/resource/grouped_light/" + GL_HOME, paths)
        self.assertIn("/clip/v2/resource/light/" + L_DESK, paths)
        self.assertIn("/clip/v2/resource/light/" + L_PLUG, paths)
        self.assertEqual(on_excl["on_count"], 2)

        # all_on without exclusions uses bridge_home grouped_light
        self.fake.api_calls.clear()
        on_all, is_err, _ = self._call_tool("hue_all_on")
        self.assertFalse(is_err, on_all)
        self.assertEqual(self.fake.api_calls, [
            ("PUT", "/clip/v2/resource/grouped_light/" + GL_HOME, {"on": {"on": True}})
        ])
        self.assertEqual(on_all["on_count"], 4)

    def test_set_sensor_motion_contact_and_button_rejection(self):
        # Motion sensor by case-insensitive name
        data, is_err, _ = self._call_tool("hue_set_sensor", {"target": "hallway motion", "enabled": False})
        self.assertFalse(is_err, data)
        self.assertEqual(self.fake.api_calls[0], ("PUT", "/clip/v2/resource/motion/" + S_MOT, {"enabled": False}))
        self.assertFalse(data["sensor"]["enabled"])

        # Contact sensor by UUID
        self.fake.api_calls.clear()
        data2, is_err, _ = self._call_tool("hue_set_sensor", {"target": S_CON, "enabled": False})
        self.assertFalse(is_err, data2)
        self.assertEqual(self.fake.api_calls[0], ("PUT", "/clip/v2/resource/contact/" + S_CON, {"enabled": False}))

        # Button sensor rejected
        self.fake.api_calls.clear()
        bad, is_err, _ = self._call_tool("hue_set_sensor", {"target": "Dimmer Switch", "enabled": False})
        self.assertTrue(is_err)
        self.assertEqual(self.fake.api_calls, [])

    def test_set_motion_area_by_name_and_uuid_and_bounds(self):
        # Update both enabled + sensitivity by name
        data, is_err, _ = self._call_tool("hue_set_motion_area", {
            "target": "living room motion",
            "enabled": False,
            "sensitivity": 4,
        })
        self.assertFalse(is_err, data)
        self.assertEqual(self.fake.api_calls, [
            ("PUT", "/clip/v2/resource/motion_area_configuration/" + MAC_LIVING, {"enabled": False}),
            ("PUT", "/clip/v2/resource/convenience_area_motion/" + CONV_LIVING, {"sensitivity": {"sensitivity": 4}}),
        ])
        self.assertFalse(data["motion_area"]["enabled"])
        self.assertEqual(data["motion_area"]["sensitivity"], 4)
        # Light motion_aware flag should update immediately
        sofa = self.fake._find_light(L_SOFA)
        self.assertFalse(sofa["motion_aware"])

        # By UUID
        self.fake.api_calls.clear()
        data2, is_err, _ = self._call_tool("hue_set_motion_area", {"target": MAC_OFFICE, "enabled": True})
        self.assertFalse(is_err, data2)
        self.assertTrue(self.fake._find_light(L_DESK)["motion_aware"])

        # Out of range sensitivity rejected
        bad, is_err, _ = self._call_tool("hue_set_motion_area", {"target": MAC_LIVING, "sensitivity": 9})
        self.assertTrue(is_err)

    def test_create_toggle_delete_rules_including_sensors_and_cycle_rejection(self):
        # 1. Sensor trigger (physical motion sensor -> room)
        r_sens, is_err, _ = self._call_tool("hue_create_rule", {
            "trigger": "Hallway Motion",
            "target": "Living Room",
            "mirror_off": True,
            "only_if_off": True,
            "time_start": "18:00",
            "time_end": "07:00",
            "name": "Motion Living",
        })
        self.assertFalse(is_err, r_sens)
        rule = r_sens["rule"]
        self.assertEqual(rule["kind"], "sensor")
        self.assertEqual(rule["trigger_kind"], "motion")
        self.assertEqual(rule["trigger_id"], S_MOT)
        self.assertEqual(rule["trigger_state"], "motion")
        self.assertEqual(rule["targets"][0]["kind"], "room")
        self.assertEqual(rule["targets"][0]["id"], ROOM_LIVING)
        self.assertTrue(rule["mirror_off"])
        self.assertTrue(rule["only_if_off"])
        self.assertEqual((rule["time_start"], rule["time_end"]), ("18:00", "07:00"))
        # Persisted to rules.json!
        saved = hue_mod.load_rules()
        self.assertTrue(any(r["id"] == rule["id"] for r in saved["rules"]))

        # 2. MotionAware area trigger by UUID -> light by name
        r_ma, is_err, _ = self._call_tool("hue_create_rule", {
            "trigger": MAC_OFFICE,
            "target": "Desk Lamp",
            "name": "Pro Motion Desk",
        })
        self.assertFalse(is_err, r_ma)
        self.assertEqual(r_ma["rule"]["trigger_kind"], "motion_area")
        self.assertEqual(r_ma["rule"]["trigger_state"], "motion")

        # 3. Contradictory cycle rejection:
        # Existing rule is L_DESK on -> L_SOFA on.
        # Adding L_SOFA on -> L_DESK off (exclusive) forms a contradictory cycle!
        cyc, is_err, _ = self._call_tool("hue_create_rule", {
            "trigger": "Sofa Lamp",
            "target": "Desk Lamp",
            "kind": "exclusive",
            "name": "Bad Cycle",
        })
        self.assertTrue(is_err)
        self.assertIn("cycle", cyc["error"].lower())
        self.assertFalse(any(r["name"] == "Bad Cycle" for r in self.fake.rules))

        # 4. Toggle rule off by name, then back on by UUID
        t1, is_err, _ = self._call_tool("hue_toggle_rule", {"rule": "motion living", "enabled": False})
        self.assertFalse(is_err, t1)
        self.assertFalse(t1["rule"]["enabled"])

        t2, is_err, _ = self._call_tool("hue_toggle_rule", {"rule": rule["id"], "enabled": True})
        self.assertFalse(is_err, t2)
        self.assertTrue(t2["rule"]["enabled"])

        # 5. Delete rule by name, and another by UUID
        d1, is_err, _ = self._call_tool("hue_delete_rule", {"rule": "Motion Living"})
        self.assertFalse(is_err, d1)
        self.assertEqual(d1["deleted"]["id"], rule["id"])

        d2, is_err, _ = self._call_tool("hue_delete_rule", {"rule": r_ma["rule"]["id"]})
        self.assertFalse(is_err, d2)
        saved_after = hue_mod.load_rules()
        self.assertEqual([r["id"] for r in saved_after["rules"]], [RULE_EXISTING])


if __name__ == "__main__":
    unittest.main()
