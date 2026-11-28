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
licences; the licence files found in each crate's source are copied at the end, and ONNX Runtime, the corner model and
pdf.js have their own section. The file is made by tools/notices.py from rust/Cargo.lock and native/pins.toml.

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

pdf.js 4.10.38 (Apache-2.0), Copyright 2012 Mozilla Foundation, https://github.com/mozilla/pdf.js

  The JBIG2 decoder in rust/lazy-pdf-filters/src/jbig2 is ported from src/core/jbig2.js and src/core/arithmetic_decoder.js,
  by way of Lazy PDF's C# and Swift ports, and changed: it is written in Rust, reads Group 4 data through the CCITT decoder
  of the same crate, takes the generic region template bits in the order of ITU-T T.88, adds the parts pdf.js lacks (Huffman
  coded symbol refinement, skipped halftone cells, typical prediction of refinement rows, the other combination operators,
  refinement region segments) and limits the sizes and counts a stream may ask for. Each of those files carries the licence
  header of the original and says it was changed. Licensed under the Apache License, Version 2.0; the licence text is at
  https://www.apache.org/licenses/LICENSE-2.0. The software is distributed on an "AS IS" basis, without warranties or
  conditions of any kind.

Unicode Character Database 17.0.0 (rust/lazy-pdf-text/src/bidi_tables.rs, testdata/pdf-fonts/bidi)
  Bidi_Class values, paired brackets and mirroring, and the lines of BidiCharacterTest-17.0.0.txt, Copyright 1991-2025
  Unicode, Inc., under the Unicode License v3:

  UNICODE LICENSE V3

  COPYRIGHT AND PERMISSION NOTICE

  Copyright © 1991-2025 Unicode, Inc.

  NOTICE TO USER: Carefully read the following legal agreement. BY
  DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
  SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
  TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
  DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

  Permission is hereby granted, free of charge, to any person obtaining a
  copy of data files and any associated documentation (the "Data Files") or
  software and any associated documentation (the "Software") to deal in the
  Data Files or Software without restriction, including without limitation
  the rights to use, copy, modify, merge, publish, distribute, and/or sell
  copies of the Data Files or Software, and to permit persons to whom the
  Data Files or Software are furnished to do so, provided that either (a)
  this copyright and permission notice appear with all copies of the Data
  Files or Software, or (b) this copyright and permission notice appear in
  associated Documentation.

  THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
  KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
  MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
  THIRD PARTY RIGHTS.

  IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
  BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
  OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
  WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
  ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
  FILES OR SOFTWARE.

  Except as contained in this notice, the name of a copyright holder shall
  not be used in advertising or otherwise to promote the sale, use or other
  dealings in these Data Files or Software without prior written
  authorization of the copyright holder.

Adobe predefined CJK CMaps (rust/lazy-pdf-fonts/src/data.rs)
  61 character collection maps, character id mappings only, from Adobe's
  cmap-resources, under the BSD 3-clause license:

  Copyright 1990-2023 Adobe. All rights reserved.

  Redistribution and use in source and binary forms, with or without
  modification, are permitted provided that the following conditions are met:

  Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

  Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

  Neither the name of Adobe nor the names of its contributors may be used to
  endorse or promote products derived from this software without specific
  prior written permission.

  THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
  AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
  IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
  ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
  LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
  CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
  SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
  INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
  CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
  ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
  POSSIBILITY OF SUCH DAMAGE.


Adobe Glyph List and ZapfDingbats glyph list (rust/lazy-pdf-fonts/src/data.rs)
  Adobe Systems, under the BSD 3-clause license:

  Copyright 2002-2019 Adobe (http://www.adobe.com/).

  Redistribution and use in source and binary forms, with or without
  modification, are permitted provided that the following conditions are met:

  Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

  Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

  Neither the name of Adobe nor the names of its contributors may be used to
  endorse or promote products derived from this software without specific
  prior written permission.

  THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
  AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
  IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
  ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
  LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
  CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
  SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
  INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
  CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
  ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
  POSSIBILITY OF SUCH DAMAGE.


Adobe Core 14 font metrics (rust/lazy-pdf-fonts/src/data.rs)
  Widths and font-wide values of the Adobe AFM files for the 14 standard
  PDF fonts. Adobe Systems. Adobe's terms for the AFM files:

  Copyright (c) 1985, 1987, 1989, 1990, 1997 Adobe Systems Incorporated.
  All Rights Reserved.

  This file and the 14 PostScript(R) AFM files it accompanies may be used,
  copied, and distributed for any purpose and without charge, with or without
  modification, provided that all copyright notices are retained; that the
  AFM files are not distributed without this file; that all modifications to
  this file or any of the AFM files are prominently noted in the modified
  file(s); and that this paragraph is not modified. Adobe Systems has no
  responsibility or obligation to support the use of the AFM files.
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
