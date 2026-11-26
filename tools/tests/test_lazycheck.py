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
NEW_SPELLING = GOOD_LICENSE.replace("ALAZY INC.", "Alazy Inc.")


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
        self.green({"LICENSE": NEW_SPELLING}, "licence")
        self.red({"LICENSE": GOOD_LICENSE.replace("ALAZY INC.", "Alazy Corp")}, "licence")

    def test_publisher(self):
        self.red({"mac/Info.plist": "<string>Copyright © 2026 Aytu</string>"}, "publisher")
        self.red({"site/index.html": "<p>Published by Aytu Tuna</p>"}, "publisher")
        self.green({"mac/Info.plist": "<string>Copyright © 2026 ALAZY INC.</string>"}, "publisher")
        self.green({"mac/Info.plist": "<string>Copyright © 2026 Alazy Inc.</string>"}, "publisher")
        self.red({"mac/Info.plist": "<string>Copyright © 2026 alazy inc</string>"}, "publisher")

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


class LanguageTableTests(unittest.TestCase):
    base = '<resources><string name="a">A</string><string name="b">B</string></resources>'
    part = '<resources><string name="a">A</string></resources>'
    res = "android/app/src/main/res/"

    def test_android_standard_folder_is_compared(self):
        files = {self.res + "values/strings.xml": self.base, self.res + "values-tr/strings.xml": self.part}
        found = run(files, "languages")
        self.assertTrue(any("missing 1 of 2" in f.text for f in found), found)

    def test_android_bplus_folder_is_compared(self):
        files = {self.res + "values/strings.xml": self.base, self.res + "values-b+tr/strings.xml": self.part}
        self.assertTrue(any("missing 1 of 2" in f.text for f in run(files, "languages")))

    def test_android_complete_table_is_green(self):
        files = {self.res + "values/strings.xml": self.base, self.res + "values-tr/strings.xml": self.base}
        self.assertEqual(run(files, "languages"), [])

    def test_android_old_hebrew_code_and_other_qualifiers(self):
        files = {self.res + "values/strings.xml": self.base, self.res + "values-iw/strings.xml": self.base,
                 self.res + "values-car/strings.xml": self.part, self.res + "values-night/strings.xml": self.part}
        found = run(files, "languages")
        self.assertFalse([f for f in found if "unknown" in f.text or "missing" in f.text], found)

    def test_legacy_chinese_folder_is_a_known_language(self):
        files = {self.res + "values/strings.xml": self.base, self.res + "values-zh-rTW/strings.xml": self.base}
        self.assertFalse([f for f in run(files, "languages") if "unknown" in f.text])

    def test_base_lproj_is_not_a_language(self):
        self.assertEqual(run({"ios/Base.lproj/Main.strings": '"a" = "A";'}, "languages"), [])

    def test_resx_behind_its_base(self):
        base = '<root><data name="a"/><data name="b"/></root>'
        found = run({"windows/Core/Strings.resx": base, "windows/Core/Strings.tr.resx": '<root><data name="a"/></root>'}, "languages")
        self.assertTrue(any("missing 1 of 2" in f.text for f in found), found)

    def test_resx_with_an_empty_base_is_not_complete_or_wrong(self):
        found = run({"windows/Core/Strings.resx": "<root/>", "windows/Core/Strings.tr.resx": '<root><data name="a"/></root>'}, "languages")
        self.assertFalse([f for f in found if "missing" in f.text])

    def test_strings_file_behind_its_base(self):
        files = {"ios/en.lproj/Kit.strings": '"a" = "A";\n"b" = "B";\n', "ios/tr.lproj/Kit.strings": '"a" = "A";\n'}
        self.assertTrue(any("missing 1 of 2" in f.text for f in run(files, "languages")))

    def test_web_table_behind_en(self):
        files = {"web/src/i18n/en.json": '{"a": "A", "b": "B"}', "web/src/i18n/fr-CA.json": '{"a": "A"}'}
        self.assertTrue(any("missing 1 of 2" in f.text for f in run(files, "languages", "collect")))
        files["web/src/i18n/fr-CA.json"] = '{"a": "A", "b": "B"}'
        self.assertFalse([f for f in run(files, "languages", "collect") if "missing" in f.text])

    def test_catalog_with_languages_per_key(self):
        shared = json.dumps({"One": {"tr": "Bir"}, "Two": {}})
        files = {"l10n/shared.json": shared, "ios/App.xcassets/x": "", "android/app/src/main/res/values/strings.xml": self.base}
        found = run(files, "languages", "pdf")
        self.assertTrue(any("tr is missing 1 of 2" in f.text for f in found), found)
        self.assertTrue(any(f.path == "android" and "tr" in f.text for f in found), found)

    def test_catalog_with_language_files(self):
        files = {"l10n/shared.json": json.dumps({"One": {}, "Two": {}}),
                 "l10n/lang/de.json": json.dumps({"strings": {"One": "Eins"}}), "mac/x": ""}
        self.assertTrue(any("de is missing 1 of 2" in f.text for f in run(files, "languages", "scan")))

    def test_catalog_groups_are_not_texts(self):
        files = {"l10n/shared.json": json.dumps({"One": {}, "infoplist": {"ios": {"Key": "x"}}}),
                 "l10n/lang/de.json": json.dumps({"strings": {"One": "Eins"}}), "mac/x": ""}
        self.assertFalse([f for f in run(files, "languages", "scan") if "missing" in f.text])

    def test_malformed_json_is_a_finding_not_a_crash(self):
        found = run({"ios/Localizable.xcstrings": "{nope"}, "languages")
        self.assertTrue(any("cannot read" in f.text for f in found))

    def test_spelling_reads_the_catalog_keys(self):
        self.assertTrue(run({"l10n/shared.json": json.dumps({"The scan was cancelled.": {}})}, "spelling"))
        self.assertEqual(run({"l10n/shared.json": json.dumps({"The scan was canceled.": {}})}, "spelling"), [])


def colorset(red, green, blue, dark=None):
    def entry(r, g, b, appearance):
        e = {"color": {"color-space": "srgb", "components": {"red": r, "green": g, "blue": b, "alpha": "1.000"}}}
        if appearance:
            e["appearances"] = [{"appearance": "luminosity", "value": "dark"}]
        return e
    colors = [entry(red, green, blue, False)]
    if dark:
        colors.append(entry(*dark, True))
    return json.dumps({"colors": colors})


class TextRuleTests(unittest.TestCase):
    def test_publisher_in_build_files_and_literals(self):
        self.assertTrue(run({"windows/Directory.Build.props": "<Copyright>Copyright (c) 2026 Aytu</Copyright>"}, "publisher"))
        self.assertTrue(run({"ios/project.yml": "NSHumanReadableCopyright: Copyright 2026 Aytu"}, "publisher"))
        self.assertTrue(run({"web/src/i18n/en.json": '{"a": "© 2026 Someone"}'}, "publisher"))
        self.assertTrue(run({"Sources/About.swift": 'Text("© 2026 Aytu")'}, "publisher"))
        self.assertEqual(run({"Sources/About.swift": "// Copyright (c) 2026 Somebody Else"}, "publisher"), [])
        self.assertEqual(run({"windows/Directory.Build.props": "<Copyright>Copyright (c) 2026 ALAZY INC.</Copyright>"}, "publisher"), [])

    def test_source_code_is_published_line(self):
        self.assertTrue(run({"site/terms.html": "<p>The source code of Lazy Collect is published under the PolyForm licence.</p>"}, "open-source"))
        self.assertEqual(run({"site/terms.html": "<p>You can read the terms below.</p>"}, "open-source"), [])

    def test_open_source_with_a_named_third_party_licence_is_allowed(self):
        self.assertEqual(run({"site/privacy.html": "<p>an open-source model (DocAligner, Apache-2.0)</p>"}, "open-source"), [])
        self.assertTrue(run({"android/res/values/strings.xml": "<string name=\"a\">Free and open source</string>"}, "open-source"))

    def test_accent_in_a_html_style_block(self):
        page = "<style>:root { --accent: #006B82; }</style>"
        self.assertTrue(run({"public/scan/index.html": page}, "accent", None))

    def test_accent_colorset(self):
        good = colorset("0.047", "0.525", "0.416", dark=("0.055", "0.624", "0.494"))
        self.assertTrue(run({"ios/A/Assets.xcassets/AccentColor.colorset/Contents.json": colorset("0.1", "0.4", "0.7")}, "accent"))
        self.assertEqual(run({"ios/A/Assets.xcassets/AccentColor.colorset/Contents.json": good}, "accent"), [])

    def test_accent_colorset_hex_and_integer_forms(self):
        hexed = colorset("0x0C", "0x86", "0x6A", dark=("0x0E", "0x9F", "0x7E"))
        self.assertEqual(run({"ios/A/Assets.xcassets/AccentColor.colorset/Contents.json": hexed}, "accent"), [])
        ints = colorset("12", "134", "106")
        found = run({"ios/A/Assets.xcassets/AccentColor.colorset/Contents.json": ints}, "accent")
        self.assertFalse([f for f in found if "light" in f.text])

    def test_accent_in_a_compose_theme(self):
        theme = "val Light = lightColorScheme(primary = Color(0xFF3F51B5))\nval Dark = darkColorScheme(primary = Color(0xFF0E9F7E))"
        found = run({"android/app/Theme.kt": theme}, "accent")
        self.assertEqual([f.text.split()[1] for f in found], ["light"])

    def test_accent_in_a_xaml_resource(self):
        self.assertTrue(run({"windows/App/Colors.xaml": '<Color x:Key="AppAccent">#112233</Color>'}, "accent"))
        self.assertEqual(run({"windows/App/Colors.xaml": '<Color x:Key="AppAccent">#0C866A</Color>'}, "accent"), [])

    def test_a_platform_without_an_accent_is_a_gap(self):
        for path in ("ios/x.txt", "android/x.txt", "windows/x.txt"):
            found = run({path: ""}, "accent")
            self.assertTrue(any("no accent found" in f.text for f in found), path)


class WorkflowRuleTests(unittest.TestCase):
    def test_guard_in_a_comment_does_not_count(self):
        wf = "# TODO call scripts/check_main_green.sh\nsteps:\n  - run: xcodebuild -exportArchive\n"
        self.assertTrue(run({".github/workflows/testflight.yml": wf}, "upload-guard"))

    def test_guard_after_the_upload_does_not_count(self):
        wf = "steps:\n  - run: xcodebuild -exportArchive\n  - run: scripts/check_main_green.sh\n"
        self.assertTrue(run({".github/workflows/testflight.yml": wf}, "upload-guard"))

    def test_play_and_fastlane_uploads_need_a_guard(self):
        for text in ("- uses: r0adkll/upload-google-play@v1", "- run: ./gradlew publishBundle", "- run: fastlane upload_to_testflight"):
            self.assertTrue(run({".github/workflows/android.yml": "steps:\n  " + text + "\n"}, "upload-guard"), text)
        self.assertEqual(run({".github/workflows/android.yml": "steps:\n  - run: gh run list --status success\n  - run: ./gradlew publishBundle\n"}, "upload-guard"), [])

    def test_arch_list_with_arm64(self):
        self.assertTrue(run({".github/workflows/release.yml": "steps:\n  - run: ./make_msix.ps1 -Arch x64,arm64\n"}, "store-x64"))
        self.assertTrue(run({".github/workflows/release.yml": "steps:\n  - run: ./make_msix.ps1 -arm64 out\n"}, "store-x64"))
        self.assertEqual(run({".github/workflows/release.yml": "steps:\n  - run: ./make_msix.ps1 -Arch x64\n"}, "store-x64"), [])

    def test_script_default_with_arm64(self):
        script = "param(\n    [string[]] $Arch = @('x64', 'arm64')\n)\n"
        self.assertTrue(run({"scripts/make_msix.ps1": script}, "store-x64"))
        self.assertEqual(run({"scripts/make_msix.ps1": script.replace(", 'arm64'", "")}, "store-x64"), [])

    def test_contracts_and_downloads_have_green_cases(self):
        self.assertEqual(run({"README.md": "x"}, "contracts"), [])
        self.assertEqual(run({".github/workflows/ci.yml": "steps:\n  - run: dotnet test\n"}, "wf-download"), [])


class RobustnessTests(unittest.TestCase):
    def test_bom_and_utf16_files_are_read(self):
        bom = bytes([0xEF, 0xBB, 0xBF])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "LazyPrint"
            root.mkdir()
            (root / "a.xcstrings").write_bytes(bom + xcstrings(en="Hello").encode())
            (root / "VERSION").write_bytes(bom + b"1.0")
            (root / "b.strings").write_text('"a" = "b";', encoding="utf-16")
            repo = lazycheck.Repo(root)
            self.assertEqual(repo.json("a.xcstrings", [], "x")["sourceLanguage"], "en")
            self.assertIn("a", repo.text("b.strings"))
            self.assertEqual(lazycheck.check(root, only={"languages", "version"}), [])

    def test_a_rule_that_raises_is_reported(self):
        original = lazycheck.CHECKS["contracts"]
        lazycheck.CHECKS["contracts"] = lambda repo: 1 / 0
        try:
            found = run({}, "contracts")
        finally:
            lazycheck.CHECKS["contracts"] = original
        self.assertIn("rule failed", found[0].text)

    def test_names_that_only_end_in_test_are_not_tests(self):
        self.assertFalse(lazycheck.is_test("server/src/attest.ts"))
        self.assertFalse(lazycheck.is_test("web/latest.json"))
        self.assertTrue(lazycheck.is_test("web/src/format.test.ts"))
        self.assertTrue(lazycheck.is_test("Tests/Foo/BarTests.swift"))
        self.assertTrue(lazycheck.is_test("windows/LazyPrint.App.Tests/X.cs"))


class CommandLineTests(unittest.TestCase):
    def test_dot_and_worktree_names(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "LazyPrint"
            root.mkdir()
            here = Path.cwd()
            try:
                import os
                os.chdir(root)
                self.assertEqual(lazycheck.Repo(".").app, "print")
            finally:
                os.chdir(here)
            self.assertIsNone(lazycheck.Repo(Path(tmp)).app)

    def test_unknown_app_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "print-slice1"
            root.mkdir()
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                lazycheck.main([str(root)])
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(lazycheck.main([str(root), "--app", "print"]), 0)

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
