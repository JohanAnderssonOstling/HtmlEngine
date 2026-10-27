#!/usr/bin/env python3
"""Compare isolated and book-shared CSS caches in spine order."""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import xml.etree.ElementTree as ET


def median(values):
    return statistics.median(values)


def compare(before, after):
    return {'isolated_ms': before, 'shared_ms': after,
            'saved_ms': before - after,
            'change_percent': 100 * (after / before - 1) if before else None}


def collect(args):
    os.sched_setaffinity(0, {args.cpu})
    binary = args.binary.resolve()
    book = args.book.resolve()
    ns = {'o': 'http://www.idpf.org/2007/opf'}
    package = ET.parse(book / 'content.opf').getroot()
    manifest = {item.attrib['id']: item.attrib['href']
                for item in package.findall('o:manifest/o:item', ns)}
    spine = [manifest[item.attrib['idref']]
             for item in package.findall('o:spine/o:itemref', ns)]
    paths = [str(book / href) for href in spine]
    captures = []
    with tempfile.TemporaryDirectory(prefix='book-css-', dir=args.output.parent) as directory:
        path_list = Path(directory) / 'spine.json'
        path_list.write_text(json.dumps(paths))
        for round_index in range(args.rounds):
            row = {}
            for kind, case in [('plain', 'book_css'), ('profile', 'book_css_profile')]:
                row[kind] = json.loads(subprocess.check_output(
                    [str(binary), str(path_list), case, str(args.samples)], text=True))
                assert row[kind]['all_outputs_identical']
            assert row['plain']['identities'] == row['profile']['identities']
            if captures:
                assert row['plain']['identities'] == captures[0]['plain']['identities']
            captures.append(row)
            print(f'Round {round_index + 1}/{args.rounds}: outputs identical', flush=True)
    independent = []
    if args.reference_binary:
        for index, path in enumerate(paths):
            row = json.loads(subprocess.check_output(
                [str(args.reference_binary.resolve()), path, 'open_deep_wall_book', '1', 'default'],
                text=True))
            expected = {key: row[key] for key in ['render_fingerprint', 'glyphs', 'lines']}
            assert expected == captures[0]['plain']['identities'][index], path
            independent.append(expected)

    def values(kind, variant, index, metric):
        return [value for capture in captures
                for value in capture[kind]['timings'][f'{variant}/{index}/{metric}']['samples_ms']]

    comparisons = {}
    for metric in ['cpu', 'wall']:
        for subset in ['book', 'first', 'later']:
            times = {}
            for variant in ['isolated', 'shared']:
                if subset == 'book':
                    series = values('plain', variant, 'book', metric)
                elif subset == 'first':
                    series = values('plain', variant, 0, metric)
                else:
                    series = [total - first for total, first in zip(
                        values('plain', variant, 'book', metric), values('plain', variant, 0, metric))]
                times[variant] = median(series)
            comparisons[f'{subset}_{metric}'] = compare(times['isolated'], times['shared'])
    metrics = ['prepare.style', 'prepare.stylesheets'] + [
        key.split('/', 2)[2] for key in captures[0]['profile']['timings']
        if key.startswith('isolated/0/style.')]
    style_comparisons = {}
    for metric in metrics:
        totals = {variant: sum(median(values('profile', variant, index, metric))
                               for index in range(len(spine))) for variant in ['isolated', 'shared']}
        style_comparisons[metric] = compare(totals['isolated'], totals['shared'])
    end_stats = [record for capture in captures for record in capture['profile']['cache_records']
                 if record['section'] == len(spine) - 1]
    stable_stats = [{key: value for key, value in row.items() if key not in ['sample', 'section']}
                    for row in end_stats]
    assert all(row == stable_stats[0] for row in stable_stats)
    result = {
        'measured_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'book': 'A Concise History of Switzerland', 'spine': spine,
        'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'reference_sha256': hashlib.sha256(args.reference_binary.read_bytes()).hexdigest()
        if args.reference_binary else None,
        'method': {
            'cpu': args.cpu, 'rounds': args.rounds, 'samples_per_round': args.samples,
            'warmups_per_process': 3, 'cache_reset_per_traversal': True,
            'ordering': 'Spine order; alternate isolated/shared traversal order every iteration.',
            'settings': '600px, 16px, BookOptimized, original language/hyphenation, deterministic mock font.',
            'timing': 'Plain mode: no detailed preparation timers. Profile mode: separate CSS timers. Both use the same binary and fresh chapter factories/shapers.',
            'limits': 'Warm OS caches and initialized process-wide default CSS AST. Includes the first compilation of each distinct book style program. Book timing is cumulative timed chapter loads; excludes gaps, fingerprints, document destruction, cache setup/destruction, EPUB extraction, native fonts/images, UI and first paint. Style totals sum per-section medians; nested style timers are not additive.',
            'cache_scope': 'One BookStylesheetCache per immutable book traversal, explicitly attached to each chapter factory. The bytes counter measures cached stylesheet source text, excluding parsed/compiled structures.',
        },
        'opening_comparisons': comparisons, 'style_comparisons': style_comparisons,
        'cache_stats_per_traversal': stable_stats[0],
        'validation': {'sections': len(spine), 'all_outputs_identical': True,
                       'plain_and_profile_match': True, 'cache_stats_stable': True,
                       'independent_reference_sections': len(independent)},
        'captures': captures,
    }
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    lines = [result['book'] + ' — book-shared CSS cache', '',
             f'{args.rounds} rounds x {args.samples} samples; 3 warmups/process; CPU {args.cpu}; {len(spine)} sections.',
             'Each traversal starts with an empty book cache and fresh chapter factories.',
             '', 'OPENING: cumulative timed chapter loads, median ms',
             f'{"Metric":26s} {"Isolated":>10s} {"Shared":>10s} {"Change":>10s}']
    for name, row in comparisons.items():
        lines.append(f'{name:26s} {row["isolated_ms"]:10.3f} {row["shared_ms"]:10.3f} {row["change_percent"]:9.2f}%')
    lines += ['', 'CSS PROFILE: sum of section medians, ms; nested timers overlap']
    for name, row in style_comparisons.items():
        lines.append(f'{name:26s} {row["isolated_ms"]:10.3f} {row["shared_ms"]:10.3f} {row["change_percent"]:9.2f}%'
                     if row['isolated_ms'] else f'{name:26s}      0.000      0.000')
    lines += ['', 'CACHE STATISTICS PER TRAVERSAL', json.dumps(stable_stats[0], indent=2),
              '', 'VALIDATION', json.dumps(result['validation'], indent=2),
              '', 'LIMITATIONS', result['method']['limits'], result['method']['cache_scope'],
              '', 'REPRODUCTION',
              'python3 tools/pipeline-bench/book_css.py <pipeline-bench-binary> <extracted-book-directory> --reference-binary <baseline-profiling-binary> --output tools/pipeline-bench/results/book-shared-css.json',
              '', 'APPLICATION',
              'Create one html::pipeline::BookStylesheetCache when the book opens.',
              'Call factory.set_book_stylesheet_cache(cache.clone()) for every chapter factory.',
              'Drop it when the book closes; clear or replace it when stylesheet resources change.']
    args.output.with_suffix('.txt').write_text('\n'.join(lines) + '\n')
    print('\n'.join(lines))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('book', type=Path)
    parser.add_argument('--reference-binary', type=Path)
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--samples', type=int, default=9)
    parser.add_argument('--cpu', type=int, default=8)
    parser.add_argument('--output', type=Path, required=True)
    arguments = parser.parse_args()
    assert arguments.rounds > 0 and arguments.samples > 0
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    collect(arguments)
