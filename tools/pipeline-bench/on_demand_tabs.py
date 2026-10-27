#!/usr/bin/env python3
"""Compare tab resolution, retained storage and rendering with frozen binaries."""

import argparse
import datetime
import hashlib
import itertools
import json
import os
from pathlib import Path
import subprocess


def fixtures(directory):
    directory.mkdir(parents=True, exist_ok=True)
    texts = [('short', 'a\tb\tc'), ('long', 'alpha\tbeta\tX Y Z\n' * 8), ('minimum', 'aa\tb')]
    styles = [
        ('pre', 'white-space:pre'), ('wrap', 'white-space:pre-wrap;width:90px'),
        ('breakspaces', 'white-space:break-spaces;width:90px'),
        ('collapsed', 'white-space:normal'),
        ('ellipsis', 'white-space:pre;width:60px;overflow:hidden;text-overflow:ellipsis'),
        ('rtl', 'white-space:pre;direction:rtl'),
    ]
    paths = []
    for (name, text), (style_name, style), tab_size in itertools.product(texts, styles, ['4', '18px', '0']):
        path = directory / f'{name}-{style_name}-{tab_size}.html'
        path.write_text(f'<html><body style="margin:0"><div style="font-size:16px;tab-size:{tab_size};{style}">{text}</div></body></html>')
        paths.append(str(path.resolve()))
    path = directory / 'nested-block-and-inline.html'
    path.write_text('<html><body style="margin:0"><pre style="font:16px Ahem;tab-size:4;text-indent:8px">a<span style="font-size:32px;tab-size:18px">\tb</span><span style="tab-size:30px">\tc</span>\nX\tY</pre></body></html>')
    paths.append(str(path.resolve()))
    return paths


def collect(args):
    os.sched_setaffinity(0, {args.cpu})
    binaries = {'before': args.before.resolve(), 'after': args.after.resolve()}
    groups = {'book': json.loads(args.spine.read_text()), 'tab_fixtures': fixtures(args.fixtures)}
    result = {
        'measured_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'binary_sha256': {name: hashlib.sha256(path.read_bytes()).hexdigest()
                          for name, path in binaries.items()},
        'method': {'cpu': args.cpu, 'book_samples': 5, 'fixture_samples': 3,
                   'warmups': 3, 'backend': 'deterministic mock', 'composition': 'BookOptimized',
                   'case': 'cold_book',
                   'limits': 'Warm OS caches; fresh session/shaper per opening. Single screening run per section, alternate binary order. Instruction and retained-memory counts are primary measurements; clocks can vary with machine load. Native fonts, image decode and UI excluded.'},
        'source_changes': json.loads(args.source_changes.read_text()) if args.source_changes else [],
    }
    for group, paths in groups.items():
        result[group] = []
        for index, path in enumerate(paths):
            row = {'path': path}
            for name in list(binaries)[::1 if index % 2 == 0 else -1]:
                row[name] = json.loads(subprocess.check_output(
                    [str(binaries[name]), path, 'cold_book', '5' if group == 'book' else '3', 'default'],
                    text=True))
            for field in ['source_fingerprint', 'text_color_fingerprint', 'glyphs']:
                assert row['before'][field] == row['after'][field], (path, field)
            row['identical_rendering'] = row['before']['render_fingerprint'] == row['after']['render_fingerprint']
            if not row['identical_rendering']:
                assert group == 'tab_fixtures'
                assert any(marker in Path(path).name for marker in ['-wrap-', '-breakspaces-'])
                assert not Path(path).name.endswith('-0.html')
                row['rendering_change'] = 'Zero-width preserved-space wrap opportunities no longer inherit tab advancement.'
            if group == 'book':
                assert row['before']['lines'] == row['after']['lines']
            result[group].append(row)
        totals = {}
        for name in binaries:
            def memory(suffix, field):
                return sum(entry[field] for row in result[group] for entry in row[name]['memory_breakdown']
                           if entry['label'].endswith(suffix))
            totals[name] = {
                'metrics_count': memory('inline_plans.run_metrics', 'count'),
                'metrics_bytes': memory('inline_plans.run_metrics', 'bytes'),
                'tokens_count': memory('inline_plans.tokens', 'count'),
                'tokens_bytes': memory('inline_plans.tokens', 'bytes'),
                'user_instructions': sum(row[name]['user_instructions'] for row in result[group]),
                'sum_section_cpu_medians_ms': sum(row[name]['cpu_median_ms'] for row in result[group]),
            }
        result[group + '_totals'] = totals
        print(group, len(paths), 'identical rendering', sum(row['identical_rendering'] for row in result[group]), flush=True)
        print(totals, flush=True)
        # Preserve completed groups even if a later fixture exposes a regression.
        args.output.write_text(json.dumps(result, indent=2) + '\n')
    result['tab_fixture_sources'] = {Path(path).name: Path(path).read_text() for path in groups['tab_fixtures']}
    args.output.write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('spine', type=Path)
    parser.add_argument('--fixtures', type=Path, default=Path('target/tab-fixtures'))
    parser.add_argument('--source-changes', type=Path)
    parser.add_argument('--cpu', type=int, default=8)
    parser.add_argument('--output', type=Path, required=True)
    arguments = parser.parse_args()
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    collect(arguments)
