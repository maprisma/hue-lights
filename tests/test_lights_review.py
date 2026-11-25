"""Regression tests for the Lights 1.0.0 review fixes."""
import importlib.util
import json
import os
import shutil
import stat
import tempfile
import threading
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))


def _load(name, rel):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, "..", "scripts", rel))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


prov = _load("lights_review_prov", "hue_providers.py")
hue = _load("lights_review_hue", "hue-lights.py")


class LanGuardTests(unittest.TestCase):
    def test_only_rfc1918_and_link_local(self):
        for ok in ("10.1.2.3", "172.16.0.9", "172.31.255.1", "192.168.178.20", "169.254.10.10"):
            self.assertTrue(prov._is_allowed_ipv4(ok), ok)
        for bad in ("198.18.0.1", "100.64.0.1", "192.0.0.8", "192.0.2.1", "0.1.2.3", "8.8.8.8",
                    "169.254.169.254", "172.32.0.1", "224.0.0.1", "255.255.255.255", "127.0.0.1", None, 42):
            self.assertFalse(prov._is_allowed_ipv4(bad), bad)
        self.assertTrue(prov._is_allowed_ipv4("127.0.0.1", allow_loopback=True))
        self.assertFalse(prov.valid_local_host("198.18.0.1"))

    def test_resolved_name_outside_lan_is_blocked(self):
        fake = [(2, 1, 6, "", ("198.18.0.1", 80))]
        with mock.patch.object(prov.socket, "getaddrinfo", return_value=fake):
            with self.assertRaises(prov.ProviderError):
                prov.resolve_local_ipv4("lamp.local", 80)


class ProvidersFileTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        os.chmod(self.dir, 0o700)
        self.path = os.path.join(self.dir, "providers.json")

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def _write(self, data):
        with open(self.path, "w") as fh:
            fh.write(data)
        os.chmod(self.path, 0o600)

    def test_fifo_does_not_block_and_blocks_overwrite(self):
        os.mkfifo(self.path, 0o600)
        done = []
        t = threading.Thread(target=lambda: done.append(prov.ProviderManager(self.dir).load()), daemon=True)
        t.start()
        t.join(3)
        self.assertTrue(done, "load() blocked on a FIFO")
        mgr = prov.ProviderManager(self.dir)
        mgr.load()
        self.assertTrue(mgr._load_blocked)

    def test_wrong_container_types_do_not_raise(self):
        for payload in ('{"providers": null}', '{"providers": 5}', '{"providers": {"a": 1}}', '"text"', '7',
                        '{"providers": [null, 3, "x", [], {"kind": 5}]}', "[" * 5000 + "]" * 5000):
            self._write(payload)
            mgr = prov.ProviderManager(self.dir)
            mgr.load()  # must never raise
            self.assertEqual(mgr.connections, [])

    def test_oversized_file_is_refused(self):
        self._write('{"providers": []}' + " " * 300000)
        mgr = prov.ProviderManager(self.dir)
        mgr.load()
        self.assertTrue(mgr._load_blocked)


class BridgeIpTests(unittest.TestCase):
    def test_bridge_ip_only_home_lan(self):
        for ok in ("192.168.178.106", "10.0.0.2", "172.20.1.1", "169.254.3.4"):
            self.assertTrue(hue.valid_bridge_ip(ok), ok)
        for bad in ("198.18.0.1", "100.64.1.1", "192.0.0.9", "192.0.2.5", "0.0.0.1", "169.254.169.254"):
            self.assertFalse(hue.valid_bridge_ip(bad), bad)


class PendingCommandTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        os.chmod(self.dir, 0o700)
        self.mgr = prov.ProviderManager(self.dir)
        self.mgr.connections = [{"id": "c1", "kind": "wled", "host": "192.168.1.20", "port": 80,
                                 "name": "Strip", "room_name": "", "token": "", "use_https": False,
                                 "enabled": True, "cert_fingerprint": ""}]
        self.mgr.conn_generations = {"c1": 1}
        self.mgr.results_gen = {"c1": 1}
        self.sent = []
        real_set_light = self.mgr.set_light

        def spy(rid, body, gen=None):
            with self.mgr.lock:
                conn = self.mgr._conn_for(self.mgr._rid_conn_id(rid), gen)
            if conn:
                self.sent.append((rid, body))
        self.mgr.set_light = spy
        self._real_set_light = real_set_light

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def test_commands_dropped_on_edit_toggle_remove_and_generation_change(self):
        for action in ("toggle", "remove", "bump"):
            self.mgr.conn_generations = {"c1": 1}
            self.mgr.queue_command("light", "wled:c1:0", {"on": {"on": True}})
            with self.mgr.lock:
                if action == "toggle":
                    self.mgr.conn_generations["c1"] += 1
                    self.mgr._drop_pending_for("c1")
                elif action == "remove":
                    self.mgr._drop_pending_for("c1")
                else:
                    self.mgr.conn_generations["c1"] += 1  # edited between queue and drain
            while self.mgr._drain_pending():
                pass
            self.assertEqual(self.sent, [], action)

    def test_current_generation_is_sent(self):
        self.mgr.queue_command("light", "wled:c1:0", {"on": {"on": True}})
        self.mgr.queue_command("light", "wled:c1:0", {"dimming": {"brightness": 50}})
        self.assertTrue(self.mgr._drain_pending())
        self.assertEqual(self.sent, [("wled:c1:0", {"on": {"on": True}, "dimming": {"brightness": 50}})])

    def test_edit_after_dequeue_is_dropped_inside_set_light(self):
        self.mgr.queue_command("light", "wled:c1:0", {"on": {"on": True}})
        sent_http = []
        with self.mgr.lock:
            key, (gen, body) = next(iter(self.mgr._pending_cmds.items()))
            del self.mgr._pending_cmds[key]
            self.mgr.conn_generations["c1"] += 1  # host edited after the command left the queue
        with mock.patch.object(prov, "_http_request", side_effect=lambda *a, **k: sent_http.append(a) or (200, b"{}")):
            self._real_set_light("wled:c1:0", body, gen=gen)
        self.assertEqual(sent_http, [])

    def test_edit_waits_for_in_flight_send(self):
        import time
        started, order = threading.Event(), []
        self.mgr.save = lambda: None

        def slow_http(*a, **k):
            started.set()
            time.sleep(0.3)
            order.append("sent")
            return 200, b"{}"

        with mock.patch.object(prov, "_http_request", side_effect=slow_http):
            t = threading.Thread(target=lambda: self._real_set_light("wled:c1:0", {"on": {"on": True}}, gen=1))
            t.start()
            started.wait(2)
            self.mgr.remove_connection("c1")
            order.append("removed")
            t.join(3)
        self.assertEqual(order, ["sent", "removed"])

    def test_lights_from_previous_target_are_not_commandable(self):
        self.mgr.save = lambda: None
        self.mgr.results["c1"] = {"rooms": [], "sensors": [], "light_count": 1, "on_count": 0}
        edited = dict(self.mgr.connections[0], host="192.168.1.99")
        with mock.patch.object(self.mgr, "_resolve_saved_or_paired_secret", return_value=edited):
            self.mgr.save_connection(edited)
        self.assertNotIn("c1", self.mgr.results)
        self.assertFalse(self.mgr.queue_command("light", "wled:c1:0", {"on": {"on": True}}))
        self.assertFalse(self.mgr._drain_pending())
        self.assertEqual(self.sent, [])

    def test_concurrent_saves_are_serialized(self):
        import time
        mgr = prov.ProviderManager(self.dir)
        mgr.connections = [dict(self.mgr.connections[0])]
        inside, peak = [0], [0]
        real_replace = os.replace

        def slow_replace(a, b):
            inside[0] += 1
            peak[0] = max(peak[0], inside[0])
            time.sleep(0.05)
            inside[0] -= 1
            return real_replace(a, b)

        with mock.patch.object(prov.os, "replace", side_effect=slow_replace):
            ts = [threading.Thread(target=mgr.save) for _ in range(4)]
            for t in ts:
                t.start()
            for t in ts:
                t.join(5)
        self.assertEqual(peak[0], 1)
        with open(os.path.join(self.dir, "providers.json")) as fh:
            self.assertEqual(json.load(fh)["providers"][0]["id"], "c1")

    def test_unknown_ids_share_one_lock(self):
        for i in range(50):
            self.mgr._send_lock("x%d" % i)
        self.assertNotIn("x1", self.mgr._send_locks)

    def test_remove_connection_drops_queue(self):
        self.mgr.queue_command("light", "wled:c1:0", {"on": {"on": False}})
        self.mgr.remove_connection("c1")
        self.assertFalse(self.mgr._drain_pending())
        self.assertEqual(self.sent, [])


class ProviderActionGateTests(unittest.TestCase):
    def test_only_one_test_or_pair_thread_at_a_time(self):
        h = hue.Hue.__new__(hue.Hue)
        h._provider_action_gate = threading.Lock()
        release = threading.Event()
        calls = []

        class Mgr:
            last_action = {}

            def test_connection(self, raw):
                calls.append(raw)
                release.wait(3)
                return "ok"

        h.providers_mgr = Mgr()
        h._providers_mod = prov
        h.emit = lambda: None
        h.redact = lambda t: t
        for _ in range(20):
            h.handle({"cmd": "test_provider", "provider": {}})
        h.handle({"cmd": "pair_provider", "kind": "nanoleaf", "host": "192.168.1.9", "port": "x"})
        h.handle({"cmd": "pair_provider", "kind": "nanoleaf", "host": "192.168.1.9", "port": 99999})
        release.set()
        import time
        for _ in range(50):
            if not h._provider_action_gate.locked():
                break
            time.sleep(0.02)
        self.assertEqual(len(calls), 1)
        self.assertFalse(h._provider_action_gate.locked())


class HttpDeadlineTests(unittest.TestCase):
    def test_dripping_server_is_cut_at_the_deadline(self):
        import socket as _s
        import time
        srv = _s.socket()
        srv.bind(("127.0.0.1", 0))
        srv.listen(1)
        port = srv.getsockname()[1]
        stop = threading.Event()

        def drip():
            c, _ = srv.accept()
            try:
                c.recv(4096)
                c.sendall(b"HTTP/1.1 200 OK\r\n")
                while not stop.is_set():
                    c.sendall(b"X-A: b\r\n")
                    time.sleep(0.2)
            except OSError:
                pass
            finally:
                c.close()

        t = threading.Thread(target=drip, daemon=True)
        t.start()
        start = time.monotonic()
        with self.assertRaises((prov.ProviderError, OSError)):
            prov._http_request("127.0.0.1", port, "GET", "/api/states", timeout=1.0, allow_loopback=True)
        self.assertLess(time.monotonic() - start, 3.0)
        stop.set()
        srv.close()


    def test_dripping_body_after_connection_close_is_cut(self):
        import socket as _s
        import time
        srv = _s.socket()
        srv.bind(("127.0.0.1", 0))
        srv.listen(1)
        port = srv.getsockname()[1]
        stop = threading.Event()

        def drip():
            c, _ = srv.accept()
            try:
                c.recv(4096)
                c.sendall(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                while not stop.is_set():
                    c.sendall(b"x")
                    time.sleep(0.2)
            except OSError:
                pass
            finally:
                c.close()

        threading.Thread(target=drip, daemon=True).start()
        start = time.monotonic()
        with self.assertRaises((prov.ProviderError, OSError)):
            prov._http_request("127.0.0.1", port, "GET", "/json", timeout=1.0, allow_loopback=True)
        self.assertLess(time.monotonic() - start, 3.0)
        stop.set()
        srv.close()


class RemoveAndTestIsolationTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        os.chmod(self.dir, 0o700)
        self.mgr = prov.ProviderManager(self.dir)
        self.mgr.connections = [{"id": "c1", "kind": "wled", "host": "192.168.1.20", "port": 80,
                                 "name": "Strip", "room_name": "", "token": "", "use_https": False,
                                 "enabled": True, "cert_fingerprint": ""}]
        self.mgr.conn_generations = {"c1": 1}

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def test_unknown_ids_leave_no_state(self):
        for i in range(100):
            self.assertFalse(self.mgr.remove_connection("ghost%d" % i))
        self.assertEqual(set(self.mgr.conn_generations), {"c1"})
        self.assertFalse(os.path.exists(os.path.join(self.dir, "providers.json")))

    def test_removed_device_state_is_released(self):
        self.mgr._send_lock("c1")
        self.mgr.remove_connection("c1")
        self.assertNotIn("c1", self.mgr.conn_generations)
        self.assertNotIn("c1", self.mgr._send_locks)

    def test_new_devices_never_reuse_a_panel_id(self):
        self.mgr.save = lambda: None
        raw = dict(self.mgr.connections[0], id="c9", host="192.168.1.30")
        with mock.patch.object(self.mgr, "_resolve_saved_or_paired_secret", return_value=dict(raw)):
            saved = self.mgr.save_connection(raw)
        self.assertNotEqual(saved["id"], "c9")

    def test_connection_test_does_not_touch_saved_caches(self):
        self.mgr.wled_effects["c1"] = ["Solid", "Rainbow"]
        other = dict(self.mgr.connections[0], host="192.168.1.77")
        reply = json.dumps({"state": {"on": True, "bri": 100, "seg": []}, "info": {"name": "Other"},
                            "effects": ["Solid", "Fire"]}).encode()
        with mock.patch.object(self.mgr, "_resolve_saved_or_paired_secret", return_value=other), \
                mock.patch.object(prov, "_http_request", return_value=(200, reply)):
            try:
                self.mgr.test_connection(other)
            except prov.ProviderError:
                pass
        self.assertEqual(self.mgr.wled_effects["c1"], ["Solid", "Rainbow"])

    def test_stale_poll_does_not_write_caches(self):
        reply = json.dumps({"state": {"on": True, "bri": 100, "seg": []}, "info": {"name": "S"},
                            "effects": ["Solid", "Fire"]}).encode()
        with mock.patch.object(prov, "_http_request", return_value=(200, reply)):
            try:
                self.mgr.fetch_single(dict(self.mgr.connections[0]), gen=0)  # generation already moved on
            except Exception:
                pass
        self.assertNotIn("c1", self.mgr.wled_effects)


class HomeAssistantPinTests(unittest.TestCase):
    def test_area_request_uses_fingerprint_captured_by_first_request(self):
        mgr = prov.ProviderManager(tempfile.mkdtemp())
        conn = {"id": "h1", "kind": "home_assistant", "host": "192.168.1.5", "port": 8123, "token": "t0k",
                "use_https": True, "cert_fingerprint": "", "name": "HA", "room_name": "", "enabled": True}
        calls = []

        def fake_http(host, port, method, path, **kw):
            calls.append((path, kw.get("expected_fp")))
            if path == "/api/states":
                if isinstance(kw.get("capture_fp"), list):
                    kw["capture_fp"][:] = ["a" * 64]
                return 200, b"[]"
            return 200, b"[]"

        with mock.patch.object(prov, "_http_request", side_effect=fake_http):
            mgr.fetch_single(conn, gen=None)
        self.assertEqual(calls[0], ("/api/states", ""))
        self.assertEqual(calls[1], ("/api/template", "a" * 64))


class FloorplanBackgroundTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.cfg = os.path.join(self.dir, "cfg")
        self.saved = (hue.CONFIG_DIR, hue.CONFIG_PATH)
        hue.CONFIG_DIR = self.cfg
        hue.CONFIG_PATH = os.path.join(self.cfg, "config.json")

    def tearDown(self):
        hue.CONFIG_DIR, hue.CONFIG_PATH = self.saved
        shutil.rmtree(self.dir, ignore_errors=True)

    def _png(self, name, w, h):
        import struct
        import zlib
        raw = b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR" + struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
        raw += struct.pack(">I", zlib.crc32(raw[12:29]) & 0xFFFFFFFF) + b"\x00" * 32
        path = os.path.join(self.dir, name)
        with open(path, "wb") as fh:
            fh.write(raw)
        return path

    def test_decompression_bomb_header_is_refused(self):
        with self.assertRaises(ValueError):
            hue.set_floorplan_background(self._png("bomb.png", 60000, 60000))

    def test_only_managed_names_are_removed(self):
        os.makedirs(self.cfg, mode=0o700)
        keep = os.path.join(self.cfg, "floorplan_bg.bak")
        with open(keep, "w") as fh:
            fh.write("user file")
        hue.set_floorplan_background(self._png("ok.png", 800, 600))
        hue.clear_floorplan_background()
        self.assertTrue(os.path.exists(keep))
        self.assertFalse(os.path.exists(os.path.join(self.cfg, "floorplan_bg.png")))


if __name__ == "__main__":
    unittest.main()
