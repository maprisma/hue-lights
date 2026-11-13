"""Pure desktop Exec encoding checks; never run the installer."""
import pathlib
import subprocess
import unittest


class DesktopExecTests(unittest.TestCase):
    def test_quoted_full_path(self):
        installer = (pathlib.Path(__file__).resolve().parents[1] / "install.sh").read_text()
        function = installer.split("desktop_exec() {", 1)[1].split("\n}\n", 1)[0]
        source = 'desktop_exec() {' + function + '\n}\ndesktop_exec "$1"'
        for path in ['/home/a/.local/bin/app', '/tmp/a b/"quote"/`tick`/$dollar/100%/back\\slash']:
            with self.subTest(path=path):
                result = subprocess.run(['bash', '-c', source, 'test', path], check=True,
                                        text=True, capture_output=True).stdout
                # Encode each argument character, then encode the desktop value layer.
                argument = ''.join('\\' + c if c in '\\"`$' else '%%' if c == '%' else c
                                   for c in path)
                self.assertEqual(result, '"' + argument.replace('\\', '\\\\') + '"')


class TrayIconTests(unittest.TestCase):
    def test_off_bulb_is_installed_under_the_trays_name(self):
        root = pathlib.Path(__file__).resolve().parents[1]
        installer = (root / "install.sh").read_text()
        lib = (root / "src" / "lib.rs").read_text()
        self.assertIn('pub const ICON_OFF: &str = "io.github.ubruckhaus.CosmicMaprismaHueLights-off-symbolic"', lib)
        self.assertIn('data/icons/hue-bulb-off-symbolic.svg "$data/icons/hicolor/scalable/apps/$id-off-symbolic.svg"',
                      installer)
        self.assertTrue((root / "data" / "icons" / "hue-bulb-off-symbolic.svg").is_file())

    def test_lit_bulb_is_green_like_every_maprisma_bar_icon(self):
        root = pathlib.Path(__file__).resolve().parents[1]
        installer = (root / "install.sh").read_text()
        self.assertIn('data/icons/hue-bulb-active-symbolic.svg "$data/icons/hicolor/scalable/apps/$id-active-symbolic.svg"',
                      installer)
        svg = (root / "data" / "icons" / "hue-bulb-active-symbolic.svg").read_text()
        self.assertIn('class="ColorScheme-PositiveText"', svg)
        self.assertNotIn('ColorScheme-Text', svg)


if __name__ == '__main__':
    unittest.main()
