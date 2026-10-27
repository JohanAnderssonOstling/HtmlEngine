#!/usr/bin/env python3
"""Update the project README from the three completed GPUI measurement reports."""

import argparse
from decimal import Decimal, ROUND_HALF_UP
from html import escape
import json
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[2]
BASE = Path(__file__).resolve().parent
GITHUB = 'https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/'
FIXTURES = [
    ('article.html', 'Article'), ('images.html', 'Images'), ('tables.html', 'Tables'),
    ('languages/rtl-arabic.html', 'Arabic (RTL)'),
    ('languages/cjk-japanese.html', 'Japanese (CJK)'),
    ('large-document.html', 'Very large document'),
    ('books/middlemarch/chapter-37.xhtml', 'Middlemarch chapter'),
]
CASES = [('cold', 'Cold first rendered frame'), ('warm', 'Warm first rendered frame'),
         ('whole', 'Warm whole document render')]


def number(value, decimals=0):
    rounded = Decimal(str(value)).quantize(Decimal(1).scaleb(-decimals), rounding=ROUND_HALF_UP)
    return format(rounded, f'.{decimals}f')


def load_report(path, warm, whole):
    report = json.loads(path.read_text())
    if not report['complete'] or not report.get('shared_gpui'):
        raise ValueError(f'incomplete or incompatible report: {path}')
    if bool(report.get('warm_engine')) != warm or bool(report.get('full_document')) != whole:
        raise ValueError(f'wrong workload scope: {path}')
    pairs = {}
    for row in report['runs']:
        fixture = str(row['fixture']).replace('\\', '/')
        relative = fixture.rsplit('/fixtures/', 1)[-1]
        if relative.startswith('fixtures/'):
            relative = relative[len('fixtures/'):]
        pair = pairs.setdefault(relative, {})
        if row['label'] in pair or row['label'] not in ('skrift', 'wry'):
            raise ValueError(f'duplicate or unexpected host: {path}')
        pair[row['label']] = row
        if whole and not row.get('full_document_covered'):
            raise ValueError(f'unverified whole-document coverage: {path}')
    if any(set(pair) != {'skrift', 'wry'} for pair in pairs.values()):
        raise ValueError(f'missing engine result: {path}')
    return report, pairs


def table(pairs, warm):
    ram_headers = ('<th colspan="3">RAM before (MiB)</th><th colspan="3">RAM after (MiB)</th>'
                   if warm else '<th colspan="3">Total RAM (MiB)</th>')
    ram_columns = '<th>Skrift</th><th>Wry</th><th>Size ratio</th>' * (2 if warm else 1)
    lines = ['<table>', '<thead>',
             '<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th>'
             + ram_headers + '</tr>',
             '<tr><th>Skrift</th><th>Wry</th><th>Speedup</th>' + ram_columns + '</tr>',
             '</thead>', '<tbody>']
    for relative, label in FIXTURES:
        if relative not in pairs:
            continue
        a, b = pairs[relative]['skrift'], pairs[relative]['wry']
        after = [row['memory']['loaded']['pss_bytes'] for row in (a, b)]
        values = [a['additional_cpu_ms'], b['additional_cpu_ms'],
                  b['additional_cpu_ms'] / a['additional_cpu_ms']]
        if warm:
            before = [row['memory']['initialized']['pss_bytes'] for row in (a, b)]
            values.extend([before[0] / 1048576, before[1] / 1048576, before[1] / before[0]])
        values.extend([after[0] / 1048576, after[1] / 1048576, after[1] / after[0]])
        url = GITHUB + quote('tools/embed-bench/fixtures/' + relative)
        cells = ''.join('<td>' + number(value, 2 if i in (2, 5, 8) else 0) + ('×' if i in (2, 5, 8) else '') + '</td>'
                        for i, value in enumerate(values))
        lines.append(f'<tr><th scope="row"><a href="{escape(url)}">{escape(label)}</a></th>{cells}</tr>')
    return '\n'.join(lines + ['</tbody>', '</table>'])


def warm_baseline_table(pair):
    skrift, wry = (pair[engine]['memory']['initialized']['pss_bytes'] for engine in ('skrift', 'wry'))
    return ('<table>\n<thead>\n<tr><th>GPUI + Skrift</th><th>GPUI + Wry</th><th>Size ratio</th></tr>\n'
            '</thead>\n<tbody>\n<tr><td>' + number(skrift / 1048576) + ' MiB</td><td>'
            + number(wry / 1048576) + ' MiB</td><td>' + number(wry / skrift, 2)
            + '×</td></tr>\n</tbody>\n</table>')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for mode, _ in CASES:
        scope = {'cold': 'cold-first', 'warm': 'warm-first', 'whole': 'warm-whole'}[mode]
        parser.add_argument('--' + mode, type=Path,
                            default=BASE / f'results/gpui-culled-suite-{scope}-20261027.json')
    parser.add_argument('--readme', type=Path, default=ROOT / 'README.md')
    args = parser.parse_args()
    reports, pairs = {}, {}
    for mode, _ in CASES:
        reports[mode], pairs[mode] = load_report(getattr(args, mode), mode != 'cold', mode == 'whole')
    expected = {relative for relative, _ in FIXTURES}
    if set(pairs['cold']) != expected or set(pairs['warm']) != expected:
        raise ValueError('cold and warm first-frame reports must contain all seven fixtures')
    significant = {relative for relative, pair in pairs['cold'].items()
                   if any(row['host']['page']['content_height'] >= 1800 for row in pair.values())}
    if set(pairs['whole']) != significant:
        raise ValueError('whole-document results must cover fixtures at least three viewports tall')
    hashes = {host['sha256'] for report in reports.values() for host in report['hosts']}
    if len(hashes) != 1:
        raise ValueError('all measurements must use one common executable')
    baseline = '## Warm engine RAM\n\n' + warm_baseline_table(pairs['warm'][FIXTURES[0][0]])
    sections = '\n\n'.join('## ' + title + '\n\n' + table(pairs[mode], mode != 'cold')
                            for mode, title in CASES)
    note = ('One sample per engine and document. CPU includes viewport snapshot readback. '
            'RAM is total process session PSS including helpers and excluding GPU VRAM. '
            'Cold shows total RAM after rendering; warm tables show RAM before and after loading. '
            'Cold starts from an empty GPUI window; warm starts after the engine renders an empty document. '
            'Layout differences remain between engines.\n\nRaw results: '
            '[cold](tools/embed-bench/results/gpui-culled-suite-cold-first-20261027.json), '
            '[warm](tools/embed-bench/results/gpui-culled-suite-warm-first-20261027.json), '
            '[whole document](tools/embed-bench/results/gpui-culled-suite-warm-whole-20261027.json).')
    readme = args.readme.read_text()
    start = readme.index('## Warm engine RAM')
    end = readme.index('## Executable size', start)
    args.readme.write_text(readme[:start] + baseline + '\n\n' + sections + '\n\n' + note + '\n\n' + readme[end:])
    print('Updated README measurement tables.')


if __name__ == '__main__':
    main()
