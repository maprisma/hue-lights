"""Unit tests for multi-device local-first lighting providers (scripts/hue_providers.py)."""
import ast
import importlib.util
import json
import os
import shutil
import stat
import struct
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
PROV_SCRIPT = os.path.join(HERE, "..", "scripts", "hue_providers.py")
HUE_SCRIPT = os.path.join(HERE, "..", "scripts", "hue-lights.py")


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


prov = _load("hue_providers_test", PROV_SCRIPT)
hue = _load("hue_lights_prov_test", HUE_SCRIPT)


class ProviderSecurityAndValidationTests(unittest.TestCase):
    def test_stdlib_only(self):
        tree = ast.parse(open(PROV_SCRIPT, encoding="utf-8").read())
        imported = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                imported.update(a.name.split(".")[0] for a in node.names)
            elif isinstance(node, ast.ImportFrom) and node.module:
                imported.add(node.module.split(".")[0])
        self.assertEqual(imported - set(sys.stdlib_module_names), set())

    def test_local_host_and_resource_id_validation(self):
        self.assertTrue(prov.valid_local_host("192.168.178.42"))
        self.assertTrue(prov.valid_local_host("10.0.0.5"))
        self.assertTrue(prov.valid_local_host("wled-desk.local"))
        self.assertFalse(prov.valid_local_host("8.8.8.8"))
        self.assertFalse(prov.valid_local_host("169.254.169.254"))
        self.assertFalse(prov.valid_local_host("example.com"))
        self.assertFalse(prov.valid_local_host("127.0.0.1", allow_loopback=False))
        self.assertTrue(prov.valid_local_host("127.0.0.1", allow_loopback=True))
        self.assertTrue(prov.valid_local_host("localhost", allow_loopback=True))

        # CR/LF injection stripped from tokens
        self.assertEqual(prov.clean_token("tok\r\nX-Injected: 1"), "tokX-Injected: 1")

        self.assertTrue(prov.valid_resource_id("11111111-1111-1111-1111-111111111111"))
        self.assertTrue(prov.valid_resource_id("wled:desk1:0"))
        self.assertTrue(prov.valid_resource_id("ha:main:light.kitchen"))
        self.assertFalse(prov.valid_resource_id("ha:main:../bad"))
        self.assertEqual(prov.provider_prefix("11111111-1111-1111-1111-111111111111"), "hue")
        self.assertEqual(prov.provider_prefix("wled:desk1:0"), "wled")
        self.assertTrue(prov.is_extra_resource("wiz:lamp1:light"))
        self.assertFalse(prov.is_extra_resource("11111111-1111-1111-1111-111111111111"))


class ProviderParserTests(unittest.TestCase):
    def test_home_assistant_parser(self):
        conn = prov.sanitize_connection({"id": "ha1", "kind": "home_assistant", "name": "HA", "host": "192.168.1.10", "token": "secret-tok-123"})
        states = [
            {
                "entity_id": "light.kitchen_strip",
                "state": "on",
                "attributes": {
                    "friendly_name": "Kitchen Strip",
                    "brightness": 204,
                    "rgb_color": [255, 100, 50],
                    "supported_color_modes": ["rgb", "color_temp"],
                    "effect_list": ["Rainbow"],
                    "effect": "Rainbow",
                },
            },
            {"entity_id": "scene.movie_night", "state": "scening", "attributes": {"friendly_name": "Movie Night"}},
            {"entity_id": "binary_sensor.hall_motion", "state": "on", "attributes": {"friendly_name": "Hall Motion", "device_class": "motion"}},
        ]
        rooms, sensors = prov.parse_home_assistant_states(conn, states, {"light.kitchen_strip": "Kitchen"})
        self.assertEqual(len(rooms), 1)
        self.assertEqual(rooms[0]["name"], "Kitchen")
        self.assertEqual(rooms[0]["provider"], "home_assistant")
        self.assertEqual(rooms[0]["lights"][0]["color"], "#ff6432")
        self.assertEqual(rooms[0]["scenes"][0]["name"], "Movie Night")
        self.assertEqual(len(sensors), 1)
        self.assertTrue(sensors[0]["motion"])

    def test_wled_wiz_lifx_govee_nanoleaf_shelly_dirigera(self):
        # WLED
        wconn = prov.sanitize_connection({"id": "w1", "kind": "wled", "name": "Desk", "host": "192.168.1.40", "room_name": "Office"})
        wrooms, _, effects = prov.parse_wled(
            wconn,
            {"state": {"on": True, "bri": 128, "ps": 2, "seg": [{"col": [[0, 200, 255]], "fx": 1}]}, "info": {"name": "Desk WLED"}, "effects": ["Solid", "Blink", "Rainbow"]},
            {"1": {"n": "Warm"}, "2": {"n": "Cyberpunk"}},
        )
        self.assertEqual(wrooms[0]["name"], "Office")
        self.assertEqual(wrooms[0]["lights"][0]["color"], "#00c8ff")
        self.assertEqual(len(wrooms[0]["scenes"]), 2)
        self.assertIn(prov.build_wled_payload({"on": {"on": True}, "dimming": {"brightness": 50}}, effects)["bri"], (127, 128))

        # WiZ
        wiz_conn = prov.sanitize_connection({"id": "z1", "kind": "wiz", "name": "Floor", "host": "192.168.1.41"})
        zrooms, _ = prov.parse_wiz_pilot(wiz_conn, {"result": {"state": True, "dimming": 70, "r": 255, "g": 170, "b": 40, "sceneId": 6}})
        self.assertEqual(zrooms[0]["lights"][0]["color"], "#ffaa28")
        cmd = json.loads(prov.build_wiz_set_pilot({"on": {"on": True}, "dimming": {"brightness": 60}}))
        self.assertEqual(cmd["method"], "setPilot")

        # LIFX binary encode & decode
        self.assertEqual(len(prov.encode_lifx_get_color()), 36)
        self.assertEqual(len(prov.encode_lifx_set_power(True)), 42)
        pkt = bytearray(88)
        struct.pack_into("<H", pkt, 32, prov.LIFX_LIGHT_STATE)
        struct.pack_into("<HHHHhH", pkt, 36, 0, 65535, 32768, 3500, 0, 65535)
        pkt[48:59] = b"Studio Bulb"
        st = prov.decode_lifx_light_state(bytes(pkt))
        self.assertTrue(st["power_on"])
        self.assertEqual(st["label"], "Studio Bulb")
        lrooms, _ = prov.lifx_to_rooms(prov.sanitize_connection({"id": "l1", "kind": "lifx", "host": "192.168.1.42"}), st)
        self.assertEqual(lrooms[0]["lights"][0]["name"], "Studio Bulb")

        # Govee
        gconn = prov.sanitize_connection({"id": "g1", "kind": "govee", "name": "TV", "host": "192.168.1.43"})
        grooms, _ = prov.parse_govee_status(gconn, {"msg": {"data": {"onOff": 1, "brightness": 85, "color": {"r": 120, "g": 40, "b": 255}}}})
        self.assertEqual(grooms[0]["lights"][0]["color"], "#7828ff")
        self.assertEqual(len(prov.build_govee_commands({"on": {"on": True}, "dimming": {"brightness": 80}})), 2)

        # Nanoleaf
        nconn = prov.sanitize_connection({"id": "n1", "kind": "nanoleaf", "name": "Shapes", "host": "192.168.1.44", "token": "tok"})
        nrooms, _, smap = prov.parse_nanoleaf(nconn, {
            "name": "Shapes", "state": {"on": {"value": True}, "brightness": {"value": 75}, "hue": {"value": 180}, "sat": {"value": 80}, "ct": {"value": 3000}, "colorMode": "hs"},
            "effects": {"select": "Aurora", "effectsList": ["Aurora", "Sunset"]},
        })
        self.assertEqual(len(nrooms[0]["scenes"]), 2)
        self.assertEqual(len(smap), 2)

        # Shelly Gen2 & Gen1
        sconn = prov.sanitize_connection({"id": "s1", "kind": "shelly", "name": "Dimmer", "host": "192.168.1.45"})
        srooms, _, smeta = prov.parse_shelly_gen2(sconn, {"name": "Hall Dimmer", "mac": "AA"}, {"light:0": {"output": True, "brightness": 64}})
        self.assertEqual(smeta["shelly:s1:light:0"]["gen"], 2)
        self.assertEqual(len(srooms[0]["lights"]), 1)
        s1rooms, _, s1meta = prov.parse_shelly_gen1(sconn, {"relays": [{"ison": True}]})
        self.assertEqual(s1meta["shelly:s1:relay:0"]["gen"], 1)
        self.assertEqual(len(s1rooms[0]["lights"]), 1)

        # IKEA DIRIGERA
        dconn = prov.sanitize_connection({"id": "d1", "kind": "dirigera", "name": "IKEA", "host": "192.168.1.46", "token": "tok"})
        drooms, dsensors, dbounds = prov.parse_dirigera(
            dconn,
            [
                {"id": "dev-1", "type": "light", "isReachable": True, "room": {"id": "r1", "name": "Bedroom"}, "attributes": {"customName": "TRÅDFRI", "isOn": True, "lightLevel": 45, "colorTemperature": 2700}, "capabilities": {"canReceive": ["isOn", "lightLevel", "colorTemperature"]}},
                {"id": "sen-1", "type": "sensor", "deviceType": "motionSensor", "isReachable": True, "room": {"id": "r1", "name": "Bedroom"}, "attributes": {"customName": "VALLHORN", "isDetected": True, "batteryPercentage": 90}},
            ],
            [{"id": "sc-1", "info": {"name": "Good Night"}}],
        )
        self.assertEqual(drooms[0]["name"], "Bedroom")
        self.assertEqual(drooms[0]["scenes"][0]["name"], "Good Night")
        self.assertEqual(len(dsensors), 1)
        self.assertTrue(dsensors[0]["motion"])
        self.assertIn("dirigera:d1:dev-1", dbounds)


class ProviderManagerPersistenceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def test_private_persistence_and_token_never_leaks(self):
        mgr = prov.ProviderManager(self.tmp)
        saved = mgr.save_connection({
            "id": "ha1",
            "kind": "home_assistant",
            "name": "Home Assistant",
            "host": "192.168.1.10",
            "token": "super-secret-ha-token-999",
        })
        self.assertEqual(saved["id"], "ha1")
        pfile = os.path.join(self.tmp, "providers.json")
        self.assertEqual(stat.S_IMODE(os.stat(pfile).st_mode), 0o600)

        mgr2 = prov.ProviderManager(self.tmp)
        _, _, _, _, plist = mgr2.snapshot_parts()
        self.assertEqual(len(plist), 1)
        self.assertTrue(plist[0]["has_token"])
        self.assertNotIn("token", plist[0])
        self.assertNotIn("super-secret-ha-token-999", json.dumps(plist))

        # Editing without re-entering token preserves the saved token ONLY when target matches
        mgr2.save_connection({"id": "ha1", "kind": "home_assistant", "name": "Renamed HA", "host": "192.168.1.10"})
        self.assertIn("super-secret-ha-token-999", mgr2.secrets())
        self.assertNotIn("super-secret-ha-token-999", mgr2.redact("Auth failed with super-secret-ha-token-999"))

        # Changing host MUST NOT reuse the saved token from the previous host
        with self.assertRaises(prov.ProviderError):
            mgr2.save_connection({"id": "ha1", "kind": "home_assistant", "name": "Moved HA", "host": "192.168.1.99"})

        # Server-side one-time pair_id keeps raw tokens out of snapshots
        pid = mgr2.store_paired_token("nanoleaf", "192.168.1.55", 16021, "paired-nano-secret-xyz")
        self.assertNotIn("paired-nano-secret-xyz", pid)
        saved_nano = mgr2.save_connection({
            "id": "n_pair",
            "kind": "nanoleaf",
            "name": "Nano",
            "host": "192.168.1.55",
            "port": 16021,
            "pair_id": pid,
        })
        self.assertEqual(saved_nano["id"], "n_pair")
        self.assertIn("paired-nano-secret-xyz", mgr2.secrets())
        # One-time pair_id is consumed and cannot be replayed
        with self.assertRaises(prov.ProviderError):
            mgr2.save_connection({
                "id": "n_pair2",
                "kind": "nanoleaf",
                "name": "Nano2",
                "host": "192.168.1.55",
                "port": 16021,
                "pair_id": pid,
            })

        # Non-0600 providers.json is refused on load
        os.chmod(pfile, 0o644)
        mgr_bad = prov.ProviderManager(self.tmp)
        self.assertEqual(mgr_bad.connections, [])
        os.chmod(pfile, 0o600)

        mgr2.toggle_connection("ha1", False)
        mgr2.toggle_connection("n_pair", False)
        self.assertFalse(mgr2.has_enabled())
        mgr2.remove_connection("ha1")
        mgr2.remove_connection("n_pair")
        self.assertEqual(mgr2.connections, [])


if __name__ == "__main__":
    unittest.main()
