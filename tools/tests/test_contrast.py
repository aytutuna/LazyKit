import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import contrast


class ContrastTests(unittest.TestCase):
    def test_black_on_white(self):
        self.assertAlmostEqual(contrast.ratio("#000000", "#FFFFFF"), 21.0)

    def test_ten_values_all_pass(self):
        rows = contrast.table()
        self.assertEqual(len(rows), 10)
        for app, surface, text, rt, control, rc in rows:
            self.assertGreaterEqual(rt, contrast.TEXT, (app, surface))
            self.assertGreaterEqual(rc, contrast.CONTROL, (app, surface))

    def test_a_passing_color_is_kept(self):
        self.assertEqual(contrast.closest("#E8930C", "#1C1C1E", contrast.TEXT), "#E8930C")

    def test_a_failing_color_moves_toward_readable(self):
        shade = contrast.closest("#E8930C", "#FFFFFF", contrast.TEXT)
        self.assertNotEqual(shade, "#E8930C")
        self.assertGreaterEqual(contrast.ratio(shade, "#FFFFFF"), contrast.TEXT)


if __name__ == "__main__":
    unittest.main()
