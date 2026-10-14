#!/usr/bin/env python3
"""Writes the notices of the third-party software shipped with a build.

The licences of the Rust crates compiled into the program come from the
dependency tree (`cargo tree`, normal dependencies of the target platform,
without proc-macros, which do not end up in the binary) and from the licence
files of each crate. Identical texts (the Apache licence, mostly) are written
once, with the list of crates that use them.

    scripts/third_party_notices.py --package form-pdf-reader \\
        --target x86_64-unknown-linux-gnu --pdfium vendor/pdfium --out NOTICES.txt

--pdfium adds PDFium and its components (the licences in the PDFium binary
distribution); --android adds the Java/Kotlin libraries of the Android app.
"""

import argparse
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STANDARD = os.path.join(ROOT, "packaging", "licenses")
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|unlicense|ofl|ufl)", re.I)
RULE = "=" * 78
THIN = "-" * 78

MIT = """Copyright (c) {who}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""

BSD3 = """Copyright (c) {who}

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors
   may be used to endorse or promote products derived from this software
   without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
"""

# Java/Kotlin libraries of the Android app (android/app/build.gradle.kts):
# under the Apache License 2.0, except jmulticard and Bouncy Castle (below).
ANDROID_LIBRARIES = """\
- AndroidX libraries (androidx.*): The Android Open Source Project.
- Material Components for Android (com.google.android.material): Google LLC.
- Kotlin standard library (org.jetbrains.kotlin): JetBrains s.r.o. and Kotlin
  Programming Language contributors.
- JNA, Java Native Access (net.java.dev.jna), dual-licensed under the LGPL 2.1
  or later and the Apache License 2.0; used here under the Apache License 2.0.

For the DNIe (Spanish national ID card) over NFC:

- jmulticard (es.gob.afirma.jmulticard: jmulticard, jmulticard-crypto,
  jmulticard-android) 2.1, by the Government of Spain (Secretaría General de
  Administración Digital), dual-licensed under the GNU General Public License
  version 2 and the European Union Public Licence (EUPL) version 1.1; used
  here under the EUPL 1.1 (https://joinup.ec.europa.eu/collection/eupl).
  Source code: https://github.com/ctt-gob-es/jmulticard
  Because of this library, the Android app as a whole is distributed under
  the EUPL 1.1; the source code of Form PDF Reader keeps its MIT licence.
- Bouncy Castle (org.bouncycastle), the parts included in jmulticard-crypto,
  The Legion of the Bouncy Castle Inc., under the MIT licence:

  Copyright (c) 2000-2026 The Legion of the Bouncy Castle Inc.
  (https://www.bouncycastle.org)

  Permission is hereby granted, free of charge, to any person obtaining a
  copy of this software and associated documentation files (the "Software"),
  to deal in the Software without restriction, including without limitation
  the rights to use, copy, modify, merge, publish, distribute, sublicense,
  and/or sell copies of the Software, and to permit persons to whom the
  Software is furnished to do so, subject to the following conditions:

  The above copyright notice and this permission notice shall be included in
  all copies or substantial portions of the Software.

  THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
  IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
  FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
  AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
  LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
  FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
  DEALINGS IN THE SOFTWARE.
"""


def read(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        return f.read().replace("\r\n", "\n").strip() + "\n"


# Workspace crates copied from other projects (with their licence files).
VENDORED = {"egui_software_backend"}


def crates(package, target):
    """(name, version) of the crates compiled into `package` for `target`."""
    meta = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked"], cwd=ROOT))
    packages = {(p["name"], p["version"]): p for p in meta["packages"]}
    tree = subprocess.check_output(
        ["cargo", "tree", "-p", package, "-e", "normal", "--target", target, "--prefix", "none", "--format", "{p}"],
        cwd=ROOT, text=True,
    )
    found = set()
    for line in tree.splitlines():
        m = re.match(r"(\S+) v(\S+)", line)
        if not m or "(proc-macro)" in line:
            continue
        p = packages[(m[1], m[2])]
        # Workspace crates are this project, except copies of other projects.
        if p["source"] is not None or p["name"] in VENDORED:
            found.add((m[1], m[2]))
    return sorted(found), packages


def mit_choice(expr):
    """True for an expression like "MIT OR Apache-2.0": the crate is used under MIT."""
    if not expr or " AND " in expr or "(" in expr:
        return False
    return "MIT" in re.split(r"\s+OR\s+|/", expr)


def license_texts(p):
    """Licence texts of a crate: its own files, or the standard text."""
    d = os.path.dirname(p["manifest_path"])
    expr = p.get("license") or ""
    files = []
    for entry in sorted(os.listdir(d)):
        path = os.path.join(d, entry)
        if os.path.isfile(path) and LICENSE_FILE.match(entry):
            files.append(path)
        elif os.path.isdir(path) and entry in ("fonts", "licenses", "LICENSES"):
            files += [os.path.join(path, f) for f in sorted(os.listdir(path)) if f.endswith(".txt") or LICENSE_FILE.match(f)]
    if p.get("license_file"):
        files.append(os.path.join(d, p["license_file"]))
    texts = [read(f) for f in dict.fromkeys(files)]
    if mit_choice(expr):
        # Used under MIT: its MIT text (and notices), not the alternatives.
        mit = [t for t in texts if "Permission is hereby granted" in t or re.match(r"(?i)\s*notice", t)]
        texts = mit or []
    if texts:
        return texts
    # No licence file in the package: the standard text of its licence.
    who = ", ".join(a.split("<")[0].strip() for a in p.get("authors", [])) or f"the {p['name']} authors"
    if mit_choice(expr) or re.search(r"\bMIT\b", expr):
        return [MIT.format(who=who)]
    if "BSD-3-Clause" in expr:
        return [BSD3.format(who=who)]
    if "MPL-2.0" in expr:
        return [read(os.path.join(STANDARD, "MPL-2.0.txt"))]
    if "Apache-2.0" in expr:
        return [read(os.path.join(STANDARD, "Apache-2.0.txt"))]
    if "BSL-1.0" in expr:
        return [read(os.path.join(STANDARD, "BSL-1.0.txt"))]
    sys.exit(f"{p['name']} {p['version']}: no licence file and no standard text for '{expr}'")


def section(out, title):
    out.append(f"\n{RULE}\n{title}\n{RULE}\n")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--package", required=True)
    ap.add_argument("--target", required=True)
    ap.add_argument("--pdfium", help="PDFium binary distribution (with LICENSE and licenses/)")
    ap.add_argument("--android", action="store_true", help="add the Java/Kotlin libraries of the Android app")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    out = [
        "Form PDF Reader - licences\n",
        "Form PDF Reader is distributed under the MIT License. It includes the\n"
        "third-party software listed below, under the licences that follow.\n",
    ]
    section(out, "Form PDF Reader")
    out.append(read(os.path.join(ROOT, "LICENSE")))

    if args.pdfium:
        section(out, "PDFium, with V8 and XFA, and its components")
        out.append("PDFium (https://pdfium.googlesource.com/pdfium), built by the pdfium-binaries\n"
                   "project (https://github.com/bblanchon/pdfium-binaries).\n")
        lic = os.path.join(args.pdfium, "licenses")
        for f in sorted(os.listdir(lic)):
            name = os.path.splitext(f)[0]
            out.append(f"\n{THIN}\n{name}\n{THIN}\n\n{read(os.path.join(lic, f))}")
        out.append(f"\n{THIN}\npdfium-binaries (build scripts)\n{THIN}\n\n{read(os.path.join(args.pdfium, 'LICENSE'))}")

    # Letras de las firmas escritas (crates/core/assets/signature-fonts),
    # incluidas en el programa: el escritorio y Android.
    fonts = os.path.join(ROOT, "crates", "core", "assets", "signature-fonts")
    section(out, "Signature fonts")
    for f in sorted(os.listdir(fonts)):
        if f.endswith(".txt"):
            name = f.rsplit("-", 1)[0]
            out.append(f"\n{THIN}\n{name}\n{THIN}\n\n{read(os.path.join(fonts, f))}")

    if args.android:
        section(out, "Android libraries")
        out.append(ANDROID_LIBRARIES)
        out.append(f"\n{THIN}\nApache License 2.0\n{THIN}\n\n{read(os.path.join(STANDARD, 'Apache-2.0.txt'))}")

    found, packages = crates(args.package, args.target)
    section(out, f"Rust crates ({len(found)})")
    # Texts that differ only in spacing are written once.
    groups = {}
    for key in found:
        p = packages[key]
        expr = p.get("license") or "see text"
        used = "MIT" if mit_choice(p.get("license")) else expr
        for text in license_texts(p):
            g = groups.setdefault(" ".join(text.split()), [text, []])
            # MPL-2.0: where its source code is available.
            source = f" - source code: {p['repository']}" if "MPL" in expr and p.get("repository") else ""
            g[1].append(f"{p['name']} {p['version']} ({used}){source}")
    # The texts used by most crates first.
    for text, users in sorted(groups.values(), key=lambda g: (-len(g[1]), g[1][0])):
        out.append(f"\n{THIN}\n" + "\n".join(users) + f"\n{THIN}\n\n{text}")

    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as f:
        f.write("".join(out))
    print(f"{args.out}: {len(found)} crates, {len(groups)} licence texts")


if __name__ == "__main__":
    main()
