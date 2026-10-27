#!/usr/bin/env python3
"""Generate deterministic Latin workloads for CJK fast-path comparisons."""
import argparse
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('directory', type=Path)
args = parser.parse_args()
args.directory.mkdir(parents=True, exist_ok=True)

paragraphs = {
    'ascii': 'The quick brown fox jumps over the lazy dog. Reading long paragraphs should remain fast, with stable line breaks and predictable spacing. ',
    'latin': 'Ångström, café, naïve, déjà vu. Français, español, português, svenska och íslenska: läsning och typografi för många språk. ',
    'long': 'A long chapter mixes ordinary words, numbers 12345, punctuation (like this), and emphasized passages. ',
    'narrow': 'Narrow columns contain English words and punctuation; they must wrap without adding extra allocations. ',
}
paragraphs['tagged'] = paragraphs['ascii']
for name, text in paragraphs.items():
    count = 900 if name == 'long' else 300
    width = 'width:160px;' if name == 'narrow' else ''
    style = f'body{{margin:0;font-size:16px}}p{{margin:0 0 8px;{width}}}'
    body = ''.join(f'<p>{text * 3}</p>' for _ in range(count))
    language = ' lang="en-US"' if name == 'tagged' else ''
    (args.directory / f'{name}.html').write_text(
        f'<!doctype html><style>{style}</style><body{language}>{body}</body>', encoding='utf-8')
