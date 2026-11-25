import contextlib
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import lazycheck

GOOD_LICENSE = "Required Notice: Copyright (c) 2026 ALAZY INC. (https://alazy.app)\n\nPolyForm Noncommercial License 1.0.0\n"


def run(files, rule, app="print"):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "Repo"
        for name, text in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf8")
        return lazycheck.check(root, app, only={rule})


def xcstrings(**langs):
    strings = {"Hello": {"localizations": {code: {"stringUnit": {"value": v}} for code, v in langs.items() if v}}}
    return json.dumps({"sourceLanguage": "en", "strings": strings})


class RuleTests(unittest.TestCase):
    def red(self, files, rule, app="print"):
        self.assertTrue(run(files, rule, app), rule)

    def green(self, files, rule, app="print"):
        self.assertEqual(run(files, rule, app), [], rule)

    def test_licence(self):
        self.red({"LICENSE": "Required Notice: Copyright (c) 2026 Aytu (https://alazy.app)\nPolyForm Noncommercial License 1.0.0"}, "licence")
        self.red({}, "licence")
        self.green({"LICENSE": GOOD_LICENSE}, "licence")

    def test_publisher(self):
        self.red({"mac/Info.plist": "<string>Copyright © 2026 Aytu</string>"}, "publisher")
        self.red({"site/index.html": "<p>Published by Aytu Tuna</p>"}, "publisher")
        self.green({"mac/Info.plist": "<string>Copyright © 2026 ALAZY INC.</string>"}, "publisher")

    def test_open_source(self):
        self.red({"site/index.html": "<p>Lazy Print is open source.</p>"}, "open-source")
        self.red({"windows/About.xaml": '<TextBlock Text="Source available under PolyForm"/>'}, "open-source")
        self.green({"README.md": "The source is available, but this is not an open source licence."}, "open-source")

    def test_accent(self):
        self.red({"site/style.css": ":root { --accent: #123456; }"}, "accent")
        self.green({"site/style.css": ":root { --accent: #0C866A; }\n@media (prefers-color-scheme: dark) {\n:root { --accent: #0E9F7E; }\n}"}, "accent")

    def test_languages_platform_sets(self):
        files = {"ios/Localizable.xcstrings": xcstrings(en="Hello", tr="Merhaba"),
                 "android/app/src/main/res/values/strings.xml": '<resources><string name="a">A</string></resources>'}
        self.red(files, "languages")

    def test_languages_missing_text(self):
        files = {"ios/Localizable.xcstrings": xcstrings(en="Hello", tr="")}
        files["ios/Localizable.xcstrings"] = json.dumps({"strings": {
            "Hello": {"localizations": {"tr": {"stringUnit": {"value": "Merhaba"}}}},
            "Bye": {"localizations": {"en": {"stringUnit": {"value": "Bye"}}}}}})
        self.red(files, "languages")

    def test_languages_unknown_code(self):
        self.red({"ios/xx.lproj/InfoPlist.strings": ""}, "languages")

    def test_languages_complete(self):
        self.green({"ios/Localizable.xcstrings": xcstrings(en="Hello", tr="Merhaba")}, "languages")

    def test_spelling(self):
        self.red({"android/app/src/main/res/values/strings.xml": '<resources><string name="a">Pick a colour</string></resources>'}, "spelling")
        self.green({"android/app/src/main/res/values/strings.xml": '<resources><string name="a">Pick a color</string></resources>'}, "spelling")

    def test_wf_artifact(self):
        wf = "jobs:\n  b:\n    steps:\n      - uses: actions/upload-artifact@v7\n        with:\n          name: pack\n          path: out/App_1.0.msixupload\n"
        self.red({".github/workflows/release.yml": wf}, "wf-artifact")
        self.green({".github/workflows/ci.yml": wf.replace("out/App_1.0.msixupload", "shots")}, "wf-artifact")

    def test_wf_download(self):
        wf = "jobs:\n  b:\n    steps:\n      - uses: actions/upload-artifact@v7\n        with:\n          name: x\n          path: out/all.zip\n"
        self.red({".github/workflows/ci.yml": wf}, "wf-download")
        self.red({".github/workflows/p.yml": "steps:\n  - run: gh release create v1\n"}, "wf-download")

    def test_upload_guard(self):
        self.red({".github/workflows/testflight.yml": "steps:\n  - run: xcodebuild -exportArchive\n"}, "upload-guard")
        self.green({".github/workflows/testflight.yml": "steps:\n  - run: scripts/check_main_green.sh\n  - run: xcodebuild -exportArchive\n"}, "upload-guard")

    def test_store_x64(self):
        self.red({".github/workflows/release.yml": "steps:\n  - run: ./make_msix.ps1 -X64 a -Arm64 b\n"}, "store-x64")
        self.green({".github/workflows/ci.yml": "steps:\n  - run: ./make_msix.ps1 -X64 a -Arm64 b\n"}, "store-x64")

    def test_contracts(self):
        self.red({"CONTRACTS.md": "x"}, "contracts")

    def test_firewall(self):
        rule = '<desktop2:Extension Category="windows.firewallRules"><desktop2:Rule LocalPortMin="%d" LocalPortMax="%d"/></desktop2:Extension>'
        self.red({"store/windows/Package.appxmanifest": "<Package/>"}, "firewall")
        self.red({"store/windows/Package.appxmanifest": rule % (52731, 52734)}, "firewall")
        self.green({"store/windows/Package.appxmanifest": rule % (52732, 52732)}, "firewall")

    def test_version(self):
        files = {"windows/Directory.Build.props": "<Version>1.0.4</Version>",
                 "ios/project.yml": '    MARKETING_VERSION: "1.0"\n'}
        self.red(files, "version")
        files["ios/project.yml"] = '    MARKETING_VERSION: "1.0.4"\n'
        self.green(files, "version")
        self.green({"windows/Directory.Build.props": "<Version>1.0.0</Version>", "ios/project.yml": 'MARKETING_VERSION: "1.0"'}, "version")


class CommandLineTests(unittest.TestCase):
    def test_fail_list_sets_the_exit_code(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "LazyPrint"
            root.mkdir()
            (root / "CONTRACTS.md").write_text("x")
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(lazycheck.main([str(root), "--fail", "contracts"]), 1)
                self.assertEqual(lazycheck.main([str(root)]), 0)
                self.assertEqual(lazycheck.main([str(root), "--fail", "licence"]), 1)


if __name__ == "__main__":
    unittest.main()
