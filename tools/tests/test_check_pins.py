import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import check_pins

A, B = "a" * 64, "b" * 64
FETCH = f'url="https://example.org/pkg/tool/1.0"\nsum={A}\nother={B}\n'
# the shapes the real script uses: a variable per digest, one case line per desktop library, abi paths as jni/$abi/
REAL = (
    f'url="https://example.org/pkg/tool/1.0"\ntool_sha={A}\nabi_sha_x86={A[::-1]}\nabi_sha_arm={B[::-1]}\n'
    f'case x in\n  L/x) entry=runtimes/linux-x64/lib.so; lib_sha={B} ;;\n'
    f'  L/y) entry=runtimes/linux-arm64/lib.so; lib_sha={"c" * 64} ;;\nesac\n'
    'unzip "jni/$abi/lib.so"\n'
)


def pin(name, sha, extra='url = "https://example.org/pkg/tool/1.0"', licence="MIT"):
    tied = "var" in extra or "path" in extra
    var = "" if tied else {A: 'var = "sum"\n', B: 'var = "other"\n'}.get(sha, "")
    return f'[[pin]]\nname = "{name}"\nsha256 = "{sha}"\nlicence = "{licence}"\n{var}{extra}\n'


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

    def test_a_version_the_address_does_not_carry(self):
        url = 'url = "https://example.org/pkg/tool/1.0"\nversion = "2.0"'
        pins = pin("tool", A, url) + pin("inner", B, 'from = "tool"')
        self.assertTrue(any("tool: version 2.0" in p for p in self.problems(pins)))

    def test_a_matching_version_is_clean(self):
        url = 'url = "https://example.org/pkg/tool/1.0"\nversion = "1.0"'
        pins = pin("tool", A, url) + pin("inner", B, 'from = "tool"')
        self.assertEqual(self.problems(pins), [])

    def test_a_version_variable_no_pin_has(self):
        url = 'url = "https://example.org/pkg/tool/1.0"\nversion = "1.0"'
        pins = pin("tool", A, url) + pin("inner", B, 'from = "tool"')
        problems = self.problems(pins, FETCH + "tool_version=1.1\n")
        self.assertTrue(any("tool_version=1.1" in p for p in problems))

    def test_a_url_made_of_a_base_and_a_file_the_script_never_joins(self):
        fetch = f'base="https://example.org/a"\nfile=x.bin\nstray=y.bin\nsum={A}\nother={B}\ndownload "$base/$file" f {A}\n'
        good = pin("tool", A, 'url = "https://example.org/a/x.bin"') + pin("inner", B, 'from = "tool"')
        self.assertEqual(self.problems(good, fetch), [])
        bad = pin("tool", A, 'url = "https://example.org/a/y.bin"') + pin("inner", B, 'from = "tool"')
        self.assertTrue(any("tool: url https://example.org/a/y.bin" in p for p in self.problems(bad, fetch)))

    def test_a_pin_from_nothing(self):
        pins = pin("tool", A) + pin("inner", B, 'from = "missing"')
        self.assertTrue(any("neither a url" in p for p in self.problems(pins)))

    def real_pins(self, x86=A[::-1], arm=B[::-1], arm_path="jni/arm64-v8a/lib.so", linux_x64=B):
        url = 'url = "https://example.org/pkg/tool/1.0"\nvar = "tool_sha"'
        return (
            pin("tool", A, url)
            + pin("x86", x86, 'from = "tool"\npath = "jni/x86_64/lib.so"\nvar = "abi_sha_x86"')
            + pin("arm", arm, f'from = "tool"\npath = "{arm_path}"\nvar = "abi_sha_arm"')
            + pin("lx64", linux_x64, 'from = "tool"\npath = "runtimes/linux-x64/lib.so"')
            + pin("larm", "c" * 64, 'from = "tool"\npath = "runtimes/linux-arm64/lib.so"')
        )

    def test_pins_tied_to_their_variables_and_lines_are_clean(self):
        self.assertEqual(self.problems(self.real_pins(), REAL), [])

    def test_two_digests_swapped_between_variables(self):
        problems = self.problems(self.real_pins(x86=B[::-1], arm=A[::-1]), REAL)
        self.assertTrue(any("x86: fetch.sh does not set abi_sha_x86" in p for p in problems))
        self.assertTrue(any("arm: fetch.sh does not set abi_sha_arm" in p for p in problems))

    def test_a_digest_on_the_wrong_case_line(self):
        problems = self.problems(self.real_pins(linux_x64="c" * 64), REAL)
        self.assertTrue(any("lx64: no line of fetch.sh has both" in p for p in problems))

    def test_a_path_the_script_does_not_extract(self):
        problems = self.problems(self.real_pins(arm_path="jni/riscv/lib.so"), REAL)
        self.assertTrue(any("arm: path jni/riscv/lib.so" in p for p in problems))

    def test_a_pin_with_nothing_to_tie_it(self):
        pins = pin("tool", A) + pin("inner", "c" * 64, 'from = "tool"')
        problems = self.problems(pins, FETCH + "c" * 64 + "\n")
        self.assertTrue(any("inner: no var or path" in p for p in problems))

    def test_the_real_files_agree(self):
        root = Path(__file__).resolve().parent.parent.parent
        self.assertEqual(check_pins.check((root / check_pins.PINS).read_text(encoding="utf-8"),
                                          (root / check_pins.FETCH).read_text(encoding="utf-8")), [])


if __name__ == "__main__":
    unittest.main()
