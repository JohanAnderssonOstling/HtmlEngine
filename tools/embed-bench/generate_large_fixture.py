#!/usr/bin/env python3
"""Generate a reproducible large document using original synthetic prose."""

import argparse
from html import escape
from pathlib import Path


SENTENCES = (
    "The library opens its reading room every morning while the city slowly fills with light.",
    "Readers compare maps and diagrams, leaving careful notes beside the passages they want to revisit.",
    "A catalogue connects each volume to the people, places and questions described within its pages.",
    "Across the courtyard, a workshop prepares paper and bindings for another collection of books.",
    "The quiet arrangement of shelves makes a long document easier to explore one section at a time.",
    "Clear paragraphs preserve the rhythm of the story even when the window changes its shape.",
    "An editor reviews the examples and records small corrections before the next edition is printed.",
    "Beyond the windows, passing trains carry letters and travellers toward distant towns and villages.",
    "Every chapter adds a different perspective without requiring an image or a script to explain it.",
    "At the end of the day, the unfinished work remains on the desk for tomorrow's visitors.",
)


def document(paragraphs):
    parts = ['''<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<title>Embedding benchmark: very large document</title>
<style>
html { overflow: hidden; font: 16px/1.5 "DejaVu Sans", sans-serif; background: white; color: #20242a; }
body { margin: 0; padding: 24px; }
h1 { font-size: 30px; line-height: 1.2; margin: 0 0 16px; }
h2 { font-size: 20px; margin: 24px 0 8px; }
p { margin: 0 0 16px; }
</style></head><body>
<h1>The reading room: a very large document</h1>
''']
    for index in range(paragraphs):
        if index % 50 == 0:
            parts.append(f'<h2>Chapter {index // 50 + 1}</h2>\n')
        sentences = [SENTENCES[(index + offset) % len(SENTENCES)] for offset in range(6)]
        parts.append(f'<p id="paragraph-{index + 1}"><strong>Entry {index + 1}.</strong> '
                     + escape(" ".join(sentences)) + '</p>\n')
    parts.append('</body></html>\n')
    return ''.join(parts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--paragraphs', type=int, default=5000)
    parser.add_argument('--output', type=Path,
                        default=Path(__file__).with_name('fixtures') / 'large-document.html')
    args = parser.parse_args()
    if args.paragraphs <= 0:
        parser.error('--paragraphs must be positive')
    if args.output.exists():
        parser.error(f'refusing to overwrite {args.output}')
    source = document(args.paragraphs)
    args.output.write_text(source, encoding='utf-8')
    print(f'{args.paragraphs:,} paragraphs, {len(source.encode("utf-8")):,} bytes: {args.output}')


if __name__ == '__main__':
    main()
