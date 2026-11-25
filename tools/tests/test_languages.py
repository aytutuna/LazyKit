import json
import unittest
from pathlib import Path

data = Path(__file__).resolve().parent.parent.parent / "testdata"
WAVE_1 = {"ja", "de", "fr", "zh-Hans", "ko", "es", "it", "pt-BR", "ru", "zh-Hant", "nl", "tr"}


class LanguageTests(unittest.TestCase):
    def setUp(self):
        self.rows = json.loads((data / "languages.json").read_text(encoding="utf8"))

    def test_fifty_localizations(self):
        self.assertEqual(len(self.rows), 50)

    def test_codes_are_unique_on_every_platform(self):
        for key in ("store", "apple", "android", "windows", "web"):
            values = [r[key] for r in self.rows]
            self.assertEqual(len(values), len(set(values)), key)

    def test_wave_one_is_the_twelve(self):
        self.assertEqual({r["apple"] for r in self.rows if r["wave"] == 1}, WAVE_1)

    def test_only_the_base_has_no_wave(self):
        self.assertEqual([r["apple"] for r in self.rows if r["wave"] not in (1, 2)], ["en"])

    def test_android_folders(self):
        for r in self.rows:
            self.assertTrue(r["android"] == "values" or r["android"].startswith(("values-b+", "values-iw", "values-in")), r)

    def test_android_uses_the_old_codes_for_hebrew_and_indonesian(self):
        folders = {r["apple"]: r["android"] for r in self.rows}
        self.assertEqual((folders["he"], folders["id"]), ("values-iw", "values-in"))

    def test_right_to_left(self):
        self.assertEqual({r["apple"] for r in self.rows if r["rtl"]}, {"ar", "he", "ur"})

    def test_glossary_covers_wave_one_only(self):
        glossary = json.loads((data / "glossary.json").read_text(encoding="utf8"))
        for term in glossary["terms"]:
            if term.get("keep"):
                continue
            self.assertEqual(set(term["tr"]), WAVE_1, term["id"])
            if term["id"] in ("share", "open-with", "settings", "quit"):
                for words in term["tr"].values():
                    self.assertEqual(set(words), {"windows", "apple", "android"}, term["id"])


if __name__ == "__main__":
    unittest.main()
