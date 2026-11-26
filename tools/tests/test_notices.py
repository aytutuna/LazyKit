import shutil
import subprocess
import sys
import tempfile
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

    def test_equal_texts_are_listed_once_with_every_crate_that_has_them(self):
        texts = {
            ("zeta", "2.0.0"): [("LICENSE-MIT", "Copyright (c) A\r\nMIT text\n")],
            ("alpha", "1.0.0"): [("LICENSE", "Copyright (c) A\nMIT text"), ("NOTICE", "Apache notice")],
            ("beta", "1.0.0"): [("LICENSE", "Copyright (c) B\nMIT text")],
        }
        text = notices.render(METADATA, PINS, texts)
        self.assertEqual(text.count("MIT text"), 2)
        bar = "=" * 78
        self.assertIn(f"Used by: alpha 1.0.0, zeta 2.0.0\n{bar}\nCopyright (c) A\nMIT text\n", text)
        self.assertIn(f"Used by: alpha 1.0.0\n{bar}\nApache notice\n", text)
        self.assertIn("Used by: beta 1.0.0", text)

    def test_licence_files_are_read_from_each_crates_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "LICENSE-APACHE").write_text("Apache text", encoding="utf-8")
            (root / "COPYING").write_text("Copying text", encoding="utf-8")
            (root / "Cargo.toml").write_text("", encoding="utf-8")
            (root / "licensing.md").write_text("not a licence file", encoding="utf-8")
            metadata = {
                "workspace_members": [],
                "packages": [{"id": "x 1.0.0", "name": "x", "version": "1.0.0", "manifest_path": str(root / "Cargo.toml")}],
            }
            found = notices.licence_texts(metadata)
        self.assertEqual(found[("x", "1.0.0")], [("COPYING", "Copying text"), ("LICENSE-APACHE", "Apache text")])

    def test_the_native_components_are_named(self):
        text = notices.render(METADATA, PINS)
        self.assertIn("ONNX Runtime 1.24.2 (MIT)", text)
        self.assertIn("ThirdPartyNotices.txt", text)
        self.assertIn("DocAligner FastViT-T8 corner model 1.1.1 (Apache-2.0)", text)

    @unittest.skipUnless(shutil.which("cargo"), "needs cargo")
    def test_the_committed_file_is_current(self):
        self.assertEqual(subprocess.run([sys.executable, "-I", str(ROOT / "tools" / "notices.py"), "--check", str(ROOT)]).returncode, 0)

    def test_the_committed_file_lists_the_inference_crates(self):
        text = (ROOT / "THIRD-PARTY-NOTICES.txt").read_text(encoding="utf-8")
        for name in ("tract-onnx ", "ort "):
            self.assertIn("\n" + name, text)


if __name__ == "__main__":
    unittest.main()
