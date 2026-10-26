#!/usr/bin/env python3
"""Compare frozen pipeline binaries, including bounded floating-point geometry changes."""

import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess


def compare_geometry(before, after):
    result = {'max_absolute_delta_px': 0.0, 'changed_numbers': 0,
              'max_delta_by_field': {}, 'examples': []}

    def compare(a, b, path):
        if isinstance(a, dict):
            assert isinstance(b, dict) and a.keys() == b.keys(), path
            for key in a:
                compare(a[key], b[key], path + [key])
        elif isinstance(a, list):
            assert isinstance(b, list) and len(a) == len(b), (path, len(a), len(b))
            for index, (left, right) in enumerate(zip(a, b)):
                compare(left, right, path + [index])
        elif type(a) is float:
            assert type(b) in (float, int) and math.isfinite(a) and math.isfinite(b), path
            delta = abs(a - b)
            field = '/'.join(str(item) for item in path if isinstance(item, str))
            # Positions can accumulate tiny per-line rounding over a long chapter.
            # Local line dimensions, glyph offsets and advances have a tighter bound.
            local = ('offsets' in path or 'advances' in path
                     or (len(path) == 4 and path[0] == 'lines'
                         and path[2] == 'geometry' and path[3] >= 2))
            assert delta <= (1e-5 if local else 0.01), (path, a, b, delta)
            result['max_absolute_delta_px'] = max(result['max_absolute_delta_px'], delta)
            result['max_delta_by_field'][field] = max(
                result['max_delta_by_field'].get(field, 0.0), delta)
            if delta:
                result['changed_numbers'] += 1
                if len(result['examples']) < 8:
                    result['examples'].append({'path': path, 'before': a, 'after': b, 'delta': delta})
        else:
            assert a == b, (path, a, b)

    compare(before, after, [])
    return result


def collect(args):
    os.sched_setaffinity(0, {args.cpu})
    binaries = {'before': args.before.resolve(), 'after': args.after.resolve()}
    groups = {'book': json.loads(args.spine.read_text())}
    for fixture_list in args.fixtures:
        groups[fixture_list.stem] = json.loads(fixture_list.read_text())
    report = {
        'measured_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'method': {'case': args.case, 'samples': args.samples, 'warmups': 3,
                   'cpu': args.cpu, 'font_backend': 'deterministic mock',
                   'geometry_limits_px': {'local': 1e-5, 'positions': 0.01},
                   'limits': 'Warm OS caches. Fresh session/shaper for cold_book; retained session for resize_book. Storage and precision validation; no controlled latency claim. Geometry snapshot runs after timed work.'},
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
                row[name] = json.loads(subprocess.check_output([
                    str(binaries[name]), path, args.case, str(args.samples), 'default', 'geometry'],
                    text=True))
                geometry[name] = row[name].pop('geometry')
            for field in ['source_fingerprint', 'text_color_fingerprint', 'glyphs', 'lines']:
                assert row['before'][field] == row['after'][field], (path, field)
            row['identical_render_fingerprint'] = (
                row['before']['render_fingerprint'] == row['after']['render_fingerprint'])
            row['geometry_comparison'] = compare_geometry(geometry['before'], geometry['after'])
            row['geometry_sha256'] = {name: hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()
                                      for name, value in geometry.items()}
            rows.append(row)
        report[group] = rows
        totals = {}
        for name in binaries:
            def memory(suffix, field):
                return sum(entry[field] for row in rows for entry in row[name]['memory_breakdown']
                           if entry['label'].endswith(suffix))
            totals[name] = {
                'metrics_count': memory('inline_plans.run_metrics', 'count'),
                'metrics_bytes': memory('inline_plans.run_metrics', 'bytes'),
                'tokens_count': memory('inline_plans.tokens', 'count'),
                'tokens_bytes': memory('inline_plans.tokens', 'bytes'),
                'user_instructions': sum(row[name]['user_instructions'] for row in rows),
            }
        report[group + '_totals'] = totals
        args.output.write_text(json.dumps(report, indent=2) + '\n')
        print(group, len(rows), 'exact:', sum(row['identical_render_fingerprint'] for row in rows),
              'max delta px:', max(row['geometry_comparison']['max_absolute_delta_px'] for row in rows),
              totals, flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    parser.add_argument('spine', type=Path)
    parser.add_argument('--fixtures', type=Path, action='append', default=[])
    parser.add_argument('--source-changes', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cpu', type=int, default=8)
    parser.add_argument('--case', choices=['cold_book', 'resize_book'], default='cold_book')
    parser.add_argument('--samples', type=int, default=3)
    collect(parser.parse_args())
