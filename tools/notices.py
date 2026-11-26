#!/usr/bin/env python3
"""Writes THIRD-PARTY-NOTICES.txt from the crates in rust/Cargo.lock (through `cargo metadata`) and the downloads in
native/pins.toml, or with --check says whether the file is current.
Usage: notices.py [--check] [repo root]"""
import json
import subprocess
import sys
import tomllib
from pathlib import Path

HEADER = """Third-party components of LazyKit

LazyKit's own code is under the PolyForm Noncommercial License 1.0.0 (LICENSE). The components below keep their own
licences; each crate's licence text and copyright lines are in its source, at the address given. The file is made by
tools/notices.py from rust/Cargo.lock and native/pins.toml.

UniFFI (MPL-2.0) is used unmodified; its source is at https://github.com/mozilla/uniffi-rs.
"""


def crate_rows(metadata):
    members = set(metadata["workspace_members"])
    rows = {
        (p["name"], p["version"], p.get("license") or "see source", p.get("repository") or "https://crates.io/crates/" + p["name"])
        for p in metadata["packages"]
        if p["id"] not in members
    }
    return sorted(rows)


def native_rows(pins_text):
    rows = []
    for pin in tomllib.loads(pins_text).get("pin", []):
        if "url" in pin:
            rows.append((pin["name"], pin.get("version", ""), pin["licence"], pin.get("upstream", pin["url"])))
    return sorted(rows)


def render(metadata, pins_text):
    lines = [HEADER, "Rust crates", ""]
    for name, version, licence, source in crate_rows(metadata):
        lines.append(f"{name} {version}\n  {licence}\n  {source}")
    lines += ["", "Downloaded files (native/pins.toml)", ""]
    for name, version, licence, source in native_rows(pins_text):
        lines.append(f"{name} {version}".rstrip() + f"\n  {licence}\n  {source}")
    return "\n".join(lines) + "\n"


def main(argv):
    check = "--check" in argv
    args = [a for a in argv[1:] if a != "--check"]
    root = Path(args[0]) if args else Path(__file__).resolve().parent.parent
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", str(root / "rust" / "Cargo.toml")],
        check=True, capture_output=True, text=True,
    ).stdout
    text = render(json.loads(out), (root / "native" / "pins.toml").read_text(encoding="utf-8"))
    target = root / "THIRD-PARTY-NOTICES.txt"
    if check:
        if not target.exists() or target.read_text(encoding="utf-8") != text:
            print("THIRD-PARTY-NOTICES.txt is out of date: run python3 tools/notices.py", file=sys.stderr)
            return 1
        print("THIRD-PARTY-NOTICES.txt is current")
        return 0
    target.write_text(text, encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
