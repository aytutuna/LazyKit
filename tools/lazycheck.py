"""Rules check for the Lazy repos.

    python lazycheck.py REPO [REPO ...] [--fail rule,rule] [--report FILE]

Every rule only reports unless it is named in --fail; then a finding makes the exit code 1.
The app is taken from the resolved folder name (LazyPrint is print, alazy is the site); --app overrides it.
"""
import argparse
import json
import re
import subprocess
import sys
from collections import namedtuple
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import contrast

Finding = namedtuple("Finding", "rule path line text")

APPS = {"lazyscan": "scan", "lazyprint": "print", "lazypdf": "pdf", "lazysend": "send", "lazycollect": "collect"}
PORTS = {"scan": 52731, "print": 52732, "pdf": 52733, "send": 52734}
NOTICE = "Required Notice: Copyright (c) 2026 ALAZY INC. (https://alazy.app)"
SKIP_DIRS = {".git", ".wt", ".scratch", "node_modules", "Vendor", "fonts"}
# build output is skipped only when walking an unversioned folder; a tracked l10n/out is real
BUILD_DIRS = {"build", "out", ".build", "Pods", "bin", "obj", "dist"}
LANGUAGES = Path(__file__).resolve().parent.parent / "testdata" / "languages.json"

PAGES = {".html", ".resx", ".xcstrings", ".strings", ".plist", ".xaml"}
CODE = {".swift", ".cs", ".kt", ".ts", ".tsx", ".js"}


class Repo:
    def __init__(self, root, app=None):
        self.root = Path(root).resolve()
        self.site = self.root.name.lower() == "alazy" and app is None
        self.app = app or APPS.get(self.root.name.lower())
        self.files = self._list()

    def _list(self):
        if (self.root / ".git").exists():
            out = subprocess.run(["git", "-C", str(self.root), "ls-files", "-z"], capture_output=True, text=True,
                                 encoding="utf8", errors="replace").stdout
            names = [n for n in out.split("\0") if n]
        else:
            names = [p.relative_to(self.root).as_posix() for p in self.root.rglob("*") if p.is_file()]
            names = [n for n in names if not set(n.split("/")[:-1]) & BUILD_DIRS]
        return sorted(n for n in names if not set(n.split("/")[:-1]) & SKIP_DIRS and (self.root / n).is_file())

    def text(self, name):
        path = self.root / name
        if path.stat().st_size > 2_000_000:
            return ""
        raw = path.read_bytes()
        if raw.startswith((b"\xff\xfe", b"\xfe\xff")):
            return raw.decode("utf-16", errors="replace")
        return raw.decode("utf-8-sig", errors="replace")

    def json(self, name, found, rule):
        """Parsed JSON, or None after adding a finding when the file cannot be read."""
        try:
            return json.loads(self.text(name))
        except ValueError as e:
            found.append(Finding(rule, name, 0, f"cannot read: {e}"))
            return None

    def lines(self, name):
        return enumerate(self.text(name).splitlines(), 1)

    def ext(self, name):
        return Path(name).suffix.lower()


def is_test(name):
    low = name.lower()
    return bool(re.search(r"(^|/)(tests?|fixtures|reports|research_notes)(/|$)|\.tests?/|(^|[/._-])tests?\.[a-z]+$|tests?\.(swift|kt|cs)$", low))


def is_notice(name):
    base = Path(name).name
    return "THIRD-PARTY" in base.upper() or base in ("CONTRACTS.md", "package-lock.json") or base.startswith("LICENSE")


def rule_licence(repo):
    names = [n for n in repo.files if re.fullmatch(r"LICENSE(\.md|\.txt)?", n)]
    if not names:
        return [] if repo.site else [Finding("licence", "LICENSE", 0, "no LICENSE file")]
    text = repo.text(names[0])
    found = []
    if "PolyForm Noncommercial License 1.0.0" not in text:
        found.append(Finding("licence", names[0], 0, "not PolyForm Noncommercial 1.0.0"))
    if NOTICE not in text:
        found.append(Finding("licence", names[0], 0, "notice line missing or different"))
    return found


COPYRIGHT = re.compile(r"(©|&copy;|\(c\)|copyright)\s*(\(c\)|©)?\s*(19|20)\d\d", re.I)
PERSON = re.compile(r"Aytu Tuna|sole proprietor\b", re.I)
COPYRIGHT_TYPES = PAGES | {".props", ".csproj", ".xml", ".yml"}


def is_i18n(name):
    return bool(re.search(r"i18n/[A-Za-z-]+\.json$", name))


def rule_publisher(repo):
    found = []
    for name in repo.files:
        ext = repo.ext(name)
        if is_test(name) or (is_notice(name) and name != "LICENSE"):
            continue
        copyright_files = ext in COPYRIGHT_TYPES or name in ("LICENSE", "README.md") or is_i18n(name)
        if not (copyright_files or ext in CODE):
            continue
        for no, line in repo.lines(name):
            hit = PERSON.search(line) or (ext == ".html" and re.search(r"\bAytu\b", line))
            if not hit and COPYRIGHT.search(line) and "ALAZY INC." not in line:
                # in code only a string literal is shown to people; a header comment is not
                hit = copyright_files or re.search(r"[\"']", line)
            if hit:
                found.append(Finding("publisher", name, no, line.strip()[:120]))
    return found


OPEN_SOURCE = re.compile(r"open[- ]source", re.I)
NOT_OPEN = re.compile(r"(not|never|isn.t|no)\s+(an?\s+)?open[- ]source", re.I)
THIRD_PARTY = re.compile(r"\b(Apache|MIT|BSD|MPL|LGPL|GPL|ISC)\b")
SOURCE_LINE = re.compile(r"github\.com/aytutuna|source[- ]available|view (the )?source|"
                         r"source code\b[^.<]{0,80}\b(is|are)\s+(published|available|open)", re.I)


def rule_open_source(repo):
    found = []
    for name in repo.files:
        ext = repo.ext(name)
        if is_test(name) or is_notice(name):
            continue
        docs = ext in PAGES or ext in (".md", ".txt", ".xml") or is_i18n(name)
        if not (docs or ext in CODE):
            continue
        for no, line in repo.lines(name):
            if docs and OPEN_SOURCE.search(line) and not NOT_OPEN.search(line) and not THIRD_PARTY.search(line):
                found.append(Finding("open-source", name, no, line.strip()[:120]))
            elif ext not in (".md", ".txt") and SOURCE_LINE.search(line):
                found.append(Finding("open-source", name, no, line.strip()[:120]))
    return found


def expected_accents():
    out = {}
    for app, surface, text, *_ in contrast.table():
        out.setdefault(app.lower(), {})[surface] = text
    return out


def close(a, b, tol=0):
    return all(abs(x - y) <= tol for x, y in zip(contrast.to_rgb(a), contrast.to_rgb(b)))


def component(value):
    """One colorset component: 0-1 float, 0-255 integer or 0x hex."""
    text = str(value).strip()
    if text.lower().startswith("0x"):
        return int(text, 16)
    return round(float(text) * 255) if "." in text else int(text)


def rule_accent(repo):
    table = expected_accents()
    found = []
    seen = set()

    def check(name, no, app, mode, value, tol=0):
        want = table.get(app, {}).get(mode)
        if want and not close(value, want, tol):
            found.append(Finding("accent", name, no, f"{app} {mode} accent is {value.upper()}, table says {want}"))

    for name in repo.files:
        ext = repo.ext(name)
        if ext in (".css", ".html"):
            app = repo.app
            parts = name.split("/")
            if app is None and len(parts) > 2 and parts[0] == "public":
                app = parts[1]
            dark = False
            for no, line in repo.lines(name):
                if re.search(r"prefers-color-scheme:\s*dark", line):
                    dark = True
                m = re.search(r"--accent:\s*(#[0-9a-fA-F]{6})\b", line)
                if m and app:
                    seen.add("web")
                    check(name, no, app, "dark" if dark else "light", m.group(1))
        elif name.endswith("AccentColor.colorset/Contents.json") and repo.app:
            seen.add("apple")
            for entry in (repo.json(name, found, "accent") or {}).get("colors", []):
                try:
                    c = entry["color"]["components"]
                    value = "#%02X%02X%02X" % tuple(component(c[k]) for k in ("red", "green", "blue"))
                except (KeyError, ValueError, TypeError):
                    found.append(Finding("accent", name, 0, "colorset entry cannot be read"))
                    continue
                check(name, 0, repo.app, "dark" if "appearances" in entry else "light", value, tol=1)
        elif ext == ".kt" and repo.app and not is_test(name):
            mode = None
            for no, line in repo.lines(name):
                scheme = re.search(r"(light|dark)ColorScheme", line)
                if scheme:
                    mode = scheme.group(1)
                m = re.search(r"\bprimary\s*=\s*Color\(0x[0-9A-Fa-f]{2}([0-9A-Fa-f]{6})\)", line)
                if m and mode:
                    seen.add("android")
                    check(name, no, repo.app, mode, "#" + m.group(1))
        elif ext == ".xaml" and repo.app and not is_test(name):
            for no, line in repo.lines(name):
                m = re.search(r"x:Key=\"[^\"]*Accent[^\"]*\"[^#]*#([0-9A-Fa-f]{6})\b", line) or \
                    re.search(r"#([0-9A-Fa-f]{6})\b[^#]*x:Key=\"[^\"]*Accent", line)
                if m and "SystemColors" not in line:
                    seen.add("windows")
                    want = table.get(repo.app, {})
                    if want and not any(close("#" + m.group(1), v) for v in want.values()):
                        found.append(Finding("accent", name, no, f"{repo.app} accent is #{m.group(1).upper()}, table says {want['light']} / {want['dark']}"))

    if repo.app:
        for dirs, platform, where in ((("ios/", "mac/"), "apple", "an AccentColor colorset"),
                                      (("android/",), "android", "a primary color in the Compose theme"),
                                      (("windows/",), "windows", "an accent resource in XAML")):
            if platform not in seen and any(n.startswith(dirs) for n in repo.files):
                found.append(Finding("accent", dirs[0], 0, f"no accent found for {platform}: expected {where}"))
        if repo.app == "collect" and "web" not in seen:
            found.append(Finding("accent", "web/", 0, "no accent found for the web app"))
    return found


ALIASES = {"in": "id", "iw": "he", "ji": "yi", "jw": "jv"}
CHINESE = {"zh-CN": "zh-Hans", "zh-SG": "zh-Hans", "zh-TW": "zh-Hant", "zh-HK": "zh-Hant", "zh-MO": "zh-Hant"}
_rows = []


def lang_rows():
    if not _rows:
        _rows.extend(json.loads(LANGUAGES.read_text(encoding="utf8")))
    return _rows


def row_for(code, rows):
    code = ALIASES.get(code, code)
    code = CHINESE.get(code, code)
    for r in rows:
        if code in (r["apple"], r["web"], r["windows"]) or code == r["android"]:
            return r["web"]
    return None


def is_language(code):
    first = code.split("-")[0]
    return ALIASES.get(first, first) in {r["web"].split("-")[0] for r in lang_rows()}


def android_tag(folder):
    """The language a values folder holds, or None when it is some other qualifier (night, car, v26)."""
    m = re.fullmatch(r"values-b\+([A-Za-z0-9+]+)", folder)
    if m:
        return m.group(1).replace("+", "-")
    m = re.fullmatch(r"values-([a-z]{2,3})(?:-r([A-Z]{2}))?", folder)
    if m and is_language(m.group(1)):
        return m.group(1) + ("-" + m.group(2) if m.group(2) else "")
    return None


def xml_names(text):
    return {m.group(1) for m in re.finditer(r'<string\s+name="([^"]+)"(?![^>]*translatable="false")', text)}


def resx_names(text):
    return {m.group(1) for m in re.finditer(r'<data\s+name="([^"]+)"', text)}


def strings_keys(text):
    return set(re.findall(r'^\s*"((?:[^"\\]|\\.)*)"\s*=', text, re.M))


def catalog(repo, found):
    """Lazy Scan and Lazy PDF keep their texts in l10n/shared.json (and l10n/lang/*.json); the tables
    the apps load are made from it at build time. Returns (English keys, {tag: keys translated}) or None."""
    if "l10n/shared.json" not in repo.files:
        return None
    data = repo.json("l10n/shared.json", found, "languages")
    if not isinstance(data, dict):
        return None
    rows = lang_rows()
    langs = {}
    for key, value in data.items():
        for code in value if isinstance(value, dict) else ():
            tag = row_for(code, rows)
            if tag:
                langs.setdefault(tag, set()).add(key)
    for name in repo.files:
        m = re.fullmatch(r"l10n/lang/([A-Za-z-]+)\.json", name)
        tag = m and row_for(m.group(1), rows)
        table = tag and repo.json(name, found, "languages")
        if isinstance(table, dict):
            langs[tag] = set(table.get("strings", {}))
    # a non-empty value that names no language is a group of texts (Lazy Scan's infoplist), not one text
    keys = {k for k, v in data.items() if not (isinstance(v, dict) and v and not any(row_for(c, rows) for c in v))}
    return keys, langs


def rule_languages(repo):
    rows = lang_rows()
    found = []
    platforms = {}

    def add(platform, code, name):
        tag = row_for(code, rows)
        if tag is None:
            found.append(Finding("languages", name, 0, f"unknown language code {code}"))
        else:
            platforms.setdefault(platform, {}).setdefault(tag, []).append(name)

    for name in repo.files:
        parts = name.split("/")
        for p in parts[:-1]:
            if p.endswith(".lproj") and p != "Base.lproj":
                add("apple", p[:-6], name)
        if name.endswith(".xcstrings"):
            data = repo.json(name, found, "languages") or {}
            for code in {c for v in data.get("strings", {}).values() for c in v.get("localizations", {})}:
                add("apple", code, name)
        if name.endswith("/strings.xml") and len(parts) > 1 and parts[-2].startswith("values"):
            tag = android_tag(parts[-2])
            if tag:
                add("android", tag, name)
        m = re.fullmatch(r"(.+)\.([a-z]{2,3}(?:-[A-Za-z0-9]+)?)\.resx", Path(name).name)
        if m and is_language(m.group(2)):
            add("windows", m.group(2), name)
        if re.search(r"i18n/[A-Za-z-]+\.json$", name):
            add("web", Path(name).stem, name)

    made = catalog(repo, found)
    made_langs = set(made[1]) if made else set()
    base = {"apple": any(n.endswith(".xcstrings") or ".lproj/" in n for n in repo.files)
            or bool(made and any(n.startswith(("ios/", "mac/")) for n in repo.files)),
            "android": any(n.endswith("res/values/strings.xml") for n in repo.files),
            "windows": any(re.search(r"/Strings\.resx$", n) for n in repo.files),
            "web": any(n.endswith("i18n/en.json") for n in repo.files)}
    shipped = {}
    for platform, present in base.items():
        if present:
            extra = made_langs if platform in ("apple", "windows") else set()
            shipped[platform] = set(platforms.get(platform, {})) | extra | {"en-US"}
    union = set().union(*shipped.values()) if shipped else set()
    for platform, langs in shipped.items():
        for tag in sorted(union - langs):
            found.append(Finding("languages", platform, 0, f"{platform} does not ship {tag}"))

    if made:
        keys, langs = made
        for tag, have in sorted(langs.items()):
            missing = sorted(keys - have)
            if missing:
                found.append(Finding("languages", "l10n/shared.json", 0, f"{tag} is missing {len(missing)} of {len(keys)} texts, first: {missing[0][:60]}"))
    found += complete_xcstrings(repo, shipped.get("apple", set()))
    found += complete_android(repo)
    found += complete_resx(repo)
    found += complete_strings(repo)
    found += complete_web(repo)
    return found


def complete_xcstrings(repo, shipped):
    found = []
    rows = lang_rows()
    for name in repo.files:
        if not name.endswith(".xcstrings"):
            continue
        strings = (repo.json(name, [], "languages") or {}).get("strings", {})
        base = {k for k, v in strings.items() if k and v.get("shouldTranslate", True)}
        for tag in sorted(shipped - {"en-US"}):
            apple = next(r["apple"] for r in rows if r["web"] == tag)
            def done(v):
                loc = v.get("localizations", {}).get(apple, {})
                return loc.get("stringUnit", {}).get("value") or loc.get("variations")
            missing = sorted(k for k in base if not done(strings[k]))
            if missing:
                found.append(Finding("languages", name, 0, f"{apple} is missing {len(missing)} of {len(base)} texts, first: {missing[0][:60]}"))
    return found


def complete_android(repo):
    found = []
    for name in repo.files:
        parts = name.split("/")
        if parts[-1] != "strings.xml" or len(parts) < 2 or parts[-2] == "values" or not android_tag(parts[-2]):
            continue
        base_name = "/".join(parts[:-2] + ["values", "strings.xml"])
        if base_name not in repo.files:
            continue
        base = xml_names(repo.text(base_name))
        missing = sorted(base - xml_names(repo.text(name)))
        if missing:
            found.append(Finding("languages", name, 0, f"missing {len(missing)} of {len(base)} texts, first: {missing[0]}"))
    return found


def complete_resx(repo):
    found = []
    for name in repo.files:
        m = re.fullmatch(r"(.+)\.([a-z]{2,3}(?:-[A-Za-z0-9]+)?)\.resx", name)
        if not m or not is_language(m.group(2)) or m.group(1) + ".resx" not in repo.files:
            continue
        base = resx_names(repo.text(m.group(1) + ".resx"))
        missing = sorted(base - resx_names(repo.text(name)))
        if base and missing:
            found.append(Finding("languages", name, 0, f"missing {len(missing)} of {len(base)} texts, first: {missing[0]}"))
    return found


def complete_strings(repo):
    found = []
    for name in repo.files:
        m = re.fullmatch(r"(.+)/([^/]+)\.lproj/([^/]+\.strings)", name)
        if not m or m.group(2) in ("en", "Base"):
            continue
        base_name = next((f"{m.group(1)}/{b}.lproj/{m.group(3)}" for b in ("en", "Base") if f"{m.group(1)}/{b}.lproj/{m.group(3)}" in repo.files), None)
        if not base_name:
            continue
        base = strings_keys(repo.text(base_name))
        missing = sorted(base - strings_keys(repo.text(name)))
        if missing:
            found.append(Finding("languages", name, 0, f"missing {len(missing)} of {len(base)} texts, first: {missing[0][:60]}"))
    return found


def complete_web(repo):
    found = []
    for name in repo.files:
        m = re.fullmatch(r"(.*i18n)/([A-Za-z-]+)\.json", name)
        if not m or m.group(2) == "en" or m.group(1) + "/en.json" not in repo.files:
            continue
        base = repo.json(m.group(1) + "/en.json", found, "languages")
        table = repo.json(name, found, "languages")
        if isinstance(base, dict) and isinstance(table, dict):
            missing = sorted(set(base) - set(table))
            if missing:
                found.append(Finding("languages", name, 0, f"missing {len(missing)} of {len(base)} texts, first: {missing[0]}"))
    return found


BRITISH = re.compile(
    r"\b(colour(s|ed|ful|ing)?|favourite|behaviour|neighbour|honour|flavour|humour|"
    r"centre[sd]?|fibre|litre|theatre|cancell(ed|ing)|labell(ed|ing)|travell(ed|ing|er)|modell(ed|ing)|grey|catalogue|analyse[sd]?|"
    r"programme|defence|offence|artefact|judgement|ageing|dialogue|tyre|cheque|whilst|amongst|learnt|licence|"
    r"(organ|custom|optim|recogn|minim|maxim|synchron|personal|normal|final|util|author|apolog|summar|categor|emphas|prioriti|"
    r"initial|digit|stabil|visual|serial|capital|memor|special|real)is(e|es|ed|ing|ation|ations))\b", re.I)


def english_texts(repo):
    """Yield (file, line, text) for the English base text of every platform."""
    for name in repo.files:
        ext = repo.ext(name)
        if is_test(name):
            continue
        if name.endswith("res/values/strings.xml") or re.search(r"/Strings\.resx$", name) or name.endswith("i18n/en.json"):
            for no, line in repo.lines(name):
                yield name, no, line
        elif name == "l10n/shared.json":
            for key in (repo.json(name, [], "spelling") or {}):
                yield name, 0, key
        elif ext == ".xcstrings":
            strings = (repo.json(name, [], "spelling") or {}).get("strings", {})
            for key, v in strings.items():
                yield name, 0, key
                for code, loc in v.get("localizations", {}).items():
                    if code == "en" or code.startswith("en-"):
                        yield name, 0, loc.get("stringUnit", {}).get("value", "")
        elif ext == ".html" and "/fr/" not in "/" + name:
            text = repo.text(name)
            if re.search(r'<html[^>]*lang="en', text):
                for no, line in enumerate(text.splitlines(), 1):
                    yield name, no, line


def rule_spelling(repo):
    found = []
    for name, no, text in english_texts(repo):
        m = BRITISH.search(re.sub(r"<[^>]*>", " ", text))
        if m:
            found.append(Finding("spelling", name, no, f"'{m.group(0)}' in {text.strip()[:80]}"))
    return found


def workflows(repo):
    return [n for n in repo.files if re.fullmatch(r"\.github/workflows/[^/]+\.ya?ml", n)]


def steps_using(repo, name, action):
    """Yield (line number, text of the step) for each step that uses the action."""
    lines = repo.text(name).splitlines()
    for i, line in enumerate(lines):
        if line.lstrip().startswith("#") or not re.search(r"uses:\s*[\"']?[^\s\"']*" + action, line):
            continue
        start = i
        while start > 0 and not re.match(r"\s*-\s", lines[start]):
            start -= 1
        dash = len(lines[start]) - len(lines[start].lstrip())
        end = i + 1
        while end < len(lines) and (not lines[end].strip() or len(lines[end]) - len(lines[end].lstrip()) > dash):
            end += 1
        yield i + 1, "\n".join(lines[start:end])


STORE_FILE = re.compile(r"\.msix|msixupload|msixbundle|\.appx|\.ipa\b|\.pkg\b|\.aab\b|\.apk\b|\.dmg\b|\bstore/", re.I)
PUBLISHES = re.compile(r"gh\s+release\s+(create|upload)|action-gh-release|create-release|upload-release|upload-pages-artifact|deploy-pages|actions/deploy-pages")


def rule_wf_artifact(repo):
    found = []
    for name in workflows(repo):
        for no, step in steps_using(repo, name, "upload-artifact"):
            if STORE_FILE.search(step):
                found.append(Finding("wf-artifact", name, no, "a Store package is uploaded as a run artifact"))
    return found


def rule_wf_download(repo):
    found = []
    for name in workflows(repo):
        for no, step in steps_using(repo, name, "upload-artifact"):
            if re.search(r"\.zip|\bzip\b", step, re.I) and not STORE_FILE.search(step):
                found.append(Finding("wf-download", name, no, "a zip is uploaded as a run artifact"))
        for no, line in repo.lines(name):
            if not line.lstrip().startswith("#") and PUBLISHES.search(line):
                found.append(Finding("wf-download", name, no, "publishes something: " + line.strip()[:80]))
    return found


GUARD = re.compile(r"require_green_main|check_main_green|--status\s+success")
UPLOAD = re.compile(r"altool|-exportArchive|upload-testflight|upload_to_testflight|upload_to_play_store|upload-google-play|"
                    r"publishBundle|publishReleaseBundle|r0adkll")


def live_text(repo, name):
    return "\n".join(l for l in repo.text(name).splitlines() if not l.lstrip().startswith("#"))


def rule_upload_guard(repo):
    found = []
    for name in workflows(repo):
        body = live_text(repo, name)
        upload = UPLOAD.search(body)
        if not (upload or re.search(r"testflight|macappstore|appstore|playstore", Path(name).name)):
            continue
        guard = GUARD.search(body)
        if not guard or (upload and guard.start() > upload.start()):
            found.append(Finding("upload-guard", name, 0, "uploads to a store with no green-main check before it"))
    return found


def rule_store_x64(repo):
    found = []
    for name in workflows(repo):
        if Path(name).name in ("ci.yml", "ci.yaml"):
            continue
        for line in live_text(repo, name).splitlines():
            if re.search(r"-Arm64\b|-Arch\b[^\n]*arm64", line, re.I):
                found.append(Finding("store-x64", name, 0, "arm64 goes into a Store package"))
    for name in repo.files:
        if name.endswith("scripts/make_msix.ps1"):
            for no, line in repo.lines(name):
                if re.search(r"\$Arch\b[^\n]*=[^\n]*arm64", line, re.I) and not line.lstrip().startswith("#"):
                    found.append(Finding("store-x64", name, no, "the script packs arm64 by default"))
    return found


def rule_contracts(repo):
    return [Finding("contracts", n, 0, "CONTRACTS.md is tracked") for n in repo.files if Path(n).name == "CONTRACTS.md"]


def rule_firewall(repo):
    if repo.app not in PORTS:
        return []
    found = []
    own = PORTS[repo.app]
    others = {p for a, p in PORTS.items() if a != repo.app}
    for name in repo.files:
        if not name.endswith("Package.appxmanifest") or is_test(name):
            continue
        text = repo.text(name)
        if "windows.firewallRules" not in text:
            found.append(Finding("firewall", name, 0, "no firewall rule"))
            continue
        ranges = [(int(a), int(b)) for a, b in re.findall(r'LocalPortMin="(\d+)"\s+LocalPortMax="(\d+)"', text)]
        if not any(a <= own <= b for a, b in ranges):
            found.append(Finding("firewall", name, 0, f"port {own} is not in a rule"))
        for p in sorted(others):
            if any(a <= p <= b for a, b in ranges):
                found.append(Finding("firewall", name, 0, f"opens port {p}, which belongs to another app"))
    return found


VERSION_SOURCES = [
    (r"VERSION$", r"^\s*([0-9][0-9.]*)\s*$"),
    (r"Directory\.Build\.props$", r"<Version(?:Prefix)?>\s*([0-9][0-9.]*)\s*<"),
    (r"(ios|mac)/project\.yml$", r"MARKETING_VERSION:\s*\"?([0-9][0-9.]*)"),
    (r"scripts/make_app\.sh$", r"CFBundleShortVersionString</key><string>([0-9][0-9.]*)<"),
    (r"android/app/build\.gradle\.kts$", r"versionName\s*=\s*\"([0-9][0-9.]*)\""),
    (r"(^|/)package\.json$", r"\"version\":\s*\"([0-9][0-9.]*)\""),
]


def normal(version):
    parts = version.split(".")
    while len(parts) > 1 and parts[-1] == "0":
        parts.pop()
    return tuple(parts)


def rule_version(repo):
    seen = []
    for name in repo.files:
        for where, pattern in VERSION_SOURCES:
            if re.search(where, name):
                m = re.search(pattern, repo.text(name), re.M)
                if m:
                    seen.append((name, m.group(1)))
    if len({normal(v) for _, v in seen}) > 1:
        listing = ", ".join(f"{n} {v}" for n, v in seen)
        return [Finding("version", seen[0][0], 0, "versions differ: " + listing)]
    return []


CHECKS = {
    "licence": rule_licence, "publisher": rule_publisher, "open-source": rule_open_source, "accent": rule_accent,
    "languages": rule_languages, "spelling": rule_spelling, "wf-artifact": rule_wf_artifact,
    "wf-download": rule_wf_download, "upload-guard": rule_upload_guard, "store-x64": rule_store_x64,
    "contracts": rule_contracts, "firewall": rule_firewall, "version": rule_version,
}


def check(root, app=None, only=None):
    repo = Repo(root, app)
    found = []
    for rule, fn in CHECKS.items():
        if only is None or rule in only:
            try:
                found += fn(repo)
            except Exception as e:
                found.append(Finding(rule, "", 0, f"rule failed: {type(e).__name__}: {e}"))
    return found


def report(results, cap=25):
    out = ["# lazycheck report", ""]
    rules = list(CHECKS)
    out.append("| repo | " + " | ".join(rules) + " | total |")
    out.append("|" + " --- |" * (len(rules) + 2))
    for repo, found in results.items():
        counts = [sum(1 for f in found if f.rule == r) for r in rules]
        out.append(f"| {repo} | " + " | ".join(str(c) for c in counts) + f" | {len(found)} |")
    for repo, found in results.items():
        out += ["", f"## {repo}"]
        for rule in rules:
            hits = [f for f in found if f.rule == rule]
            if not hits:
                continue
            out += ["", f"### {rule} ({len(hits)})", ""]
            for f in hits[:cap]:
                where = f"{f.path}:{f.line}" if f.line else f.path
                out.append(f"- {where}: {f.text}")
            if len(hits) > cap:
                out.append(f"- and {len(hits) - cap} more")
    return "\n".join(out) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("repos", nargs="+")
    ap.add_argument("--app")
    ap.add_argument("--fail", default="", help="comma list of rules that fail the run")
    ap.add_argument("--report", help="write the report here as well")
    args = ap.parse_args(argv)
    fail = {r for r in args.fail.split(",") if r}
    unknown = fail - set(CHECKS)
    if unknown:
        ap.error("unknown rule: " + ", ".join(sorted(unknown)))
    results = {}
    for r in args.repos:
        repo = Repo(r, args.app)
        if repo.app is None and not repo.site:
            ap.error(f"cannot tell which app {repo.root.name} is; pass --app scan, print, pdf, send or collect")
        results[repo.root.name] = check(r, args.app)
    text = report(results)
    print(text)
    if args.report:
        Path(args.report).write_text(text, encoding="utf8")
    return 1 if any(f.rule in fail for found in results.values() for f in found) else 0


if __name__ == "__main__":
    sys.exit(main())
