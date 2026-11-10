"""Check literal gettext keys, placeholders, desktop metadata and bundled catalogs."""
import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

class Translations(unittest.TestCase):
    def test_catalog_coverage(self):
        keys = set()
        for path in (ROOT / 'src').rglob('*.rs'):
            keys.update(json.loads('"' + match + '"') for match in re.findall(r'\btrf?\(\s*"((?:[^"\\]|\\.)*)"', path.read_text()))
        bundled = json.loads((ROOT / 'translations/bundled.json').read_text())
        for code in ('en', 'de', 'fr', 'es', 'it', 'pt', 'nl', 'pl'):
            base = json.loads((ROOT / f'translations/{code}.json').read_text())
            extra = json.loads((ROOT / f'translations/extra/{code}.json').read_text())
            combined = {**base, **extra}
            self.assertEqual(keys - combined.keys(), set(), code)
            self.assertEqual(combined, bundled[code], f'Rerun tools/gen-translations: {code}')
            for key in keys:
                values = combined[key] if isinstance(combined[key], list) else [combined[key]]
                for value in values:
                    self.assertEqual(set(re.findall(r'\{\d+\}', key)), set(re.findall(r'\{\d+\}', value)), (code, key))

    def test_desktop_localizations(self):
        desktop = (ROOT / 'data/io.github.ubruckhaus.CosmicMaprismaHueLights.desktop').read_text()
        installer = (ROOT / 'install.sh').read_text()
        for code in ('de', 'fr', 'es', 'it', 'pt', 'nl', 'pl'):
            self.assertIn(f'Name[{code}]=', desktop)
            self.assertIn(f'Comment[{code}]=', desktop)
            self.assertIn(f'Name[{code}]=', installer)

if __name__ == '__main__':
    unittest.main()
