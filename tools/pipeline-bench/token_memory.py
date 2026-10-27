#!/usr/bin/env python3
"""Measure opening-only AMD Rembrandt L2 data-request counters."""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess


def collect(args):
    os.sched_setaffinity(0, {args.cpu})
    binary = args.binary.resolve()
    paths = json.loads(args.spine.read_text())
    reference = json.loads(args.reference.read_text())
    expected = reference['captures'][0]['after']['book']['identities']
    captures = []
    for index in range(args.rounds):
        capture = json.loads(subprocess.check_output(
            [str(binary), str(args.spine.resolve()), 'book_cache', str(args.samples)], text=True))
        assert capture['identities'] == expected
        assert capture['paths'] == paths
        assert len(capture['counter_records']) == args.samples * len(paths) * 2
        for record in capture['counter_records']:
            values = record['values']
            assert values['time_enabled_ns'] > 0
            assert values['time_running_ns'] == values['time_enabled_ns'], 'PMU group was multiplexed'
        captures.append(capture)
        print(f'Round {index + 1}: all outputs identical; counters ran 100% of each opening interval', flush=True)
    variants = {}
    for variant in ['isolated', 'shared']:
        rows = [row['values'] for capture in captures for row in capture['counter_records']
                if row['variant'] == variant]
        totals = {key: sum(row[key] for row in rows) for key in rows[0]}
        ratios = []
        for capture in captures:
            for sample in range(args.samples):
                subset = [row['values'] for row in capture['counter_records']
                          if row['variant'] == variant and row['sample'] == sample]
                hits = sum(row['l2_data_hits'] for row in subset)
                misses = sum(row['l2_data_misses'] for row in subset)
                ratios.append(100 * hits / (hits + misses))
        variants[variant] = {
            'totals': totals,
            'pooled_l2_data_request_hit_percent': 100 * totals['l2_data_hits'] /
                (totals['l2_data_hits'] + totals['l2_data_misses']),
            'median_book_hit_percent': statistics.median(ratios),
            'min_book_hit_percent': min(ratios), 'max_book_hit_percent': max(ratios),
            'book_hit_percent_samples': ratios,
        }
    memory = []
    for section in reference['sessions']:
        labels = section['after']['memory_breakdown']
        records = next(row for row in labels if row['label'].endswith('glyph_metrics.GlyphMetrics.records'))
        singles = next((row for row in labels if row['label'].endswith('glyph_metrics.GlyphMetrics.singles')), None)
        memory.append({'path': section['path'], 'text_positions': section['after']['glyphs'],
                       'metric_records': records, 'single_metrics': singles})
    result = {
        'measured_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'method': {'cpu': args.cpu, 'rounds': args.rounds, 'samples_per_round': args.samples,
                   'sections': len(paths), 'warmups': 3,
                   'scope': 'Counters enabled immediately before source read/parse/preparation/shaping/layout, disabled immediately after CPU/wall timing. Fingerprints, destruction and report generation excluded. Per-thread user mode; warm OS caches; normal release flags without forced frame pointers; deterministic mock fonts.',
                   'limits': 'These are whole-opening L2 data-request hit rates, conditional on reaching L2, not glyph-stage, L1, L3 or DRAM rates. Data-request events are not counts of retired application loads. They do not measure memory-wait milliseconds.',
                   'multiplexing': 'Rejected unless running time equals enabled time in every opening interval.'},
        'events': {'l2_data_hits': 'cpu/event=0x64,umask=0xf0/',
                   'l2_data_misses': 'cpu/event=0x64,umask=0x08/',
                   'load_dispatches': 'cpu/event=0x29,umask=0x01/',
                   'store_dispatches': 'cpu/event=0x29,umask=0x02/'},
        'variants': variants, 'captures': captures, 'metric_storage': memory,
        'unavailable': 'AMD IBS memory-latency sampling requires CPU-wide recording here. Per-thread IBS was rejected; CPU-wide IBS was denied under perf_event_paranoid=1. No system permissions were changed.',
        'validation': {'identical_sections': len(paths), 'counter_running_percent': 100,
                       'reference': str(args.reference)},
    }
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    for variant, row in variants.items():
        print(variant, 'L2 data-request hits', row['pooled_l2_data_request_hit_percent'],
              'range', row['min_book_hit_percent'], row['max_book_hit_percent'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('spine', type=Path)
    parser.add_argument('--reference', type=Path, default=Path('tools/pipeline-bench/results/text-token-stages.json'))
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--samples', type=int, default=7)
    parser.add_argument('--cpu', type=int, default=8)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    assert args.rounds > 0 and args.samples > 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    collect(args)
