import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import check_pins

A, B = "a" * 64, "b" * 64
FETCH = f'url="https://example.org/pkg/tool/1.0"\nsum={A}\nother={B}\n'


def pin(name, sha, extra='url = "https://example.org/pkg/tool/1.0"', licence="MIT"):
    return f'[[pin]]\nname = "{name}"\nsha256 = "{sha}"\nlicence = "{licence}"\n{extra}\n'


class CheckPins(unittest.TestCase):
    def problems(self, pins, fetch=FETCH):
        return check_pins.check(pins, fetch)

    def test_agreement_is_clean(self):
        pins = pin("tool", A) + pin("inner", B, 'from = "tool"')
        self.assertEqual(self.problems(pins), [])

    def test_a_digest_the_script_lacks(self):
        pins = pin("tool", A) + pin("inner", "c" * 64, 'from = "tool"')
        self.assertTrue(any("inner: its digest is not in fetch.sh" in p for p in self.problems(pins)))

    def test_a_digest_without_a_pin(self):
        problems = self.problems(pin("tool", A))
        self.assertTrue(any(B in p and "no pin lists" in p for p in problems))

    def test_a_licence_off_the_allowlist(self):
        pins = pin("tool", A, licence="GPL-3.0-only") + pin("inner", B, 'from = "tool"')
        self.assertTrue(any("licence" in p for p in self.problems(pins)))

    def test_an_address_the_script_does_not_use(self):
        pins = pin("tool", A, 'url = "https://example.org/other/tool/9.9"') + pin("inner", B, 'from = "tool"')
        self.assertTrue(any("url" in p for p in self.problems(pins)))

    def test_a_pin_from_nothing(self):
        pins = pin("tool", A) + pin("inner", B, 'from = "missing"')
        self.assertTrue(any("neither a url" in p for p in self.problems(pins)))

    def test_the_real_files_agree(self):
        root = Path(__file__).resolve().parent.parent.parent
        self.assertEqual(check_pins.check((root / check_pins.PINS).read_text(encoding="utf-8"),
                                          (root / check_pins.FETCH).read_text(encoding="utf-8")), [])


if __name__ == "__main__":
    unittest.main()
