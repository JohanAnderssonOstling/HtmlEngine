#!/usr/bin/env python3
"""Attribute native token-builder samples or compare token-construction builds."""

import argparse
import collections
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import statistics
import subprocess


BUILDER = 'html_layout::layout::inline::text_tokens::append_text_tokens'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def timing_quality(captures):
    rounds = []
    for capture in captures:
        rounds.append({name: {
            'isolated_cpu_ms': capture[name]['book']['timings']['isolated/book/cpu']['median_ms'],
            'shared_cpu_ms': capture[name]['book']['timings']['shared/book/cpu']['median_ms'],
            'text_emit_wall_ms': sum(
                (row['scopes']['tokens.text_emit'] or {'inclusive_wall': {'median_ms': 0}})
                ['inclusive_wall']['median_ms'] for row in capture[name]['sections']),
        } for name in ['before', 'after']})
    spreads = {name: max(row[name]['isolated_cpu_ms'] for row in rounds) /
               min(row[name]['isolated_cpu_ms'] for row in rounds) for name in ['before', 'after']}
    return {
        'rounds': rounds, 'opening_cpu_round_max_over_min': spreads,
        'large_environment_variation': any(value > 1.25 for value in spreads.values()),
        'screen': 'A >25% opening CPU spread across rounds flags environment variation; this is a screening rule, not a significance test. Do not infer startup speedups from unstable book batches.',
        'text_emit_improved_every_round': all(
            row['after']['text_emit_wall_ms'] < row['before']['text_emit_wall_ms'] for row in rounds),
    }


def profile(args):
    symbols = subprocess.check_output(['nm', '-S', '-C', str(args.binary)], text=True)
    start = next(int(row.split()[0], 16) for row in symbols.splitlines()
                 if row.endswith(' ' + BUILDER))
    records = []
    total = 0

    def consume(frames):
        nonlocal total
        if not frames:
            return
        total += 1
        for index, frame in enumerate(frames):
            if BUILDER + '+' in frame:
                offset = int(re.search(r'append_text_tokens\+0x([0-9a-f]+)', frame).group(1), 16)
                # Caller addresses point after the call; attribute its final byte.
                records.append((start + offset - (index > 0),
                                re.sub(r'\+0x[0-9a-f]+$', '',
                                       frames[0].split(' (')[0].split(None, 1)[1])))
                return

    frames = []
    with args.stacks.open() as stream:
        for row in stream:
            if not row.strip():
                consume(frames)
                frames = []
            elif row.startswith('\t'):
                frames.append(row.strip())
        consume(frames)
    assert records, 'No token-builder stacks; check frame pointers and symbols.'
    addresses = sorted({address for address, _ in records})
    resolved = subprocess.check_output(
        ['addr2line', '-a', '-C', '-f', '-i', '-e', str(args.binary)],
        input=''.join(f'{address:#x}\n' for address in addresses), text=True)
    locations = {}
    current = None
    for row in resolved.splitlines():
        if re.fullmatch('0x[0-9a-f]+', row):
            current = int(row, 16)
            locations[current] = []
        else:
            locations[current].append(row)

    source = args.source.read_text().splitlines()
    begin = next(i for i, row in enumerate(source) if 'pub(super) fn append_text_tokens(' in row)
    end = next(i for i in range(begin, len(source)) if 'fn create_ellipsis_token(' in source[i])

    def line(marker):
        return next(i + 1 for i in range(begin, end) if marker in source[i])

    ranges = [
        (line('for glyph_idx in glyphs'), 'loop_and_unattributed'),
        (line('let glyph = engine.text.glyph_at'), 'glyph_and_metric_reads'),
        (line('let (advance, cluster_boundary)'), 'advance_and_cluster_geometry'),
        (line('let line_height = fixed_line_height'), 'line_height'),
        (line('let unit = canonical_text_unit_from_facts'), 'break_and_width_classification'),
        (line('let tab_advance ='), 'preserved_tabs'),
        (line('let token_kind ='), 'token_construction'),
        (line('let placement_required = placement_required'), 'metrics_reuse_and_binding'),
        (line('dense.push(token)'), 'token_buffer_write'),
        (line('if let Some((hyphen_glyph, hyphen_width))'), 'discretionary_hyphens'),
    ]
    ranges.sort()
    counts = collections.Counter()
    address_counts = collections.Counter(address for address, _ in records)
    rows = []
    for address in addresses:
        lines = [int(m.group(1)) for row in locations[address]
                 if (m := re.search(r'/inline/text_tokens.rs:(\d+)', row))]
        outer = lines[-1] if lines else 0
        stage = 'run_setup_and_profiling' if begin < outer < ranges[0][0] else 'loop_and_unattributed'
        if ranges[0][0] <= outer < end - 2:
            stage = next(name for first, name in reversed(ranges) if first <= outer)
        counts[stage] += address_counts[address]
        rows.append({'elf_address': hex(address), 'builder_line': outer, 'stage': stage,
                     'samples': address_counts[address], 'inline_locations': locations[address]})
    result = {
        'binary_sha256': sha(args.binary), 'source_sha256': sha(args.source),
        'total_nonempty_stacks': total, 'builder_samples': len(records),
        'method': 'Unweighted frequency-sampled user cycles; frame-pointer build; source and inline locations from addr2line. Caller return addresses adjusted by one byte. Source attribution is approximate because of optimization, instruction skid, and frame-pointer overhead. Percentages describe builder samples, not exact production stage durations.',
        'stages': {name: {'samples': count, 'percent': 100 * count / len(records)}
                   for name, count in counts.most_common()},
        'leaf_functions': collections.Counter(name for _, name in records),
        'addresses': rows,
    }
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    for name, row in result['stages'].items():
        print(f'{row["percent"]:6.2f}% {name}')


def compare(args):
    os.sched_setaffinity(0, {args.cpu})
    paths = json.loads(args.spine.read_text())
    binaries = {'before': args.before.resolve(), 'after': args.after.resolve()}
    captures = []
    identities = None
    scope_names = ['tokens.text_glyphs', 'tokens.text_emit', 'tokens.text_run_setup', 'tokens.from_runs']
    for round_index in range(args.rounds):
        capture = {}
        order = list(binaries)
        if round_index % 2:
            order.reverse()
        for name in order:
            book = json.loads(subprocess.check_output(
                [str(binaries[name]), str(args.spine.resolve()), 'book_css', str(args.samples)], text=True))
            if identities is None:
                identities = book['identities']
            assert identities == book['identities']
            capture[name] = {'book': book, 'sections': []}
        for index, path in enumerate(paths):
            section_order = order if index % 2 == 0 else order[::-1]
            for name in section_order:
                detailed = json.loads(subprocess.check_output(
                    [str(binaries[name]), path, 'open_deep_wall_book', str(args.samples), 'default'], text=True))
                assert all(detailed[key] == value for key, value in identities[index].items())
                capture[name]['sections'].append({
                    'path': path, 'identity': identities[index],
                    'scopes': {key: detailed['scopes'].get(key) for key in scope_names},
                    'timings': {key: detailed['timings'][key] for key in ['total_cpu', 'total_wall']},
                })
        captures.append(capture)
        print(f'Round {round_index + 1}/{args.rounds}: all {len(paths)} sections identical', flush=True)

    def comparison(before, after):
        return {'before': before, 'after': after, 'change_percent': 100 * (after / before - 1)}

    totals = {}
    for variant in ['isolated', 'shared']:
        for metric in ['cpu', 'wall']:
            values = {name: statistics.median([
                value for capture in captures
                for value in capture[name]['book']['timings'][f'{variant}/book/{metric}']['samples_ms']])
                      for name in binaries}
            totals[f'{variant}_{metric}_ms'] = comparison(values['before'], values['after'])
    scope_totals = {}
    for scope in scope_names:
        values = {}
        for name in binaries:
            values[name] = sum(statistics.median([
                value for capture in captures
                for value in (capture[name]['sections'][index]['scopes'][scope] or
                              {'inclusive_wall': {'samples_ms': [0.0]}})['inclusive_wall']['samples_ms']])
                               for index in range(len(paths)))
        scope_totals[scope] = comparison(values['before'], values['after'])
    # The production session entry point also supplies instruction counts and
    # independent source/color fingerprints, outside the scope-timing captures.
    sessions = []
    for index, path in enumerate(paths):
        row = {'path': path}
        for name in list(binaries)[::1 if index % 2 == 0 else -1]:
            row[name] = json.loads(subprocess.check_output(
                [str(binaries[name]), path, 'cold_book', '5', 'default'], text=True))
        for key in ['render_fingerprint', 'source_fingerprint', 'text_color_fingerprint', 'glyphs', 'lines']:
            assert row['before'][key] == row['after'][key], (path, key)
        for name in binaries:
            assert all(row[name][key] == value for key, value in identities[index].items()), path
        sessions.append(row)
    instructions = {name: sum(row[name]['user_instructions'] for row in sessions)
                    for name in binaries} if all(
                        row[name]['user_instructions'] is not None for row in sessions for name in binaries) else None
    result = {
        'measured_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'binary_sha256': {name: sha(path) for name, path in binaries.items()},
        'method': {'rounds': args.rounds, 'samples_per_round': args.samples, 'cpu': args.cpu,
                   'warmups': 3, 'sections': len(paths),
                   'limits': 'Normal release builds, no forced frame pointers; mock fonts, warm OS caches. Book totals are cumulative timed chapter loads, including initial book-cache compilation. Scope totals sum section medians. Native fonts/images, UI and first paint excluded.'},
        'opening': totals, 'scopes': scope_totals,
        'timing_quality': timing_quality(captures),
        'session_instructions': comparison(instructions['before'], instructions['after']) if instructions else None,
        'validation': {'identical_sections': len(paths), 'book_scope_and_session_match': True,
                       'session_source_and_color_fingerprints_match': True},
        'captures': captures, 'sessions': sessions,
    }
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    for group in ['opening', 'scopes']:
        for name, row in result[group].items():
            print(name, row)
    print('session_instructions', result['session_instructions'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    native = commands.add_parser('profile')
    native.add_argument('binary', type=Path)
    native.add_argument('stacks', type=Path)
    native.add_argument('source', type=Path)
    native.add_argument('--output', type=Path, required=True)
    timing = commands.add_parser('compare')
    timing.add_argument('before', type=Path)
    timing.add_argument('after', type=Path)
    timing.add_argument('spine', type=Path)
    timing.add_argument('--output', type=Path, required=True)
    timing.add_argument('--rounds', type=int, default=3)
    timing.add_argument('--samples', type=int, default=9)
    timing.add_argument('--cpu', type=int, default=8)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.command == 'profile':
        profile(args)
    else:
        assert args.samples > 0 and args.rounds > 0
        compare(args)
