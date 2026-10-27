#!/usr/bin/env python3
"""Compare stripped GPUI application executables, preserving the build outputs."""

import argparse
import hashlib
import json
from pathlib import Path
import platform
import re
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--skrift', type=Path, required=True)
    parser.add_argument('--wry', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    binaries = {name: path.resolve(strict=True) for name, path in
                [('skrift', args.skrift), ('wry', args.wry)]}
    if binaries['skrift'] == binaries['wry']:
        parser.error('provide separate engine-specific executables')
    args.output_dir.mkdir(parents=True, exist_ok=False)
    records = {}
    for name, source in binaries.items():
        stripped = (args.output_dir / f'gpui-{name}').resolve()
        subprocess.run(['strip', '--strip-all', '-o', str(stripped), str(source)], check=True)
        dynamic = subprocess.check_output(['readelf', '-d', str(stripped)], text=True)
        records[name] = {
            'source': str(source), 'source_size_bytes': source.stat().st_size,
            'source_sha256': digest(source), 'stripped': str(stripped),
            'stripped_size_bytes': stripped.stat().st_size,
            'stripped_sha256': digest(stripped),
            'direct_shared_libraries': re.findall(r'\(NEEDED\).*?\[(.*?)\]', dynamic),
        }
    skrift = records['skrift']['stripped_size_bytes']
    wry = records['wry']['stripped_size_bytes']
    report = {
        'platform': platform.platform(),
        'scope': 'stripped executable file bytes; shared libraries and helpers excluded',
        'strip_version': subprocess.check_output(['strip', '--version'], text=True).splitlines()[0],
        'rustc_version': subprocess.check_output(['rustc', '--version'], text=True).strip(),
        'executables': records,
        'skrift_minus_wry_bytes': skrift - wry,
        'skrift_to_wry_size_ratio': skrift / wry,
    }
    with (args.output_dir / 'sizes.json').open('x') as handle:
        json.dump(report, handle, indent=2)
        handle.write('\n')
    for name, record in records.items():
        print(f"GPUI + {name}: {record['stripped_size_bytes'] / 1048576:.2f} MiB")
    print(f'Difference (Skrift minus Wry): {(skrift - wry) / 1048576:.2f} MiB')
    print(f'Skrift / Wry: {skrift / wry:.2f}x')


if __name__ == '__main__':
    main()
