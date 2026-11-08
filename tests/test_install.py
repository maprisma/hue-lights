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


if __name__ == '__main__':
    unittest.main()
