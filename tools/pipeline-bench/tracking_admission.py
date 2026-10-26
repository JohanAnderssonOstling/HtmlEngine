#!/usr/bin/env python3
"""Compare frozen solver binaries, outputs, counters and opening times."""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import xml.etree.ElementTree as ET


PHASES = ['lines.justified.normal', 'lines.justified.emergency']


def run(binary, book, href, case, count):
    return json.loads(subprocess.check_output(
        [str(binary), str(book / href), case, str(count), 'default'], text=True))


def identity(row):
    return {key: row[key] for key in ['render_fingerprint', 'glyphs', 'lines']}


def canonical(rows):
    return [{key: value for key, value in row.items() if key != 'wall_ns'}
            for row in rows]


def collect(args):
    os.sched_setaffinity(0, {args.cpu})
    binaries = {'before': args.before.resolve(), 'after': args.after.resolve()}
    book = args.book.resolve()
    ns = {'opf': 'http://www.idpf.org/2007/opf'}
    package = ET.parse(book / 'content.opf').getroot()
    manifest = {item.attrib['id']: item.attrib['href']
                for item in package.findall('opf:manifest/opf:item', ns)}
    spine = [manifest[item.attrib['idref']]
             for item in package.findall('opf:spine/opf:itemref', ns)]
    result = {
        'measured_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'book': 'A Concise History of Switzerland',
        'binary_sha256': {key: hashlib.sha256(path.read_bytes()).hexdigest()
                          for key, path in binaries.items()},
        'method': {
            'cpu': args.cpu, 'rounds': args.rounds,
            'samples_per_round': args.samples, 'warmups_per_process': 3,
            'counter_samples': 3,
            'timings': 'open_deep_wall_book: hot-loop counters and experimental probes disabled; process CPU and inclusive wall scopes include profiling overhead.',
            'counters': 'open_solver_book: opt-in diagnostic capture; timings from this case are not used.',
            'ordering': 'Alternate before/after process order per round and section.',
            'settings': '600px; 16px; BookOptimized; linked publication CSS; deterministic mock shaper; original chapters, no language or hyphenation changes.',
            'limits': 'Warm OS caches; fresh document factory/session/shaper. Excludes EPUB extraction, native font loading/shaping, image decode, UI and first paint. Aggregates sum independent per-section medians and are not exact whole-book elapsed times.',
        },
        'spine': [],
    }
    metric_names = ['opening_cpu', 'opening_wall', 'normal_wall', 'emergency_wall', 'solver_wall']
    for section, href in enumerate(spine):
        variants = {}
        for name, binary in binaries.items():
            detailed = run(binary, book, href, 'open_solver_book', 3)
            measurements = {}
            for phase in PHASES:
                samples = detailed['measurements'].get(phase, [[], [], []])
                rows = canonical(samples[0])
                assert all(canonical(sample) == rows for sample in samples)
                measurements[phase] = rows
            variants[name] = {**identity(detailed), 'measurements': measurements,
                              'samples_ms': {metric: [] for metric in metric_names}}
        assert variants['before']['glyphs'] == variants['after']['glyphs'], href
        if args.require_identical_output:
            for key in ['render_fingerprint', 'glyphs', 'lines', 'measurements']:
                assert variants['before'][key] == variants['after'][key], (href, key)
        for iteration in range(args.rounds):
            order = list(binaries)
            if (iteration + section) % 2:
                order.reverse()
            for name in order:
                output = run(binaries[name], book, href, 'open_deep_wall_book', args.samples)
                assert not output['measurements']
                assert identity(output) == {key: variants[name][key] for key in identity(output)}, (href, name)
                values = variants[name]['samples_ms']
                values['opening_cpu'].extend(output['timings']['total_cpu']['samples_ms'])
                values['opening_wall'].extend(output['timings']['total_wall']['samples_ms'])
                for key, phase in [('normal_wall', PHASES[0]), ('emergency_wall', PHASES[1]),
                                   ('solver_wall', 'lines.justified_solver')]:
                    metric = output['scopes'].get(phase)
                    values[key].extend(metric['inclusive_wall']['samples_ms'] if metric else [0.0] * args.samples)
        for variant in variants.values():
            variant['median_ms'] = {key: statistics.median(values)
                                    for key, values in variant['samples_ms'].items()}
        result['spine'].append({'href': href, **variants,
                               'identical_output': variants['before']['render_fingerprint'] == variants['after']['render_fingerprint']})
        print(href, 'same output', result['spine'][-1]['identical_output'], flush=True)
    aggregate = {}
    for name in binaries:
        counts = {}
        for phase in PHASES:
            rows = [item for section in result['spine']
                    for item in section[name]['measurements'][phase]]
            keys = rows[0].keys() if rows else []
            counts[phase] = {
                key: (max(row[key] for row in rows) if key.startswith('max_') or '_max_' in key
                      else sum(row[key] for row in rows)) for key in keys}
            counts[phase].update(passes=len(rows), failed=sum(not row['success'] for row in rows))
        aggregate[name] = {
            'sum_section_medians_ms': {key: sum(row[name]['median_ms'][key] for row in result['spine'])
                                       for key in metric_names},
            'counts': counts,
        }
    result['aggregate'] = aggregate
    result['validation'] = {
        'sections': len(spine),
        'identical_output_sections': sum(row['identical_output'] for row in result['spine']),
        'glyph_counts_unchanged': True,
        'counters_stable_across_samples': True,
        'timing_and_detailed_outputs_match': True,
        'identical_output_and_counters_required': args.require_identical_output,
    }
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(aggregate, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('book', type=Path)
    parser.add_argument('--require-identical-output', action='store_true')
    parser.add_argument('--cpu', type=int, default=8)
    parser.add_argument('--samples', type=int, default=9)
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--output', type=Path, default=Path(__file__).parent /
                        'results/tracking-admission.json')
    args = parser.parse_args()
    if args.samples < 1 or args.rounds < 1:
        parser.error('sample and round counts must be positive')
    collect(args)


if __name__ == '__main__':
    main()
