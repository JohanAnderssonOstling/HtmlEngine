#!/usr/bin/env python3
"""Alternating, CPU-pinned comparisons of isolated hot-access changes."""
import argparse
import hashlib
import json
import statistics
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--before', required=True, help='Binary prefix with -time and -heap suffixes')
parser.add_argument('--after', required=True)
parser.add_argument('--fixture', required=True)
parser.add_argument('--case', required=True)
parser.add_argument('--finding', required=True)
parser.add_argument('--cpu', default='8')
parser.add_argument('--samples', type=int, default=21)
parser.add_argument('--rounds', type=int, default=3)
parser.add_argument('--output', required=True)
args = parser.parse_args()
rows = {'time': {'before': [], 'after': []}, 'heap': {}}
integrity_keys = ['integrity', 'render_fingerprint', 'source_fingerprint', 'text_color_fingerprint', 'glyphs', 'lines']
def run(variant, kind, count):
    binary = getattr(args, variant) + '-' + kind
    command = ['taskset', '-c', args.cpu, binary, args.fixture, args.case, str(count)]
    result = subprocess.run(command, text=True, capture_output=True, check=True)
    return json.loads(result.stdout)
for round_index in range(args.rounds):
    for variant in (['before', 'after'] if round_index % 2 == 0 else ['after', 'before']):
        rows['time'][variant].append(run(variant, 'time', args.samples))
for variant in ['before', 'after']:
    rows['heap'][variant] = run(variant, 'heap', 5)
reference = rows['time']['before'][0]
for result in rows['time']['before'] + rows['time']['after'] + list(rows['heap'].values()):
    for key in integrity_keys:
        if key in reference:
            assert result[key] == reference[key], (key, reference[key], result[key])
summary = {}
for key in ['cpu_median_ms', 'wall_median_ms', 'user_instructions']:
    if all(row.get(key) is not None for runs in rows['time'].values() for row in runs):
        before = statistics.median(row[key] for row in rows['time']['before'])
        after = statistics.median(row[key] for row in rows['time']['after'])
        paired = statistics.median(a[key] / b[key] - 1 for b, a in zip(rows['time']['before'], rows['time']['after'])) * 100
        summary[key] = {'before': before, 'after': after, 'paired_change_percent': paired}
for key in ['allocations', 'allocated_bytes', 'allocation_requests', 'allocations_median', 'allocated_bytes_median']:
    if key in rows['heap']['before']:
        before, after = [rows['heap'][variant][key] for variant in ['before', 'after']]
        summary[key] = {'before': before, 'after': after, 'change': after - before}
hashes = {variant + '-' + kind: hashlib.sha256(Path(getattr(args, variant) + '-' + kind).read_bytes()).hexdigest()
          for variant in ['before', 'after'] for kind in ['time', 'heap']}
output = {'finding': args.finding, 'fixture': args.fixture, 'fixture_sha256': hashlib.sha256(Path(args.fixture).read_bytes()).hexdigest(),
          'case': args.case, 'cpu': args.cpu, 'samples_per_round': args.samples, 'rounds': args.rounds,
          'integrity_matches': True, 'binary_sha256': hashes, 'summary': summary, 'runs': rows}
Path(args.output).write_text(json.dumps(output, indent=2) + '\n')
print(json.dumps({'finding': args.finding, 'case': args.case, 'summary': summary}))
