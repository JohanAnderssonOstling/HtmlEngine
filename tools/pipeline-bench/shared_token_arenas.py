#!/usr/bin/env python3
"""Check exact layout and count opening allocations for frozen arena binaries."""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess

from precision_compare import compare_geometry


def collect(args):
    os.sched_setaffinity(0, {args.cpu})
    binaries = {name: path.resolve() for name, path in
                [('before', args.before), ('after', args.after)]}
    groups = {'book': json.loads(args.spine.read_text())}
    groups.update({path.stem: json.loads(path.read_text()) for path in args.fixtures})
    report = {
        'measured_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'method': {
            'case': args.case, 'samples': args.samples, 'warmups': 3, 'cpu': args.cpu,
            'font_backend': 'deterministic mock',
            'limits': 'Fresh session/shaper for cold_book; warm OS caches. Heap builds '
                      'count alloc/realloc requests, including arena growth. Section totals '
                      'sum independent snapshots, not simultaneous residency. Instrumented '
                      'CPU times do not establish uninstrumented latency.',
        },
        'binary_sha256': {name: hashlib.sha256(path.read_bytes()).hexdigest()
                          for name, path in binaries.items()},
        'source_changes': json.loads(args.source_changes.read_text()),
    }
    for group, paths in groups.items():
        rows = []
        for index, path in enumerate(paths):
            row = {'path': path}
            if group != 'book':
                row['source'] = Path(path).read_text()
            geometry = {}
            for name in list(binaries)[::1 if index % 2 == 0 else -1]:
                result = json.loads(subprocess.check_output([
                    str(binaries[name]), path, args.case, str(args.samples), 'default', 'geometry'],
                    text=True))
                assert result['heap_instrumented'], 'build both binaries with --features heap'
                geometry[name] = result.pop('geometry')
                row[name] = result
            for field in ['source_fingerprint', 'text_color_fingerprint', 'render_fingerprint',
                          'glyphs', 'lines']:
                assert row['before'][field] == row['after'][field], (path, field)
            row['geometry_comparison'] = compare_geometry(geometry['before'], geometry['after'])
            assert row['geometry_comparison']['changed_numbers'] == 0, path
            rows.append(row)
        report[group] = rows
        totals = {}
        for name in binaries:
            def memory(suffix, field):
                return sum(entry[field] for row in rows for entry in row[name]['memory_breakdown']
                           if entry['label'].endswith('inline_plans.' + suffix))
            totals[name] = {
                field: sum(row[name][field] for row in rows)
                for field in ['allocation_requests', 'allocated_bytes', 'heap_live_bytes',
                              'heap_peak_bytes', 'reported_document_bytes', 'cpu_median_ms']
            }
            totals[name].update({
                'retained_plans': memory('entries', 'count'),
                'tokens_count': memory('tokens', 'count'),
                'tokens_bytes': memory('tokens', 'bytes'),
                'metrics_count': memory('run_metrics', 'count'),
                'inline_plan_bytes': sum(entry['bytes'] for row in rows
                                         for entry in row[name]['memory_breakdown']
                                         if 'inline_plans.' in entry['label']),
            })
        report[group + '_totals'] = totals
        args.output.write_text(json.dumps(report, indent=2) + '\n')
        print(group, len(rows), 'exact layouts;', totals, flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('spine', type=Path)
    parser.add_argument('--case', choices=['cold_book', 'resize_book'], default='cold_book')
    parser.add_argument('--fixtures', type=Path, action='append', default=[])
    parser.add_argument('--source-changes', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cpu', type=int, default=8)
    parser.add_argument('--samples', type=int, default=3)
    collect(parser.parse_args())
