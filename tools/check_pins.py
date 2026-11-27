#!/usr/bin/env python3
"""Checks native/pins.toml against rust/lazy-scan-model/scripts/fetch.sh: every pin's digest and download address is in the
script, tied to the pin: a pin with `var` is the script's `var=<digest>` line, any other pin with a `path` is on the script's
line that names that path. Every digest the script holds is a pin, and every pin names a licence the cargo-deny allowlist
would also take.
Usage: check_pins.py [repo root]"""
import re
import sys
import tomllib
from pathlib import Path

FETCH = Path("rust/lazy-scan-model/scripts/fetch.sh")
PINS = Path("native/pins.toml")
LICENCES = {"MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib"}
HEX = re.compile(r"\b[0-9a-f]{64}\b")


def check(pins_text, fetch_text):
    pins = tomllib.loads(pins_text).get("pin", [])
    fetch_text = uncommented(fetch_text)
    downloads = download_calls(fetch_text)
    problems = []
    names = {p.get("name") for p in pins}
    if len(names) != len(pins):
        problems.append("a pin name is used twice")
    for p in pins:
        name = p.get("name", "?")
        if not re.fullmatch(r"[0-9a-f]{64}", p.get("sha256", "")):
            problems.append(f"{name}: no SHA-256")
        if p.get("licence") not in LICENCES:
            problems.append(f"{name}: licence {p.get('licence')!r} is not on the allowlist")
        if "url" not in p and p.get("from") not in names:
            problems.append(f"{name}: neither a url nor a pin to come from")
        digest = p.get("sha256", "")
        if digest not in fetch_text:
            problems.append(f"{name}: its digest is not in fetch.sh")
        elif "var" in p:
            if not re.search(rf"^{re.escape(p['var'])}={digest}$", fetch_text, re.M):
                problems.append(f"{name}: fetch.sh does not set {p['var']} to its digest")
        elif "path" in p:
            if not any(p["path"] in line and digest in line for line in fetch_text.splitlines()):
                problems.append(f"{name}: no line of fetch.sh has both its path and its digest")
        else:
            problems.append(f"{name}: no var or path to tie its digest to fetch.sh")
        if "path" in p and not path_in_script(p["path"], fetch_text):
            problems.append(f"{name}: path {p['path']} is not what fetch.sh extracts")
        for key in ("url", "upstream"):
            if key in p and (p[key], digest) not in downloads:
                problems.append(f"{name}: no download call of fetch.sh fetches {key} {p[key]} and checks it against its digest")
        if "version" in p and not any(
            re.search(rf"(?<![\d.]){re.escape(p['version'])}(?![\d.])", p[key]) for key in ("url", "upstream") if key in p
        ):
            problems.append(f"{name}: version {p['version']} is not in its download address")
    versions = {p.get("version") for p in pins}
    for var, value in assignments(uncommented(fetch_text)).items():
        if var.endswith("_version") and value not in versions:
            problems.append(f"fetch.sh has {var}={value} but no pin has that version")
    pinned = {p.get("sha256") for p in pins}
    for digest in sorted(set(HEX.findall(fetch_text)) - pinned):
        problems.append(f"fetch.sh holds digest {digest} that no pin lists")
    return problems


def path_in_script(path, fetch_text):
    # the Android libraries are taken out as `jni/$abi/libonnxruntime.so`
    generic = re.sub(r"/(x86_64|arm64-v8a)/", "/$abi/", path)
    return path in fetch_text or generic in fetch_text


def assignments(fetch_text):
    """The script's plain `name=value` and `name="value"` lines, each value with earlier variables filled in."""
    values = {}
    for m in re.finditer(r'^(\w+)=("?)(.*?)\2$', fetch_text, re.M):
        values[m.group(1)] = expand(m.group(3), values)
    return values


def expand(text, values):
    return re.sub(r"\$\{?(\w+)\}?", lambda m: values.get(m.group(1), m.group(0)), text)


def uncommented(fetch_text):
    return "\n".join(line for line in fetch_text.splitlines() if not line.lstrip().startswith("#"))


def download_calls(fetch_text):
    """What the script's `download "<url>" "<file>" "<sha256>"` calls fetch and check, as (url, sha256) with the variables
    filled in (the script writes a url as `$copy/$model`). Commented lines are skipped."""
    code = uncommented(fetch_text)
    values = assignments(code)
    calls = set()
    for m in re.finditer(r'\bdownload\s+"([^"]+)"\s+"[^"]+"\s+"([^"]+)"', code):
        calls.add((expand(m.group(1), values), expand(m.group(2), values)))
    return calls


def main(argv):
    root = Path(argv[1]) if len(argv) > 1 else Path(__file__).resolve().parent.parent
    problems = check((root / PINS).read_text(encoding="utf-8"), (root / FETCH).read_text(encoding="utf-8"))
    for problem in problems:
        print(problem, file=sys.stderr)
    if not problems:
        print("pins.toml and fetch.sh agree")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
