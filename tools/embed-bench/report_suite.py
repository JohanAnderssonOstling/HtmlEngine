#!/usr/bin/env python3
"""Update README benchmark tables from a completed fixed-frequency suite summary."""

import argparse
from decimal import Decimal, ROUND_HALF_UP
from html import escape
import json
from pathlib import Path
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[2]
BASE = Path(__file__).resolve().parent
LABELS = {
    'article': 'Article', 'images': 'Images', 'tables': 'Tables',
    'arabic': 'Arabic (RTL)', 'japanese': 'Japanese (CJK)',
    'large': 'Very large document', 'middlemarch': 'Middlemarch chapter',
}
MODES = [('cold-first', 'Cold first viewport'), ('warm-first', 'Warm first viewport'),
         ('whole-surface', 'Warm whole document render')]


def number(value, decimals=1):
    rounded = Decimal(str(value)).quantize(Decimal(1).scaleb(-decimals), rounding=ROUND_HALF_UP)
    return format(rounded, f'.{decimals}f')


def relative(path):
    return quote(Path(path).resolve().relative_to(ROOT).as_posix())


def table(cases, warm):
    ram_headers = ('<th colspan="3">RAM before (MiB)</th><th colspan="3">RAM after (MiB)</th>'
                   if warm else '<th colspan="3">Total RAM (MiB)</th>')
    columns = '<th>Skrift</th><th>WebKitGTK</th><th>Size ratio</th>' * (2 if warm else 1)
    lines = ['<table>', '<thead>',
             '<tr><th rowspan="2">Document</th>'
             '<th colspan="3">CPU time (ms)</th>' + ram_headers + '</tr>',
             '<tr><th>Skrift</th><th>WebKitGTK</th><th>CPU ratio</th>' + columns + '</tr>',
             '</thead>', '<tbody>']
    for label in LABELS:
        case = next((case for case in cases if case['fixture'] == label), None)
        if case is None:
            continue
        skrift, webkit = (case['engines'][engine] for engine in ('skrift', 'wry'))
        window = case['windows_ms'][-1]
        cpu = [engine['cpu_by_window_ms'][str(window)]['median'] for engine in (skrift, webkit)]
        values = [number(cpu[0]), number(cpu[1]), number(cpu[1] / cpu[0], 2) + '×']
        for field in (['baseline_pss_mib', 'loaded_pss_mib'] if warm else ['loaded_pss_mib']):
            ram = [engine[field]['median'] for engine in (skrift, webkit)]
            values.extend([number(ram[0]), number(ram[1]), number(ram[1] / ram[0], 2) + '×'])
        lines.append('<tr><th scope="row"><a href="' + relative(case['path']) + '">'
                     + escape(LABELS[label]) + '</a></th>'
                     + ''.join('<td>' + value + '</td>' for value in values) + '</tr>')
    return '\n'.join(lines + ['</tbody>', '</table>'])


def benchmark_section(summary, summary_path):
    if not summary['complete'] or summary['completed_measurements'] != summary['planned_measurements']:
        raise ValueError('the suite must be complete before updating README')
    cases, config = summary['cases'], summary['configuration']
    keys = {(case['mode'], case['fixture']) for case in cases}
    if len(keys) != len(cases) or any(mode not in dict(MODES) for mode, _ in keys):
        raise ValueError('duplicate or unsupported benchmark case')
    for mode in ('cold-first', 'warm-first'):
        if {fixture for scope, fixture in keys if scope == mode} != set(LABELS):
            raise ValueError('first-viewport results must include all seven fixtures')
    for reused in summary['reused_cases']:
        if ((reused['mode'], reused['fixture']) in keys
                or (reused['source_mode'], reused['fixture']) not in keys):
            raise ValueError('whole-document result is duplicated or has no source')
    samples = config['samples_per_engine']
    for case in cases:
        if set(case['engines']) != {'skrift', 'wry'}:
            raise ValueError('missing engine results')
        for engine in case['engines'].values():
            if (engine['completed_samples'] != samples or not engine['kernel_cpuset_verified']
                    or not engine['frequency_limits_verified']):
                raise ValueError('incomplete samples or unverified CPU controls')
    if sum(e['completed_samples'] for c in cases for e in c['engines'].values()) != summary['completed_measurements']:
        raise ValueError('sample counts do not match the summary')
    baseline = next(c for c in cases if c['mode'] == 'warm-first' and c['fixture'] == 'article')
    ram = [baseline['engines'][e]['baseline_pss_mib']['median'] for e in ('skrift', 'wry')]
    frequency = number(config['khz'] / 1000000)
    cpus = ', '.join(map(str, config['cpus']))
    parts = [f'''## Benchmark environment

| Component | Version / configuration |
| --- | --- |
| CPU | AMD Ryzen 7 6800HS Creator Edition |
| GPUI | 0.2.2 |
| Wry binding (`lb-wry`) | 0.53.3 |
| WebKitGTK runtime | 2.54.2 |
| GTK runtime | 3.24.52 |
| Host/helper CPU set | {cpus}: four physical cores, one SMT thread per core |
| CPU frequency limits | {frequency} GHz minimum and maximum, including SMT siblings |
| First viewport | 800 × 600 pixels, scale 1, X11 |
| Samples | {samples} fresh processes per engine and case; {summary['completed_measurements']} measurements |

## Warm engine RAM

Median after empty-page warmup for the Article case, before loading its content.

<table>
<thead>
<tr><th>GPUI + Skrift</th><th>GPUI + WebKitGTK</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><td>{number(ram[0])} MiB</td><td>{number(ram[1])} MiB</td><td>{number(ram[1] / ram[0], 2)}×</td></tr>
</tbody>
</table>''']
    for mode, title in MODES:
        parts.append('## ' + title + '\n\n' + table([c for c in cases if c['mode'] == mode], mode != 'cold-first'))
    directory = summary_path.parent
    parts.append('Raw results: [summary](toolpath), [measurement plan](planpath) and [validation](validationpath). '
                 'Individual samples, CPU checkpoints and ranges are retained in the results directory.'
                 .replace('toolpath', relative(summary_path))
                 .replace('planpath', relative(directory / 'plan.json'))
                 .replace('validationpath', relative(directory / 'validation.json')))
    return '\n\n'.join(parts) + '\n\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--summary', type=Path,
                        default=BASE / 'results/fixed-frequency-suite-20261101-204746/summary.json')
    parser.add_argument('--readme', type=Path, default=ROOT / 'README.md')
    args = parser.parse_args()
    summary_path = args.summary.resolve(strict=True)
    content = benchmark_section(json.loads(summary_path.read_text()), summary_path)
    readme = args.readme.read_text()
    start = readme.index('## Benchmark environment')
    end = readme.index('## Executable size', start)
    args.readme.write_text(readme[:start] + content + readme[end:])
    print('Updated README benchmark results and measurement notes.')


if __name__ == '__main__':
    main()
