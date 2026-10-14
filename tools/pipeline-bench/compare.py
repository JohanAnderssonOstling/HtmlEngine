#!/usr/bin/env python3
"""Compare identical real chapters using separate time and heap builds."""
import argparse
import json
import os
import statistics
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--before-time', required=True)
parser.add_argument('--after-time', required=True)
parser.add_argument('--before-heap', required=True)
parser.add_argument('--after-heap', required=True)
parser.add_argument('--chapters', required=True)
parser.add_argument('--output', required=True)
parser.add_argument('--cpu', type=int, required=True)
parser.add_argument('--samples', type=int, default=11)
parser.add_argument('--rounds', type=int, default=3)
args = parser.parse_args()
chapters = sorted(Path(args.chapters).glob('c000[123].html'))
assert len(chapters) == 3
results = []

def run(binary, chapter, case, samples):
    command = ['taskset', '-c', str(args.cpu), binary, str(chapter.resolve()), case, str(samples)]
    process = subprocess.run(command, capture_output=True, text=True)
    if process.returncode:
        return {'error': process.stderr.strip(), 'returncode': process.returncode}
    return json.loads(process.stdout)

for chapter in chapters:
    for case in ['cold', 'resize', 'paint', 'font', 'handoff']:
        row = {'chapter': chapter.name, 'case': case, 'time': {}, 'heap': {}}
        samples = {'before': [], 'after': []}
        for round_index in range(args.rounds):
            order = ['before', 'after'] if round_index % 2 == 0 else ['after', 'before']
            for variant in order:
                if samples[variant] and 'error' in samples[variant][-1]:
                    continue
                result = run(getattr(args, variant + '_time'), chapter, case, args.samples)
                samples[variant].append(result)
        for variant in ['before', 'after']:
            runs = samples[variant]
            summary = dict(runs[-1])
            if 'error' not in summary:
                for key in ['wall_median_ms', 'cpu_median_ms']:
                    summary[key] = statistics.median(run[key] for run in runs)
                if all(run.get('user_instructions') is not None for run in runs):
                    summary['user_instructions'] = int(statistics.median(run['user_instructions'] for run in runs))
                summary['rss_peak_bytes'] = int(statistics.median(run['rss_peak_bytes'] for run in runs))
                summary['measured_samples'] = args.samples * len(runs)
            summary['rounds'] = runs
            row['time'][variant] = summary
            row['heap'][variant] = run(getattr(args, variant + '_heap'), chapter, case, 5)
        if all('error' not in sample for variant in samples.values() for sample in variant):
            row['paired_change_percent'] = {key: 100 * statistics.median(after[key] / before[key] - 1 for before, after in zip(samples['before'], samples['after'])) for key in ['wall_median_ms', 'cpu_median_ms']}
        before, after = row['time']['before'], row['time']['after']
        if 'error' not in before and 'error' not in after:
            for key in ['glyphs', 'lines', 'render_fingerprint', 'source_fingerprint', 'text_color_fingerprint']:
                assert before[key] == after[key], (chapter.name, case, key, before[key], after[key])
        if case == 'paint' and 'error' in before and 'error' not in after:
            reference = run(args.before_time, chapter, 'reference', args.samples)
            assert 'error' not in reference, reference
            row['full_layout_reference'] = reference
            for key in ['glyphs', 'lines', 'render_fingerprint', 'source_fingerprint', 'text_color_fingerprint']:
                assert reference[key] == after[key], (chapter.name, case, key, reference[key], after[key])
        results.append(row)
        Path(args.output).write_text(json.dumps({'cpu': args.cpu, 'results': results}, indent=2) + '\n')
        print(json.dumps(row), flush=True)
