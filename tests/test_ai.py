import importlib.util
import json
import os
import socket
import tempfile
import time
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))

_hue_spec = importlib.util.spec_from_file_location("hue_lights", os.path.join(HERE, "..", "scripts", "hue-lights.py"))
hue_mod = importlib.util.module_from_spec(_hue_spec)
_hue_spec.loader.exec_module(hue_mod)

_ai_spec = importlib.util.spec_from_file_location("hue_ai", os.path.join(HERE, "..", "scripts", "hue-ai.py"))
ai_mod = importlib.util.module_from_spec(_ai_spec)
_ai_spec.loader.exec_module(ai_mod)


# Deterministic UUIDs for fixtures
L_SOFA = "11111111-1111-1111-1111-111111111111"
L_FLOOR = "22222222-2222-2222-2222-222222222222"
L_DESK = "33333333-3333-3333-3333-333333333333"
L_LR_DESK = "33333333-4444-4444-4444-444444444444"
L_BLUE = "33333333-5555-5555-5555-555555555555"

D1 = "aaaaaaaa-1111-1111-1111-111111111111"
D2 = "aaaaaaaa-2222-2222-2222-222222222222"
D3 = "aaaaaaaa-3333-3333-3333-333333333333"
D4 = "aaaaaaaa-4444-4444-4444-444444444444"
D5 = "aaaaaaaa-5555-5555-5555-555555555555"

GL_LIVING = "bbbbbbbb-1111-1111-1111-111111111111"
GL_HOME = "bbbbbbbb-9999-9999-9999-999999999999"

ROOM_LIVING = "cccccccc-1111-1111-1111-111111111111"
ROOM_OFFICE = "cccccccc-2222-2222-2222-222222222222"

SC_MOVIE = "dddddddd-1111-1111-1111-111111111111"
SC_ENERGIZE_LR = "dddddddd-2222-2222-2222-222222222222"
SC_ENERGIZE_OFF = "dddddddd-3333-3333-3333-333333333333"

MAC_LIVING = "55555555-1111-1111-1111-111111111111"
CONV_LIVING = "66666666-1111-1111-1111-111111111111"

S_MOT = "77777777-2222-2222-2222-222222222222"
D_MOT = "77777777-1111-1111-1111-111111111111"
S_CON = "88888888-2222-2222-2222-222222222222"
D_CON = "88888888-1111-1111-1111-111111111111"
D_BTN = "99999999-1111-1111-1111-111111111111"
S_BTN = "99999999-2222-2222-2222-222222222222"

SECRET_APP_KEY = "SuperSecretAppKeyValue1234567890"
SECRET_CLIENT_KEY = "AABBCCDDEEFF00112233445566778899"


def _make_hue(tmpdir, german_names=False, paired=True, schedule_mode="local"):
    for fname in ("local_schedules.json", "rules.json", "themes.json"):
        fpath = os.path.join(tmpdir, fname)
        if os.path.exists(fpath):
            os.remove(fpath)
    hue_mod.CONFIG_DIR = tmpdir
    hue_mod.CONFIG_PATH = os.path.join(tmpdir, "config.json")
    h = hue_mod.Hue()
    h.emit = lambda: None
    if paired:
        h.config = {
            "ip": "192.168.1.42",
            "app_key": SECRET_APP_KEY,
            "client_key": SECRET_CLIENT_KEY,
            "bridge_id": "001788FFFE112233",
            "fingerprint": "deadbeef" * 8,
            "name": "Test Bridge",
        }
        h.remember_secrets(SECRET_APP_KEY, SECRET_CLIENT_KEY)
        h.phase = "ready"
    else:
        h.config = {}
        h.phase = "unpaired"

    h.bridge_home_gl = GL_HOME
    h.bridge_model_id = "BSB003"
    h.handle({"cmd": "schedule_mode", "mode": schedule_mode})

    living_name = "Wohnzimmer" if german_names else "Living Room"
    office_name = "Büro" if german_names else "Office"
    sofa_name = "Sofa Lampe" if german_names else "Sofa Lamp"
    floor_name = "Stehlampe" if german_names else "Floor Lamp"
    desk_name = "Schreibtischlampe" if german_names else "Desk Lamp"
    lr_desk_name = "Wohnzimmer Schreibtisch" if german_names else "Living Room Desk"
    blue_name = "Blue Lamp"
    movie_name = "Abendstimmung" if german_names else "Movie Night"
    mot_name = "Flur Bewegung" if german_names else "Hallway Motion"
    con_name = "Türkontakt" if german_names else "Front Door Contact"
    btn_name = "Flur Schalter" if german_names else "Wall Switch"
    ma_name = "Wohnzimmer Bewegung" if german_names else "Living Room Motion"

    h.rooms = [
        {
            "id": ROOM_LIVING,
            "kind": "room",
            "name": living_name,
            "grouped_light": GL_LIVING,
            "on": True,
            "bri": 70.0,
            "on_count": 1,
            "archetype": "living_room",
            "color": "#ffd27a",
            "lights": [
                {
                    "id": L_SOFA, "device_id": D1, "name": sofa_name,
                    "on": True, "bri": 70.0, "dimmable": True, "color": "#ffd27a",
                    "reachable": True, "supports_color": True, "supports_ct": True,
                    "mirek": 350, "mirek_min": 153, "mirek_max": 500,
                    "effect": "no_effect",
                    "available_effects": ["no_effect", "candle", "fire", "sparkle", "prism"],
                    "motion_aware": True, "motion_active": True,
                },
                {
                    "id": L_FLOOR, "device_id": D2, "name": floor_name,
                    "on": False, "bri": 50.0, "dimmable": True, "color": "#ffd27a",
                    "reachable": True, "supports_color": True, "supports_ct": True,
                    "mirek": 370, "mirek_min": 153, "mirek_max": 500,
                    "effect": "no_effect",
                    "available_effects": ["no_effect", "candle", "fire", "sparkle", "prism"],
                    "motion_aware": True, "motion_active": True,
                },
                {
                    "id": L_LR_DESK, "device_id": D4, "name": lr_desk_name,
                    "on": False, "bri": 60.0, "dimmable": True, "color": "#ffd27a",
                    "reachable": True, "supports_color": True, "supports_ct": True,
                    "mirek": 300, "mirek_min": 153, "mirek_max": 500,
                    "effect": "no_effect",
                    "available_effects": ["no_effect", "candle", "fire", "sparkle", "prism"],
                    "motion_aware": False, "motion_active": False,
                },
            ],
            "scenes": [
                {"id": SC_MOVIE, "name": movie_name, "active": False},
                {"id": SC_ENERGIZE_LR, "name": "Energize", "active": False},
            ],
        },
        {
            "id": ROOM_OFFICE,
            "kind": "room",
            "name": office_name,
            "grouped_light": "",  # Intentionally empty to test member-light fallback
            "on": False,
            "bri": 0.0,
            "on_count": 0,
            "archetype": "office",
            "color": "#ffd27a",
            "lights": [
                {
                    "id": L_DESK, "device_id": D3, "name": desk_name,
                    "on": False, "bri": 40.0, "dimmable": True, "color": "#ffd27a",
                    "reachable": True, "supports_color": True, "supports_ct": True,
                    "mirek": 250, "mirek_min": 153, "mirek_max": 500,
                    "effect": "no_effect",
                    "available_effects": ["no_effect", "candle", "fire", "sparkle", "prism"],
                    "motion_aware": False, "motion_active": False,
                },
                {
                    "id": L_BLUE, "device_id": D5, "name": blue_name,
                    "on": True, "bri": 80.0, "dimmable": True, "color": "#1a53ff",
                    "reachable": True, "supports_color": True, "supports_ct": True,
                    "mirek": None, "mirek_min": 153, "mirek_max": 500,
                    "effect": "no_effect",
                    "available_effects": ["no_effect", "candle", "fire", "sparkle", "prism"],
                    "motion_aware": False, "motion_active": False,
                },
            ],
            "scenes": [
                {"id": SC_ENERGIZE_OFF, "name": "Energize", "active": False},
            ],
        },
    ]
    h.light_count = 5
    h.on_count = 2
    h.sensors = [
        {
            "id": S_MOT, "device_id": D_MOT, "kind": "motion", "name": mot_name,
            "room_id": "", "enabled": True, "motion": True, "open": False,
            "temperature": 21.5, "lux": 42.0, "battery": 90, "battery_state": "normal",
            "sensitivity": 2, "sensitivity_max": 4, "button_ids": [], "last_event": "",
        },
        {
            "id": S_CON, "device_id": D_CON, "kind": "contact", "name": con_name,
            "room_id": ROOM_OFFICE, "enabled": True, "motion": False, "open": True,
            "temperature": None, "lux": None, "battery": 85, "battery_state": "normal",
            "sensitivity": None, "sensitivity_max": None, "button_ids": [], "last_event": "",
        },
        {
            "id": D_BTN, "device_id": D_BTN, "kind": "button", "name": btn_name,
            "room_id": ROOM_LIVING, "enabled": True, "motion": False, "open": False,
            "temperature": None, "lux": None, "battery": 100, "battery_state": "normal",
            "sensitivity": None, "sensitivity_max": None, "button_ids": [S_BTN], "last_event": "",
        },
    ]
    h.motion_areas = [
        {
            "id": MAC_LIVING, "name": ma_name, "enabled": True, "room_id": ROOM_LIVING,
            "motion": True, "motion_valid": True, "sensitivity": 3, "sensitivity_max": 4,
            "convenience_id": CONV_LIVING, "light_ids": [L_SOFA, L_FLOOR], "health": "healthy",
        },
    ]
    h.custom_themes = [
        {"id": "jazz_club", "name": "Jazz Club", "hex": "#8a2be2", "mirek": None, "bri": 45, "effect": "no_effect", "built_in": False},
    ]
    h.rules = []
    h.local_schedules = []
    h.bridge_schedules = []
    h.schedules = []
    h.pending = {}
    h.api_queue = []
    return h


class BaseAITest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.orig_dir = hue_mod.CONFIG_DIR
        self.orig_path = hue_mod.CONFIG_PATH

    def tearDown(self):
        hue_mod.CONFIG_DIR = self.orig_dir
        hue_mod.CONFIG_PATH = self.orig_path
        self._tmp.cleanup()


class TestAllOnOff(BaseAITest):
    def test_all_off_en_and_de(self):
        for prompt in ("all lights off", "turn off everything", "alle lichter aus"):
            h = _make_hue(self._tmp.name)
            res = ai_mod.execute_ask_ai(h, prompt)
            self.assertTrue(res["ok"], prompt)
            self.assertEqual(res["engine"], "builtin")
            self.assertIn("all_off", res["actions"])
            self.assertIn(("grouped_light", GL_LIVING), h.pending)
            self.assertEqual(h.pending[("grouped_light", GL_LIVING)]["on"], {"on": False})

    def test_all_on_en_and_de(self):
        for prompt in ("turn all lights on", "alle lampen an", "alle lichter einschalten"):
            h = _make_hue(self._tmp.name)
            res = ai_mod.execute_ask_ai(h, prompt)
            self.assertTrue(res["ok"], prompt)
            self.assertIn("all_on", res["actions"])
            self.assertEqual(h.pending[("grouped_light", GL_HOME)]["on"], {"on": True})

    def test_all_on_except_room(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "turn all lights on except Office")
        self.assertTrue(res["ok"])
        self.assertIn(("grouped_light", GL_LIVING), h.pending)
        self.assertNotIn(("grouped_light", GL_HOME), h.pending)

    def test_all_lights_in_specific_room_targets_room_only(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "turn off all lights in Living Room")
        self.assertTrue(res["ok"])
        self.assertNotIn("all_off", res["actions"])
        self.assertIn(("grouped_light", GL_LIVING), h.pending)
        self.assertEqual(h.pending[("grouped_light", GL_LIVING)]["on"], {"on": False})


class TestDirectControl(BaseAITest):
    def test_longest_name_wins(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "turn on Living Room Desk to 45%")
        self.assertTrue(res["ok"])
        self.assertIn(("light", L_LR_DESK), h.pending)
        self.assertNotIn(("grouped_light", GL_LIVING), h.pending)
        self.assertEqual(h.pending[("light", L_LR_DESK)]["dimming"]["brightness"], 45.0)

    def test_room_without_grouped_light_falls_back_to_member_lights(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "turn on Office at 60%")
        self.assertTrue(res["ok"])
        self.assertIn(("light", L_DESK), h.pending)
        self.assertIn(("light", L_BLUE), h.pending)
        self.assertEqual(h.pending[("light", L_DESK)]["dimming"]["brightness"], 60.0)

    def test_brightness_variants_en_and_de(self):
        cases = [
            ("dim Sofa Lamp to 30%", False, 30.0),
            ("dim Sofa Lamp to 40", False, 40.0),
            ("Sofa Lampe auf 60%", True, 60.0),
            ("Sofa Lampe dimmen", True, 30.0),
            ("make Sofa Lamp bright", False, 100.0),
            ("Sofa Lampe hell", True, 100.0),
        ]
        for prompt, de, expected in cases:
            h = _make_hue(self._tmp.name, german_names=de)
            res = ai_mod.execute_ask_ai(h, prompt)
            self.assertTrue(res["ok"], prompt)
            self.assertEqual(res["engine"], "builtin", prompt)
            self.assertEqual(h.pending[("light", L_SOFA)]["dimming"]["brightness"], expected, prompt)

    def test_warmth_and_kelvin_en_and_de(self):
        cases = [
            ("set Desk Lamp to 2700K", False, 370),
            ("Desk Lamp warm", False, 370),
            ("Schreibtischlampe warmweiß", True, 370),
            ("Desk Lamp candle", False, 454),
            ("Desk Lamp neutral", False, 250),
            ("Desk Lamp daylight", False, 182),
            ("Schreibtischlampe Tageslicht", True, 182),
            ("Desk Lamp cool", False, 160),
            ("Schreibtischlampe kaltweiß", True, 153),
        ]
        for prompt, de, expected_mirek in cases:
            h = _make_hue(self._tmp.name, german_names=de)
            res = ai_mod.execute_ask_ai(h, prompt)
            self.assertTrue(res["ok"], prompt)
            self.assertEqual(h.pending[("light", L_DESK)]["color_temperature"]["mirek"], expected_mirek, prompt)

    def test_named_colors_and_hex_en_and_de(self):
        for prompt, de in (
            ("set Desk Lamp to red", False),
            ("Schreibtischlampe rot", True),
            ("Desk Lamp blue", False),
            ("Schreibtischlampe grün", True),
            ("Schreibtischlampe türkis", True),
            ("Schreibtischlampe lila", True),
            ("Desk Lamp pink", False),
            ("Desk Lamp #ff6600", False),
        ):
            h = _make_hue(self._tmp.name, german_names=de)
            res = ai_mod.execute_ask_ai(h, prompt)
            self.assertTrue(res["ok"], prompt)
            self.assertIn("color", h.pending[("light", L_DESK)], prompt)
            self.assertIn("xy", h.pending[("light", L_DESK)]["color"], prompt)

    def test_entity_name_containing_color_does_not_force_color_on_turn_off(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "turn off Blue Lamp")
        self.assertTrue(res["ok"])
        self.assertEqual(h.pending[("light", L_BLUE)], {"on": {"on": False}})

    def test_native_effects_en_and_de(self):
        cases = [
            ("set Floor Lamp to candle effect", False, "candle"),
            ("Stehlampe Feuer", True, "fire"),
            ("Floor Lamp sparkle", False, "sparkle"),
            ("Stehlampe funkeln", True, "sparkle"),
            ("Floor Lamp prism", False, "prism"),
            ("Floor Lamp no effect", False, "no_effect"),
        ]
        for prompt, de, expected_eff in cases:
            h = _make_hue(self._tmp.name, german_names=de)
            res = ai_mod.execute_ask_ai(h, prompt)
            self.assertTrue(res["ok"], prompt)
            self.assertEqual(h.pending[("light", L_FLOOR)]["effects"]["effect"], expected_eff, prompt)


class TestThemesAndScenes(BaseAITest):
    def test_builtin_and_custom_themes_on_light_and_room(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "apply Cyberpunk theme to Desk Lamp")
        self.assertTrue(res["ok"])
        self.assertIn(("light", L_DESK), h.pending)
        self.assertEqual(h.pending[("light", L_DESK)]["dimming"]["brightness"], 85.0)
        self.assertIn("color", h.pending[("light", L_DESK)])

        # Theme applied to whole room fans out to all lights in room
        h2 = _make_hue(self._tmp.name)
        res2 = ai_mod.execute_ask_ai(h2, "set Living Room to Reading")
        self.assertTrue(res2["ok"])
        for lid in (L_SOFA, L_FLOOR, L_LR_DESK):
            self.assertIn(("light", lid), h2.pending)
            self.assertEqual(h2.pending[("light", lid)]["color_temperature"]["mirek"], 320)

        # Sunset Glow must apply theme, NOT create a sunset schedule
        h3 = _make_hue(self._tmp.name)
        res3 = ai_mod.execute_ask_ai(h3, "apply Sunset Glow to Sofa Lamp")
        self.assertTrue(res3["ok"])
        self.assertEqual(len(h3.api_queue), 0)
        self.assertEqual(len(h3.local_schedules), 0)
        self.assertIn(("light", L_SOFA), h3.pending)

        # Custom theme
        h4 = _make_hue(self._tmp.name)
        res4 = ai_mod.execute_ask_ai(h4, "set Desk Lamp to Jazz Club")
        self.assertTrue(res4["ok"])
        self.assertEqual(h4.pending[("light", L_DESK)]["dimming"]["brightness"], 45.0)

        # German theme alias
        h5 = _make_hue(self._tmp.name, german_names=True)
        res5 = ai_mod.execute_ask_ai(h5, "Kaminfeuer auf Stehlampe")
        self.assertTrue(res5["ok"])
        self.assertEqual(h5.pending[("light", L_FLOOR)]["effects"]["effect"], "candle")

    def test_scene_recall_and_room_scoping(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "activate Movie Night in Living Room")
        self.assertTrue(res["ok"])
        self.assertIn(("scene", SC_MOVIE), h.pending)

        # Disambiguate shared scene name by room
        h2 = _make_hue(self._tmp.name)
        res2 = ai_mod.execute_ask_ai(h2, "activate Energize in Office")
        self.assertTrue(res2["ok"])
        self.assertIn(("scene", SC_ENERGIZE_OFF), h2.pending)
        self.assertNotIn(("scene", SC_ENERGIZE_LR), h2.pending)

        # German scene recall
        h3 = _make_hue(self._tmp.name, german_names=True)
        res3 = ai_mod.execute_ask_ai(h3, "Szene Abendstimmung im Wohnzimmer")
        self.assertTrue(res3["ok"])
        self.assertIn(("scene", SC_MOVIE), h3.pending)


class TestSchedules(BaseAITest):
    def test_time_and_weekdays_en_and_de(self):
        h = _make_hue(self._tmp.name, schedule_mode="local")
        res = ai_mod.execute_ask_ai(h, "schedule Living Room on at 07:30 on weekdays at 80%")
        self.assertTrue(res["ok"])
        self.assertEqual(len(h.local_schedules), 1)
        s = h.local_schedules[0]
        self.assertEqual(s["target_kind"], "room")
        self.assertEqual(s["target_id"], ROOM_LIVING)
        self.assertEqual(s["time"], "07:30")
        self.assertEqual(s["days"], ["mon", "tue", "wed", "thu", "fri"])
        self.assertEqual(s["action"], "on")
        self.assertEqual(s["bri"], 80)

        h2 = _make_hue(self._tmp.name, german_names=True, schedule_mode="local")
        res2 = ai_mod.execute_ask_ai(h2, "Schalte Wohnzimmer werktags um 07:30 auf 75% ein")
        self.assertTrue(res2["ok"])
        self.assertEqual(len(h2.local_schedules), 1)
        s2 = h2.local_schedules[0]
        self.assertEqual(s2["time"], "07:30")
        self.assertEqual(s2["days"], ["mon", "tue", "wed", "thu", "fri"])
        self.assertEqual(s2["bri"], 75)

        h3 = _make_hue(self._tmp.name, german_names=True, schedule_mode="local")
        res3 = ai_mod.execute_ask_ai(h3, "Schalte Büro Montag und Mittwoch um 22 Uhr aus")
        self.assertTrue(res3["ok"])
        self.assertEqual(len(h3.local_schedules), 1)
        s3 = h3.local_schedules[0]
        self.assertEqual(s3["time"], "22:00")
        self.assertEqual(s3["days"], ["mon", "wed"])
        self.assertEqual(s3["action"], "off")

    def test_weekend_and_daily_schedules(self):
        h = _make_hue(self._tmp.name, schedule_mode="local")
        res = ai_mod.execute_ask_ai(h, "turn on Desk Lamp at 09:00 on weekend")
        self.assertTrue(res["ok"])
        self.assertEqual(h.local_schedules[0]["days"], ["sat", "sun"])

        res2 = ai_mod.execute_ask_ai(h, "turn off Office at 23:00 every day")
        self.assertTrue(res2["ok"])
        self.assertEqual(h.local_schedules[1]["days"], ["mon", "tue", "wed", "thu", "fri", "sat", "sun"])
        self.assertEqual(h.local_schedules[1]["action"], "off")

    def test_sunrise_and_sunset_schedules_en_and_de(self):
        # Even when schedule_mode is 'local', solar schedules must route to bridge API queue!
        h = _make_hue(self._tmp.name, schedule_mode="local")
        res = ai_mod.execute_ask_ai(h, "turn on Living Room at sunset")
        self.assertTrue(res["ok"])
        self.assertEqual(len(h.api_queue), 1)
        method, path, payload = h.api_queue[0]
        self.assertEqual(method, "POST")
        self.assertEqual(path, "/clip/v2/resource/behavior_instance")
        self.assertEqual(payload["configuration"]["when"]["time_point"]["type"], "sunset")

        h2 = _make_hue(self._tmp.name, german_names=True, schedule_mode="local")
        res2 = ai_mod.execute_ask_ai(h2, "Schalte Büro bei Sonnenaufgang aus")
        self.assertTrue(res2["ok"])
        self.assertEqual(len(h2.api_queue), 1)
        _, _, payload2 = h2.api_queue[0]
        self.assertEqual(payload2["configuration"]["when"]["time_point"]["type"], "sunrise")
        self.assertEqual(payload2["configuration"]["what"], [{"on": {"on": False}}])


class TestRules(BaseAITest):
    def test_lamp_to_lamp_follow_and_options(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(
            h,
            "when Sofa Lamp turns on between 22:00 and 06:00, turn on Floor Lamp only if off and match brightness and mirror off",
        )
        self.assertTrue(res["ok"])
        self.assertEqual(len(h.rules), 1)
        r = h.rules[0]
        self.assertEqual(r["trigger_kind"], "light")
        self.assertEqual(r["trigger_id"], L_SOFA)
        self.assertEqual(r["trigger_state"], "on")
        self.assertEqual(r["targets"][0]["id"], L_FLOOR)
        self.assertEqual(r["targets"][0]["action"], "on")
        self.assertTrue(r["mirror_off"])
        self.assertTrue(r["only_if_off"])
        self.assertTrue(r["match_bri"])
        self.assertEqual(r["time_start"], "22:00")
        self.assertEqual(r["time_end"], "06:00")

    def test_lamp_to_lamp_exclusive(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "when Sofa Lamp turns on, turn off Floor Lamp")
        self.assertTrue(res["ok"])
        self.assertEqual(len(h.rules), 1)
        r = h.rules[0]
        self.assertEqual(r["kind"], "exclusive")
        self.assertEqual(r["targets"][0]["action"], "off")

    def test_motion_sensor_rule_en_and_de(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "when Hallway Motion detects motion, turn on Desk Lamp at 35% and also turn off when clear")
        self.assertTrue(res["ok"])
        r = h.rules[0]
        self.assertEqual(r["kind"], "sensor")
        self.assertEqual(r["trigger_kind"], "motion")
        self.assertEqual(r["trigger_id"], S_MOT)
        self.assertEqual(r["trigger_state"], "motion")
        self.assertEqual(r["targets"][0]["id"], L_DESK)
        self.assertEqual(r["targets"][0]["bri"], 35)
        self.assertTrue(r["mirror_off"])

        h2 = _make_hue(self._tmp.name, german_names=True)
        res2 = ai_mod.execute_ask_ai(h2, "Wenn Flur Bewegung keine Bewegung erkennt, schalte Büro aus")
        self.assertTrue(res2["ok"])
        r2 = h2.rules[0]
        self.assertEqual(r2["trigger_kind"], "motion")
        self.assertEqual(r2["trigger_state"], "clear")
        self.assertEqual(r2["targets"][0]["kind"], "room")
        self.assertEqual(r2["targets"][0]["id"], ROOM_OFFICE)
        self.assertEqual(r2["targets"][0]["action"], "off")

    def test_contact_sensor_and_button_rules_en_and_de(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "when Front Door Contact opens, turn on Desk Lamp")
        self.assertTrue(res["ok"])
        r = h.rules[0]
        self.assertEqual(r["trigger_kind"], "contact")
        self.assertEqual(r["trigger_id"], S_CON)
        self.assertEqual(r["trigger_state"], "open")

        h2 = _make_hue(self._tmp.name, german_names=True)
        res2 = ai_mod.execute_ask_ai(h2, "Wenn Türkontakt schließt, schalte Schreibtischlampe aus")
        self.assertTrue(res2["ok"])
        r2 = h2.rules[0]
        self.assertEqual(r2["trigger_kind"], "contact")
        self.assertEqual(r2["trigger_state"], "closed")
        self.assertEqual(r2["targets"][0]["action"], "off")

        h3 = _make_hue(self._tmp.name)
        res3 = ai_mod.execute_ask_ai(h3, "when Wall Switch long press, turn off Living Room")
        self.assertTrue(res3["ok"])
        r3 = h3.rules[0]
        self.assertEqual(r3["trigger_kind"], "button")
        self.assertEqual(r3["trigger_id"], D_BTN)
        self.assertEqual(r3["trigger_state"], "long_press")

    def test_motion_aware_rule(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "when Living Room MotionAware detects motion, turn on Sofa Lamp")
        self.assertTrue(res["ok"])
        r = h.rules[0]
        self.assertEqual(r["kind"], "sensor")
        self.assertEqual(r["trigger_kind"], "motion_area")
        self.assertEqual(r["trigger_id"], MAC_LIVING)
        self.assertEqual(r["trigger_state"], "motion")
        self.assertEqual(r["targets"][0]["id"], L_SOFA)

    def test_contradictory_cycle_rejected(self):
        h = _make_hue(self._tmp.name)
        res1 = ai_mod.execute_ask_ai(h, "when Sofa Lamp turns on, turn on Floor Lamp")
        self.assertTrue(res1["ok"])
        res2 = ai_mod.execute_ask_ai(h, "when Floor Lamp turns on, turn off Sofa Lamp")
        self.assertFalse(res2["ok"])
        self.assertEqual(len(h.rules), 1)


class TestSensorsStatusAndMultiClause(BaseAITest):
    def test_sensor_and_motion_aware_toggles_en_and_de(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "disable Hallway Motion")
        self.assertTrue(res["ok"])
        self.assertEqual(h.api_queue[-1], ("PUT", "/clip/v2/resource/motion/%s" % S_MOT, {"enabled": False}))

        res2 = ai_mod.execute_ask_ai(h, "enable Living Room MotionAware")
        self.assertTrue(res2["ok"])
        self.assertEqual(
            h.api_queue[-1],
            ("PUT", "/clip/v2/resource/motion_area_configuration/%s" % MAC_LIVING, {"enabled": True}),
        )

        res3 = ai_mod.execute_ask_ai(h, "set Living Room MotionAware sensitivity to 2")
        self.assertTrue(res3["ok"])
        self.assertEqual(
            h.api_queue[-1],
            ("PUT", "/clip/v2/resource/convenience_area_motion/%s" % CONV_LIVING, {"sensitivity": {"sensitivity": 2}}),
        )

        h_de = _make_hue(self._tmp.name, german_names=True)
        res_de = ai_mod.execute_ask_ai(h_de, "deaktiviere Türkontakt")
        self.assertTrue(res_de["ok"])
        self.assertEqual(h_de.api_queue[-1], ("PUT", "/clip/v2/resource/contact/%s" % S_CON, {"enabled": False}))

    def test_status_queries_en_and_de(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "which lights are on?")
        self.assertTrue(res["ok"])
        self.assertEqual(res["actions"], ["status"])
        self.assertIn("Sofa Lamp", res["reply"])
        self.assertIn("Hallway Motion", res["reply"])
        self.assertIn("Front Door Contact", res["reply"])

        h_de = _make_hue(self._tmp.name, german_names=True)
        res_de = ai_mod.execute_ask_ai(h_de, "welche lichter sind an?")
        self.assertTrue(res_de["ok"])
        self.assertIn("Lichter an", res_de["reply"])
        self.assertIn("Sofa Lampe", res_de["reply"])

        res_s = ai_mod.execute_ask_ai(h, "status")
        self.assertTrue(res_s["ok"])
        self.assertEqual(res_s["actions"], ["status"])

    def test_multi_clause_commands(self):
        h = _make_hue(self._tmp.name)
        res = ai_mod.execute_ask_ai(h, "turn on Living Room and turn off Desk Lamp")
        self.assertTrue(res["ok"])
        self.assertEqual(len(res["actions"]), 2)
        self.assertEqual(h.pending[("grouped_light", GL_LIVING)]["on"], {"on": True})
        self.assertEqual(h.pending[("light", L_DESK)]["on"], {"on": False})

        # Target carry-over across 'and'
        h2 = _make_hue(self._tmp.name)
        res2 = ai_mod.execute_ask_ai(h2, "turn on Sofa Lamp and dim to 40%")
        self.assertTrue(res2["ok"])
        self.assertEqual(h2.pending[("light", L_SOFA)]["dimming"]["brightness"], 40.0)

        # Shared trailing action across two targets
        h3 = _make_hue(self._tmp.name)
        res3 = ai_mod.execute_ask_ai(h3, "Sofa Lamp and Desk Lamp off")
        self.assertTrue(res3["ok"])
        self.assertEqual(h3.pending[("light", L_SOFA)]["on"], {"on": False})
        self.assertEqual(h3.pending[("light", L_DESK)]["on"], {"on": False})

        # German multi-clause
        h_de = _make_hue(self._tmp.name, german_names=True)
        res_de = ai_mod.execute_ask_ai(h_de, "Wohnzimmer auf 50% und deaktiviere Flur Bewegung")
        self.assertTrue(res_de["ok"])
        self.assertEqual(h_de.pending[("grouped_light", GL_LIVING)]["dimming"]["brightness"], 50.0)
        self.assertEqual(h_de.api_queue[-1], ("PUT", "/clip/v2/resource/motion/%s" % S_MOT, {"enabled": False}))


class TestSecurityAndOllama(BaseAITest):
    def test_reject_non_loopback_ollama_urls(self):
        bad_urls = [
            "http://192.168.1.50:11434",
            "http://10.0.0.2:11434",
            "http://evil.example.com:11434",
            "https://127.0.0.1:11434",
            "http://user:pass@127.0.0.1:11434",
            "file:///etc/passwd",
            "http://127.0.0.1:11434/?foo=bar",
        ]
        for url in bad_urls:
            with self.assertRaises(ValueError, msg=url):
                ai_mod.validate_ollama_url(url)

        for good_url, exp_host, exp_port in (
            ("http://127.0.0.1:11434", "127.0.0.1", 11434),
            ("http://localhost:11434", "localhost", 11434),
            ("http://localhost", "localhost", 11434),
            ("http://[::1]:11434", "::1", 11434),
        ):
            host, port, _ = ai_mod.validate_ollama_url(good_url)
            self.assertEqual((host, port), (exp_host, exp_port))

        # Verify execute_ask_ai rejects non-loopback URL without touching sockets
        h = _make_hue(self._tmp.name)
        with mock.patch.object(ai_mod, "_probe_tcp") as m_probe:
            res = ai_mod.execute_ask_ai(
                h,
                "prepare something mysterious for dinner party",
                ollama_url="http://192.168.1.99:11434",
                ollama_model="llama3",
            )
            self.assertFalse(res["ok"])
            self.assertEqual(res["engine"], "ollama")
            self.assertIn("loopback", res["reply"].lower())
            m_probe.assert_not_called()

    def test_ollama_fallback_never_leaks_secrets_and_validates_commands(self):
        h = _make_hue(self._tmp.name)
        captured_bodies = []

        def fake_http_json(host, port, method, path, body=None, timeout=15.0):
            captured_bodies.append(body)
            return {
                "message": {
                    "content": json.dumps({
                        "reply": "Set cozy mood (leaked %s)" % SECRET_APP_KEY,
                        "commands": [
                            {"cmd": "unpair"},  # Must be blocked by allowlist!
                            {"cmd": "light", "id": L_SOFA, "bri": 25, "hex": "#ff8822"},
                        ],
                    })
                }
            }

        with mock.patch.object(ai_mod, "_probe_tcp", return_value=True), \
             mock.patch.object(ai_mod, "_http_json", side_effect=fake_http_json):
            res = ai_mod.execute_ask_ai(
                h,
                "create a mysterious cozy midnight ambiance",
                ollama_url="http://127.0.0.1:11434",
                ollama_model="qwen2.5:7b",
            )

        self.assertTrue(res["ok"])
        self.assertEqual(res["engine"], "ollama")
        # Secret must NEVER appear in request sent to Ollama or in returned reply
        sent_blob = json.dumps(captured_bodies)
        self.assertNotIn(SECRET_APP_KEY, sent_blob)
        self.assertNotIn(SECRET_CLIENT_KEY, sent_blob)
        self.assertNotIn(SECRET_APP_KEY, res["reply"])
        # Bridge must NOT be unpaired, and valid light command must have executed
        self.assertTrue(bool(h.config))
        self.assertIn(("light", L_SOFA), h.pending)
        self.assertEqual(h.pending[("light", L_SOFA)]["dimming"]["brightness"], 25.0)

    def test_offline_probe_does_not_stall(self):
        # Grab an ephemeral unused localhost port
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.bind(("127.0.0.1", 0))
        closed_port = sock.getsockname()[1]
        sock.close()

        h = _make_hue(self._tmp.name)
        t0 = time.monotonic()
        res = ai_mod.execute_ask_ai(
            h,
            "completely unrecognized gibberish xyzzy",
            ollama_url="http://127.0.0.1:%d" % closed_port,
            ollama_model="llama3",
        )
        elapsed = time.monotonic() - t0
        self.assertFalse(res["ok"])
        self.assertLess(elapsed, 0.8)

    def test_empty_prompt_and_unpaired_bridge(self):
        h = _make_hue(self._tmp.name, paired=False)
        res_empty = ai_mod.execute_ask_ai(h, "   ")
        self.assertFalse(res_empty["ok"])

        res_unpaired = ai_mod.execute_ask_ai(h, "all lights off")
        self.assertFalse(res_unpaired["ok"])
        self.assertIn("paired", res_unpaired["reply"].lower())


class TestHueHandleAiCommands(BaseAITest):
    def test_hue_handle_ai_ask_updates_snapshot_and_ai_clear_resets(self):
        h = _make_hue(self._tmp.name)
        snaps = []
        h.emit = lambda: snaps.append(h.snapshot())

        # Initial snapshot exposes clean ai dict
        init_ai = h.snapshot()["ai"]
        self.assertFalse(init_ai["running"])
        self.assertEqual(init_ai["last_prompt"], "")
        self.assertEqual(init_ai["last_reply"], "")
        self.assertEqual(init_ai["last_actions"], [])
        self.assertEqual(init_ai["error"], "")

        # Empty prompt is rejected cleanly without spawning background work
        h.handle({"cmd": "ai_ask", "prompt": "   "})
        err_ai = h.snapshot()["ai"]
        self.assertFalse(err_ai["running"])
        self.assertTrue(bool(err_ai["error"]))

        # Valid prompt runs asynchronously, sets running=True then completes with reply & actions
        snaps.clear()
        h.handle({"cmd": "ai_ask", "prompt": "all lights off"})
        self.assertGreaterEqual(len(snaps), 1)
        self.assertTrue(snaps[0]["ai"]["running"])
        self.assertEqual(snaps[0]["ai"]["last_prompt"], "all lights off")

        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline:
            if not h.snapshot()["ai"]["running"]:
                break
            time.sleep(0.01)

        done_ai = h.snapshot()["ai"]
        self.assertFalse(done_ai["running"])
        self.assertEqual(done_ai["last_prompt"], "all lights off")
        self.assertTrue(bool(done_ai["last_reply"]))
        self.assertIn("all_off", done_ai["last_actions"])
        self.assertEqual(done_ai["engine"], "builtin")
        self.assertEqual(done_ai["error"], "")
        self.assertIn(("grouped_light", GL_LIVING), h.pending)
        self.assertEqual(h.pending[("grouped_light", GL_LIVING)]["on"], {"on": False})

        # ai_clear resets reply, actions, error, and prompt
        h.handle({"cmd": "ai_clear"})
        cleared = h.snapshot()["ai"]
        self.assertFalse(cleared["running"])
        self.assertEqual(cleared["last_prompt"], "")
        self.assertEqual(cleared["last_reply"], "")
        self.assertEqual(cleared["last_actions"], [])
        self.assertEqual(cleared["error"], "")


if __name__ == "__main__":
    unittest.main()
