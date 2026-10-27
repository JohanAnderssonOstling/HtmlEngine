#!/usr/bin/env python3
"""Probe justified fallback causes without using experimental line breaks.

Build pipeline-bench with the profiling feature, then pass its binary and an
extracted EPUB directory containing content.opf. Timings include probe work;
this script measures feasibility and counts, not performance improvements.
"""

import argparse
import datetime
import hashlib
import json
import os
import re
import shutil
import statistics
from pathlib import Path
import platform
import subprocess
import tempfile
import xml.etree.ElementTree as ET


PREFIX = 'lines.justified.'
PROBES = ['tracking', 'hyphens', 'tracking_hyphens', 'underfull', 'tight',
          'spacing', 'underfull_hyphens', 'tight_hyphens']
IDENTITY = ['render_fingerprint', 'glyphs', 'lines']
SELECTED = ['selected_lines', 'selected_loose', 'selected_tight',
            'selected_excessive_hyphen_run', 'selected_tracking_fits',
            'selected_requires_ragged', 'selected_ragged_total_millipx',
            'selected_ragged_max_millipx', 'selected_ragged_over_1px',
            'selected_ragged_over_3px', 'selected_ragged_over_10px',
            'selected_tracking_fit_total_millipx', 'selected_tracking_fit_max_millipx']


def canonical(rows):
    return [{k: v for k, v in row.items() if k != 'wall_ns'} for row in rows]


def run(binary, book, href, case, count):
    return json.loads(subprocess.check_output(
        [str(binary), str(book / href), case, str(count), 'default'], text=True))




def compare_timing(binary, book, modified, spine, samples, rounds):
    metrics = {
        'opening_wall': ('timings', 'total_wall'),
        'opening_cpu': ('timings', 'total_cpu'),
        'normal_solver_wall': ('scopes', PREFIX + 'normal'),
        'emergency_wall': ('scopes', PREFIX + 'emergency'),
        'hyphenation_wall': ('scopes', 'tokens.hyphenation'),
    }
    rows = []
    for href in spine:
        series = {variant: {key: [] for key in metrics}
                  for variant in ['baseline', 'english_auto']}
        for iteration in range(rounds):
            order = [('baseline', book), ('english_auto', modified)]
            if iteration % 2:
                order.reverse()
            for variant, directory in order:
                output = run(binary, directory, href, 'open_deep_wall_book', samples)
                assert not output['measurements']
                for key, (kind, name) in metrics.items():
                    metric = output[kind].get(name)
                    if kind == 'scopes':
                        metric = metric['inclusive_wall'] if metric else None
                    values = metric['samples_ms'] if metric else [0.0] * samples
                    series[variant][key].extend(values)
        rows.append({'href': href, 'samples_ms': series,
                     'median_ms': {variant: {key: statistics.median(values)
                                            for key, values in by_metric.items()}
                                   for variant, by_metric in series.items()}})
    return {
        'samples_per_round': samples, 'rounds': rounds,
        'method': 'Alternating baseline/English-auto processes per section and round; 3 warmups/process. open_deep_wall_book disables hot-loop counters and experimental probes. Sums of independent per-section medians are a comparison statistic, not an exact whole-book duration.',
        'sum_section_medians_ms': {
            variant: {key: sum(row['median_ms'][variant][key] for row in rows)
                      for key in metrics}
            for variant in ['baseline', 'english_auto']},
        'spine': rows,
    }


def hyphenation_experiment(binary, book, spine, baseline, timing_samples, timing_rounds):
    """Change only temporary copies; these variants intentionally change output."""
    variants = {
        'language_only': 'lang="en"',
        'auto_only': 'style="hyphens: auto"',
        'english_auto': 'lang="en" style="hyphens: auto"',
    }
    result = {'variants': {}, 'notes':
              'Source prose chapters have no lang/xml:lang or hyphens declaration. '
              'The OPF declares English, but standalone chapter loads do not '
              'consume OPF metadata. Initial hyphens is manual. '
              'These are feasibility experiments, not performance comparisons; '
              'automatic hyphenation changes line breaks and adds token work.'}
    base = {row['href']: row for row in baseline}
    with tempfile.TemporaryDirectory(prefix='htmlengine-hyphen-causes-') as temp:
        for name, attributes in variants.items():
            copy = Path(temp) / name
            shutil.copytree(book, copy)
            rows = []
            for href in spine:
                path = copy / href
                source = path.read_text()
                root = re.search(r'<html(?=[\s>])[^>]*>', source)
                assert root and not re.search(r'(?<!:)\blang\s*=|\bstyle\s*=', root.group()), href
                source = source[:root.start()] + root.group().replace(
                    '<html', '<html ' + attributes, 1) + source[root.end():]
                path.write_text(source)
                output = run(binary, copy, href, 'open_solver_book', 1)
                measurements = output['measurements']
                normal = measurements.get(PREFIX + 'normal', [[]])[0]
                emergency = measurements.get(PREFIX + 'emergency', [[]])[0]
                same = all(output[key] == base[href][key] for key in IDENTITY)
                if name != 'english_auto':
                    assert same, (name, href, 'unexpected output change')
                    assert len(normal) == base[href]['normal_requests']
                    assert len(emergency) == base[href]['failures']
                rows.append({
                    'href': href, 'normal_requests': len(normal),
                    'failures': len(emergency),
                    'same_output_as_baseline': same,
                    'candidate_evaluations': {
                        'normal': sum(row['candidate_evaluations'] for row in normal),
                        'emergency': sum(row['candidate_evaluations'] for row in emergency),
                    },
                    'rescued_by': {
                        probe: sum(row['success'] for row in measurements.get(PREFIX + 'probe.' + probe, [[]])[0])
                        for probe in PROBES},
                    **{key: output[key] for key in IDENTITY},
                    'emergency_selected': {
                        key: (max((row[key] for row in emergency), default=0)
                              if '_max_' in key else sum(row[key] for row in emergency))
                        for key in SELECTED},
                })
            result['variants'][name] = {
                'normal_requests': sum(row['normal_requests'] for row in rows),
                'failures': sum(row['failures'] for row in rows),
                'sections_changed': sum(not row['same_output_as_baseline'] for row in rows),
                'candidate_evaluations': {
                    phase: sum(row['candidate_evaluations'][phase] for row in rows)
                    for phase in ['normal', 'emergency']},
                'rescued_by': {probe: sum(row['rescued_by'][probe] for row in rows)
                               for probe in PROBES},
                'spine': rows,
            }
            print('Hyphenation', name, result['variants'][name]['failures'], flush=True)
            if name == 'english_auto' and timing_samples:
                result['timing'] = compare_timing(binary, book, copy, spine, timing_samples, timing_rounds)
                print('Timing', result['timing']['sum_section_medians_ms'], flush=True)
    return result


def collect(args):
    if args.cpu is not None:
        os.sched_setaffinity(0, {args.cpu})
    binary = args.binary.resolve()
    book = args.book.resolve()
    ns = {'opf': 'http://www.idpf.org/2007/opf'}
    package = ET.parse(book / 'content.opf').getroot()
    manifest = {item.attrib['id']: item.attrib['href']
                for item in package.findall('opf:manifest/opf:item', ns)}
    spine = [manifest[item.attrib['idref']]
             for item in package.findall('opf:spine/opf:itemref', ns)]
    baseline = json.loads(args.baseline.read_text()) if args.baseline else None
    previous = {row['href']: row for row in baseline['spine']} if baseline else {}
    source = dict(baseline['source_sha256']) if baseline else {}
    if args.source_root:
        key = 'crates/04-html-layout/src/layout/inline/wrapping/knuth_plass.rs'
        source[key] = hashlib.sha256((args.source_root / key).read_bytes()).hexdigest()
    result = {
        'measured_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'book': 'A Concise History of Switzerland',
        'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'source_sha256': source,
        'platform': platform.platform(),
        'method': {
            'cpu': args.cpu, 'samples_per_section': args.samples,
            'warmups_per_process': 3,
            'settings': '600px; 16px; BookOptimized; publication CSS; deterministic mock shaper',
            'probes': 'Only failed strict requests are retried under isolated relaxations. Probe choices are discarded; the original emergency pass runs last.',
            'tracking': 'Admit lines that fit within the existing 0.5% tracking capacity, in either direction; retain the hyphen-run limit.',
            'hyphens': 'Relax only the two-consecutive-hyphens limit.',
            'underfull': 'Admit loose lines with existing penalties; retain tight and hyphen constraints.',
            'tight': 'Admit extra compression within the existing tracking capacity; retain loose and hyphen constraints.',
            'spacing': 'Underfull and tight together, retaining the hyphen limit.',
            'limits': 'Probes measure solver feasibility, not an output-quality approval or a performance gain. Warm OS caches; mock metrics; no native fonts, image decode, UI or first paint.',
        },
        'spine': [],
    }
    failures = []
    for href in spine:
        detailed = run(binary, book, href, 'open_solver_book', args.samples)
        ordinary = run(binary, book, href, 'open_deep_wall_book', 1)
        for key in IDENTITY:
            assert detailed[key] == ordinary[key], (href, key)
            if href in previous:
                assert detailed[key] == previous[href][key], (href, key, 'prior capture')
        rows = {}
        for name, samples in detailed['measurements'].items():
            assert len(samples) == args.samples, (href, name)
            first = canonical(samples[0])
            assert all(canonical(sample) == first for sample in samples)
            rows[name.removeprefix(PREFIX)] = first
        normal = rows.get('normal', [])
        failed = [(i, row) for i, row in enumerate(normal) if not row['success']]
        emergency = rows.get('emergency', [])
        assert len(failed) == len(emergency)
        for probe in PROBES:
            assert len(rows.get('probe.' + probe, [])) == len(failed)
        if href in previous:
            old = previous[href]['measurements'].get(PREFIX + 'normal', [[]])[0]
            assert len(old) == len(normal)
            for before, after in zip(old, normal):
                assert all(after[key] == value for key, value in before.items()
                           if key != 'wall_ns'), (href, 'strict behavior changed')
        for i, (request, strict) in enumerate(failed):
            assert emergency[i]['success']
            failures.append({
                'href': href, 'request': request,
                'first_line_failed': strict['scored_candidates'] == 0,
                'rescued_by': [probe for probe in PROBES
                               if rows['probe.' + probe][i]['success']],
                'emergency_selected': {key: emergency[i][key] for key in SELECTED},
            })
        result['spine'].append({
            'href': href, **{key: detailed[key] for key in IDENTITY},
            'normal_requests': len(normal), 'failures': len(failed),
            'measurements': rows,
        })
        print(href, len(failed), '/', len(normal), flush=True)
    total = sum(row['normal_requests'] for row in result['spine'])
    result['failures'] = failures
    result['aggregate'] = {
        'normal_requests': total,
        'failed_requests': len(failures),
        'first_line_failed': sum(row['first_line_failed'] for row in failures),
        'rescued_by': {probe: sum(probe in row['rescued_by'] for row in failures)
                       for probe in PROBES},
        'emergency_selected': {
            key: (max((row['emergency_selected'][key] for row in failures), default=0)
                  if '_max_' in key else
                  sum(row['emergency_selected'][key] for row in failures))
            for key in SELECTED},
        'remaining_after_tracking': sum('tracking' not in row['rescued_by'] for row in failures),
        'sections_with_failures': sum(row['failures'] > 0 for row in result['spine']),
    }
    result['validation'] = {
        'matching_sections': len(spine),
        'matches_prior_capture': bool(previous),
        'strict_counters_match_prior_capture': bool(previous),
        'counters_stable_across_samples': True,
    }
    if args.hyphenation_experiment:
        result['hyphenation_experiment'] = hyphenation_experiment(binary, book, spine, result['spine'], args.timing_samples, args.timing_rounds)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result['aggregate'], indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('book', type=Path)
    parser.add_argument('--output', type=Path, default=Path(__file__).parent /
                        'results/justified-solver-fallback-causes.json')
    parser.add_argument('--baseline', type=Path,
                        help='Previous justified-solver-investigation.json for output/counter checks')
    parser.add_argument('--source-root', type=Path,
                        help='Frozen source directory matching this binary')
    parser.add_argument('--samples', type=int, default=3)
    parser.add_argument('--cpu', type=int)
    parser.add_argument('--hyphenation-experiment', action='store_true')
    parser.add_argument('--timing-samples', type=int, default=0,
                        help='Optional samples per timing round for baseline versus English-auto')
    parser.add_argument('--timing-rounds', type=int, default=2)
    args = parser.parse_args()
    if args.samples < 1 or args.timing_rounds < 1 or args.timing_samples < 0:
        parser.error('sample/round counts must be positive; timing samples may be zero')
    if args.timing_samples and not args.hyphenation_experiment:
        parser.error('--timing-samples requires --hyphenation-experiment')
    collect(args)


if __name__ == '__main__':
    main()
