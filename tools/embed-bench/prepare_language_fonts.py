#!/usr/bin/env python3
"""Give both hosts the application's CJK font without installing system fonts.

Requires fontTools. A private family name avoids selecting the system Droid
font with the same original family name, whose cmap lacks Hangul on this host.
Glyphs and metrics are unchanged. Existing output directories are preserved.
"""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
from xml.sax.saxutils import escape

from fontTools.ttLib import TTFont

FAMILY = "Skrift Benchmark CJK"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--license", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    license_path = args.license.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    fonts = output / "fonts"
    fonts.mkdir()
    target = fonts / "SkriftBenchmarkCJK.ttf"
    with TTFont(source, recalcTimestamp=False) as font:
        names = {1:FAMILY, 2:"Regular", 3:"SkriftBenchmarkCJK-Regular", 4:FAMILY,
                 6:"SkriftBenchmarkCJK-Regular", 16:FAMILY, 17:"Regular"}
        for record in font["name"].names:
            if record.nameID in names:
                record.string = names[record.nameID].encode(record.getEncoding())
        font.save(target)
    shutil.copyfile(license_path, output / "LICENSE-DROID.txt")
    config = output / "fonts.conf"
    config.write_text(f'''<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <include>/etc/fonts/fonts.conf</include>
  <dir>{escape(str(fonts))}</dir>
  <cachedir>{escape(str(output / "cache"))}</cachedir>
</fontconfig>
''')
    manifest = {"family":FAMILY, "source":str(source),
                "source_sha256":hashlib.sha256(source.read_bytes()).hexdigest(),
                "font":str(target), "font_sha256":hashlib.sha256(target.read_bytes()).hexdigest(),
                "fontconfig":str(config), "change":"name table only; glyphs and metrics retained"}
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(config)


if __name__ == "__main__":
    main()
