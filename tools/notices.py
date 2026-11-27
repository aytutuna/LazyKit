#!/usr/bin/env python3
"""Writes THIRD-PARTY-NOTICES.txt from the crates in rust/Cargo.lock (through `cargo metadata`) and the downloads in
native/pins.toml, or with --check says whether the file is current.
Usage: notices.py [--check] [repo root]"""
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

HEADER = """Third-party components of LazyKit

LazyKit's own code is under the PolyForm Noncommercial License 1.0.0 (LICENSE). The components below keep their own
licences; the licence files found in each crate's source are copied at the end, and ONNX Runtime and the corner model have
their own section. The file is made by tools/notices.py from rust/Cargo.lock and native/pins.toml.

UniFFI (MPL-2.0) is used unmodified; its source is at https://github.com/mozilla/uniffi-rs.
"""

LICENCE_FILE = re.compile(r"(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)", re.I)

NATIVE = """ONNX Runtime {ort} (MIT), the library the scanner loads, from the Microsoft.ML.OnnxRuntime NuGet package

  Copyright (c) Microsoft Corporation

  Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
  documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights
  to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to
  whom the Software is furnished to do so, subject to the following conditions:

  The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
  Software.

  THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
  WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
  COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
  ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

  ONNX Runtime bundles code of other projects. Their notices are in ThirdPartyNotices.txt, which comes with the NuGet package
  (the package root) and is at
  {ort_notices}
  It has to be shipped with any build that includes the library.

DocAligner FastViT-T8 corner model {model} (Apache-2.0), by DocsaidLab, https://github.com/DocsaidLab/DocAligner

  Licensed under the Apache License, Version 2.0; the licence text is at https://www.apache.org/licenses/LICENSE-2.0 and in
  the project's LICENSE file. The model is used as published, except for the copy made for tract (the file named
  fastvit_t8_h_e_bifpn_256_fp32-tract.onnx), which tools/patch_einsum.py changes to rewrite its Einsum operators. The
  software is distributed on an "AS IS" basis, without warranties or conditions of any kind.
"""

def native_text(pins_text):
    """The native section, with the versions and the notices address read from the pins."""
    pins = {p["name"]: p for p in tomllib.loads(pins_text).get("pin", [])}
    return NATIVE.format(
        ort=pins["onnxruntime-nuget"]["version"],
        ort_notices=pins["onnxruntime-nuget"]["notices"],
        model=pins["docaligner-model"]["version"],
    )


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


FALLBACK = Path(__file__).resolve().parent / "licences"


def read_files(folder, keep):
    return [(f.name, f.read_text(encoding="utf-8", errors="replace")) for f in sorted(folder.iterdir()) if f.is_file() and keep(f.name)]


def licence_texts(metadata, fallback=FALLBACK):
    """The licence, copying and notice files at the top of each crate's source: {(name, version): [(file, text)]}. A crate
    that ships none uses the files in `fallback`/<name>-<version>/ (copied from its upstream repository; SOURCE says where);
    a crate with neither is an error."""
    members = set(metadata["workspace_members"])
    found, missing = {}, []
    for p in metadata["packages"]:
        if p["id"] in members:
            continue
        files = read_files(Path(p["manifest_path"]).parent, lambda n: LICENCE_FILE.match(n))
        extra = fallback / f"{p['name']}-{p['version']}"
        if not files and extra.is_dir():
            files = read_files(extra, lambda n: n != "SOURCE")
        if not files:
            missing.append(f"{p['name']} {p['version']}")
        found[(p["name"], p["version"])] = files
    if missing:
        raise MissingTexts("no licence text for " + ", ".join(missing) + f": add them under {fallback}/<crate>-<version>/")
    return found


class MissingTexts(Exception):
    pass


def text_sections(texts):
    """One entry per distinct text, with every crate that carries it."""
    shared = {}
    for (name, version), files in texts.items():
        for _, text in files:
            body = text.replace("\r\n", "\n").strip("\n")
            if body.strip():
                shared.setdefault(body, set()).add(f"{name} {version}")
    return sorted(((sorted(crates), body) for body, crates in shared.items()), key=lambda e: (e[0], e[1]))


def render(metadata, pins_text, texts=None):
    lines = [HEADER, "Rust crates", ""]
    for name, version, licence, source in crate_rows(metadata):
        lines.append(f"{name} {version}\n  {licence}\n  {source}")
    lines += ["", "Downloaded files (native/pins.toml)", ""]
    for name, version, licence, source in native_rows(pins_text):
        lines.append(f"{name} {version}".rstrip() + f"\n  {licence}\n  {source}")
    lines += ["", "Native components", "", native_text(pins_text).rstrip("\n")]
    lines += ["", "Licence texts of the Rust crates", ""]
    for crates, body in text_sections(texts or {}):
        lines += ["=" * 78, "Used by: " + ", ".join(crates), "=" * 78, body, ""]
    return "\n".join(lines).rstrip("\n") + "\n"


def main(argv):
    check = "--check" in argv
    args = [a for a in argv[1:] if a != "--check"]
    root = Path(args[0]) if args else Path(__file__).resolve().parent.parent
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--all-features", "--manifest-path", str(root / "rust" / "Cargo.toml")],
        check=True, capture_output=True, text=True,
    ).stdout
    metadata = json.loads(out)
    try:
        texts = licence_texts(metadata)
    except MissingTexts as err:
        print(err, file=sys.stderr)
        return 1
    text = render(metadata, (root / "native" / "pins.toml").read_text(encoding="utf-8"), texts)
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
