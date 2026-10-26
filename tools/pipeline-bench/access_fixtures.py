#!/usr/bin/env python3
"""Generate deterministic workloads for the hot access audit."""
import argparse
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('directory', type=Path)
args = parser.parse_args()
args.directory.mkdir(parents=True, exist_ok=True)
text = 'The quick brown fox jumps over the lazy dog. Stable text metrics keep line breaks reproducible. '
fixtures = {
    'first_line': ('p::first-line{font-size:18px;color:#246}', ''.join(f'<p>{text * 3}</p>' for _ in range(160))),
    'first_line_links': ('p::first-line{font-size:18px;color:#246}', ''.join(f'<p id="p{i}"><a href="#p{i}">{text * 3}</a></p>' for i in range(160))),
    'style': ('body{font-variant-numeric:tabular-nums; font-feature-settings:"kern" 1,"liga" 1} .grid{display:grid;grid-template-columns:1fr 2fr}',
              ''.join(f'<div class="grid"><p style="color:rgb({i % 200},30,40);padding-left:{i % 17}px">{text}</p></div>' for i in range(600))),
    'plain': ('', ''.join(f'<p>{text * 3}</p>' for _ in range(300))),
    'mixed': ('em{display:inline-block;width:24px}', ''.join(f'<p>{text}<em>box</em>{text}</p>' for _ in range(300))),
    'table': ('td{padding:2px}table{width:580px}', '<table>' + ''.join('<tr>' + ''.join(f'<td>{text * 2}</td>' for _ in range(4)) + '</tr>' for _ in range(90)) + '</table>'),
    'table_parallel': ('td{padding:2px}table{width:580px}', '<table>' + ''.join('<tr>' + ''.join(f'<td><div style="height:50%;overflow:hidden">{text * 2}</div></td>' for _ in range(4)) + '</tr>' for _ in range(90)) + '</table>'),
    'table_batches': ('td{padding:2px}table{width:580px}', ''.join('<table>' + ''.join('<tr>' + ''.join(f'<td><div style="height:50%;overflow:hidden">{text * 8}</div></td>' for _ in range(2)) + '</tr>' for _ in range(4)) + '</table>' for _ in range(12))),
    'controls': ('', ''.join('<p>' + ('a\u00adb\u200ec\u200fd\u00ade ' * 60) + '</p>' for _ in range(160))),
    'soft_hyphens': ('', ''.join('<p>' + ('a\u00adb\u00adc\u00add ' * 60) + '</p>' for _ in range(160))),
    'first_line_nested': ('.outer::first-line{font-size:18px}.atom::first-line{font-size:20px}.atom{display:inline-block;width:120px}', '<div class="outer">Before text <span class="atom">Inner words <i>more words</i></span> after text trailing words.</div>'),
}
for name, (style, body) in fixtures.items():
    (args.directory / f'{name}.html').write_text(f'<!doctype html><style>body{{margin:0;font-size:16px}}p{{margin:0 0 8px}}{style}</style><body>{body}</body>', encoding='utf-8')
