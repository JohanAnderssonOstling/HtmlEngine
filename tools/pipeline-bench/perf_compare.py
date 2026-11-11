#!/usr/bin/env python3
"""Compare two layout-profile binaries with scoped perf counters and samples.

Build both binaries with identical release/debug/frame-pointer flags first.
Each --case supplies NAME HTML CSS STAT_ITERATIONS PROFILE_ITERATIONS.
The output retains raw measurements and one CSV of sampled periods per symbol.
"""
import argparse
import collections
import csv
import hashlib
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import tempfile

EVENTS = ['cycles', 'instructions', 'cache-misses', 'branch-misses']


def run(command, output, env=None):
    with output.open('w') as stdout, output.with_suffix('.log').open('w') as stderr:
        subprocess.run(command, stdout=stdout, stderr=stderr, env=env, check=True)


def measure(args, version, case, mode, index, iterations):
    name, html, css, _, _ = case
    prefix = args.output / f'{name}-{version}-{mode}-{index}'
    with tempfile.TemporaryDirectory(dir=args.output) as directory:
        control, ack = Path(directory) / 'control', Path(directory) / 'ack'
        os.mkfifo(control)
        os.mkfifo(ack)
        env = dict(os.environ, LAYOUT_PERF_CONTROL=str(control), LAYOUT_PERF_ACK=str(ack))
        command = ['perf', mode, '--delay=-1', f'--control=fifo:{control},{ack}',
                   '-e', ','.join(event + ':u' for event in EVENTS)]
        if mode == 'stat':
            command += ['-x', ',', '-o', str(prefix.with_suffix('.csv'))]
        else:
            command += ['-F', '999', '-o', str(prefix.with_suffix('.data'))]
        command += ['--', 'taskset', '-c', str(args.cpu), str(getattr(args, version)),
                    html, css, 'pipeline', str(iterations)]
        run(command, prefix.with_suffix('.json'), env)
    return prefix


def counters(path):
    result = {}
    for row in csv.reader(path.open()):
        if len(row) > 4 and row[2].endswith(':u'):
            result[row[2][:-2]] = float(row[0])
            if float(row[4]) < 99:
                raise RuntimeError(f'Multiplexed counters in {path}: {row}')
    if set(result) != set(EVENTS):
        raise RuntimeError(f'Missing counters in {path}: {result}')
    return result


def symbols(path):
    result = collections.defaultdict(lambda: collections.defaultdict(int))
    samples = collections.Counter()
    process = subprocess.Popen(['perf', 'script', '-i', str(path), '-F', 'event,period,ip,sym,dso'],
                               stdout=subprocess.PIPE, text=True)
    pattern = re.compile(r'^\s*(\d+)\s+(\S+):u:\s+\S+\s+(.*?)\s+\((.*)\)$')
    for line in process.stdout:
        match = pattern.match(line)
        if not match:
            raise RuntimeError(f'Unparsed perf sample: {line!r}')
        period, event, symbol, dso = match.groups()
        if symbol == '[unknown]':
            symbol += ' ' + Path(dso).name
        result[symbol][event] += int(period)
        samples[event] += 1
    if process.wait() != 0:
        raise RuntimeError(f'perf script failed: {path}')
    return result, samples


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cpu', type=int, default=2)
    parser.add_argument('--repeats', type=int, default=5)
    parser.add_argument('--case', nargs=5, action='append', required=True)
    args = parser.parse_args()
    args.before = args.before.resolve()
    args.after = args.after.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    manifest = dict(cpu=args.cpu, repeats=args.repeats, events=EVENTS, cases=args.case,
                    inputs={path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
                            for case in args.case for path in case[1:3]},
                    binaries={v: {'path': str(getattr(args, v)), 'sha256': hashlib.sha256(getattr(args, v).read_bytes()).hexdigest()}
                              for v in ['before', 'after']})
    (args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    summary = {}
    for case in args.case:
        name, html, css, stat_iterations, profile_iterations = case
        stat_iterations, profile_iterations = int(stat_iterations), int(profile_iterations)
        snapshots = []
        for version in ['before', 'after']:
            snapshot = args.output / f'{name}-{version}-geometry.json'
            run(['taskset', '-c', str(args.cpu), str(getattr(args, version)), html, css, 'pipeline', '1'],
                args.output / f'{name}-{version}-snapshot.json', dict(os.environ, LAYOUT_SNAPSHOT=str(snapshot)))
            snapshots.append(snapshot.read_bytes())
        if snapshots[0] != snapshots[1]:
            raise RuntimeError(f'{name}: geometry/link snapshot mismatch')
        print(f'{name}: geometry and links match', flush=True)
        measurements = collections.defaultdict(list)
        for repetition in range(args.repeats):
            for version in (['before', 'after'] if repetition % 2 == 0 else ['after', 'before']):
                prefix = measure(args, version, case, 'stat', repetition, stat_iterations)
                wall = json.loads(prefix.with_suffix('.json').read_text())
                values = {event: value / stat_iterations for event, value in counters(prefix.with_suffix('.csv')).items()}
                values['wall_ms'] = statistics.median(wall['samples_ms'])
                values.update({stage + '_ms': statistics.median(times) for stage, times in wall['stages_ms'].items()})
                measurements[version].append(values)
            print(f'{name}: counter pair {repetition + 1}/{args.repeats}', flush=True)
        summary[name] = {v: {key: statistics.median(row[key] for row in rows) for key in rows[0]}
                         for v, rows in measurements.items()}
        summary[name]['runs'] = dict(measurements)
        sampled = {}
        for version in ['before', 'after']:
            prefix = measure(args, version, case, 'record', 0, profile_iterations)
            sampled[version], sample_counts = symbols(prefix.with_suffix('.data'))
            summary[name][version]['profile_samples'] = dict(sample_counts)
            print(f'{name}: {version} samples {dict(sample_counts)}', flush=True)
        with (args.output / f'{name}-functions.csv').open('w') as output:
            writer = csv.writer(output)
            writer.writerow(['symbol'] + [f'{v}_{e}_sampled_per_iteration' for e in EVENTS for v in ['before', 'after']])
            names = set(sampled['before']) | set(sampled['after'])
            for symbol in sorted(names, key=lambda s: -sampled['before'].get(s, {}).get('cycles', 0)):
                writer.writerow([symbol] + [sampled[v].get(symbol, {}).get(e, 0) / profile_iterations for e in EVENTS for v in ['before', 'after']])
        (args.output / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps({name: {v: row[v] for v in ['before', 'after']} for name, row in summary.items()}, indent=2))


if __name__ == '__main__':
    main()
