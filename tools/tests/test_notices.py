import shutil
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import notices

METADATA = {
    "workspace_members": ["own 0.1.0 (path+file:///x)"],
    "packages": [
        {"id": "own 0.1.0 (path+file:///x)", "name": "own", "version": "0.1.0", "license": None, "repository": None},
        {"id": "zeta 2.0.0", "name": "zeta", "version": "2.0.0", "license": "MIT", "repository": "https://example.org/zeta"},
        {"id": "alpha 1.0.0", "name": "alpha", "version": "1.0.0", "license": "Apache-2.0 OR MIT", "repository": None},
    ],
}
PINS = """[[pin]]
name = "tool"
version = "1.2"
url = "https://example.org/tool.zip"
sha256 = "a"
licence = "MIT"

[[pin]]
name = "inner"
from = "tool"
sha256 = "b"
licence = "MIT"
"""
ROOT = Path(__file__).resolve().parent.parent.parent


class Notices(unittest.TestCase):
    def test_own_crates_are_left_out_and_the_rest_sorted(self):
        text = notices.render(METADATA, PINS)
        self.assertNotIn("own 0.1.0", text)
        self.assertLess(text.index("alpha 1.0.0"), text.index("zeta 2.0.0"))
        self.assertIn("alpha 1.0.0\n  Apache-2.0 OR MIT\n  https://crates.io/crates/alpha", text)
        self.assertIn("zeta 2.0.0\n  MIT\n  https://example.org/zeta", text)

    def test_only_pins_with_an_address_are_listed(self):
        text = notices.render(METADATA, PINS)
        self.assertIn("tool 1.2\n  MIT\n  https://example.org/tool.zip", text)
        self.assertNotIn("inner", text)

    @unittest.skipUnless(shutil.which("cargo"), "needs cargo")
    def test_the_committed_file_is_current(self):
        self.assertEqual(subprocess.run([sys.executable, "-I", str(ROOT / "tools" / "notices.py"), "--check", str(ROOT)]).returncode, 0)

    def test_the_committed_file_lists_the_inference_crates(self):
        text = (ROOT / "THIRD-PARTY-NOTICES.txt").read_text(encoding="utf-8")
        for name in ("tract-onnx ", "ort "):
            self.assertIn("
" + name, text)


if __name__ == "__main__":
    unittest.main()
