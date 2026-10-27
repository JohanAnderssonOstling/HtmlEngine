#!/usr/bin/env python3
"""Profile every EPUB spine item with fresh engine sessions and warm OS caches.

Build:
  cargo build --release --locked --manifest-path tools/pipeline-bench/Cargo.toml
Run:
  python3 tools/pipeline-bench/opening.py --binary <pipeline-bench> \
    --epub <book.epub> --cpu 8 --output <results.json>

Archive timings use Python's ZIP/XML readers as a host-side reference. They do
not represent the reader application's EPUB implementation or first paint.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import posixpath
import statistics
import subprocess
import tempfile
import time
from datetime import datetime, timezone
import xml.etree.ElementTree as ET
from zipfile import ZipFile


def describe(values):
    ordered = sorted(values)
    return {
        'median_ms': statistics.median(ordered),
        'p95_ms': ordered[(len(ordered) * 95 + 99) // 100 - 1],
        'min_ms': ordered[0],
        'max_ms': ordered[-1],
        'samples_ms': values,
    }


def package(archive):
    container = ET.fromstring(archive.read('META-INF/container.xml'))
    rootfile = next(node for node in container.iter() if node.tag.endswith('}rootfile'))
    package_path = rootfile.attrib['full-path']
    root = ET.fromstring(archive.read(package_path))
    ns = {'opf': 'http://www.idpf.org/2007/opf', 'dc': 'http://purl.org/dc/elements/1.1/'}
    manifest = {item.attrib['id']: item.attrib for item in root.findall('opf:manifest/opf:item', ns)}
    spine = [manifest[item.attrib['idref']]['href'] for item in root.findall('opf:spine/opf:itemref', ns)]
    base = posixpath.dirname(package_path)
    spine = [posixpath.normpath(posixpath.join(base, href)) for href in spine]
    return root.findtext('opf:metadata/dc:title', namespaces=ns), spine


def plot(result, destination):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt

    stages = [
        ('source_read', 'Read source', '#94a3b8'),
        ('parse', 'Parse markup', '#0ea5e9'),
        ('prepare.dom', 'Build DOM', '#6366f1'),
        ('prepare.stylesheets', 'Read/expand CSS', '#8b5cf6'),
        ('prepare.style', 'Compile CSS + resolve styles', '#a855f7'),
        ('prepare.box_tree', 'Prepare box tree', '#f59e0b'),
        ('paint_prepare', 'Prepare paint bindings', '#f97316'),
        ('shape', 'Prepare/shape text (mock backend)', '#10b981'),
        ('layout', 'Layout + finalize', '#2563eb'),
    ]
    totals = result['aggregate']['sum_profile_medians_ms']
    fig, axes = plt.subplots(1, 2, figsize=(15, 11), gridspec_kw={'width_ratios': [1, 1.5]})
    values = [totals[key] for key, _, _ in stages]
    axes[0].barh([label for _, label, _ in stages], values, color=[color for _, _, color in stages])
    axes[0].invert_yaxis()
    axes[0].set_title('Stage totals across all spine items\nSum of medians; nested timers excluded')
    for index, value in enumerate(values):
        axes[0].text(value + max(values) * .02, index, f'{value:.1f}', va='center', fontsize=9)
    axes[0].set_xlim(0, max(values) * 1.25)
    rows = result['spine']
    offsets = [0.0] * len(rows)
    positions = list(range(len(rows)))
    for key, label, color in stages:
        widths = [row['timings'][key]['median_ms'] for row in rows]
        axes[1].barh(positions, widths, left=offsets, color=color, label=label)
        offsets = [left + width for left, width in zip(offsets, widths)]
    axes[1].scatter([row['baseline_wall_ms'] for row in rows], positions, color='#111827', s=14, marker='D', label='Full session baseline', zorder=4)
    axes[1].set_yticks(positions, [Path(row['href']).name for row in rows], fontsize=8)
    axes[1].invert_yaxis()
    axes[1].set_title('Opening each spine item\nStage medians; diamonds = independent full-session baseline')
    for ax in axes:
        ax.set_xlabel('Milliseconds')
        ax.grid(axis='x', alpha=.2)
        ax.set_axisbelow(True)
        ax.spines[['top', 'right']].set_visible(False)
    handles, labels = axes[1].get_legend_handles_labels()
    fig.legend(handles, labels, loc='lower center', ncol=3, fontsize=8, frameon=False, bbox_to_anchor=(.5, .025))
    fig.suptitle(result['book'] + '\nBook opening benchmark — engine processing', fontsize=14)
    if result.get('environment_notes', {}).get('system_load'):
        fig.text(.5, .9, 'Busy system: wall timings include scheduler contention (see environment notes in the results).', ha='center', fontsize=9, color='#92400e')
    sample_count = result['method']['samples_per_round'] * result['method']['rounds']
    fig.text(.5, .005, f"Release build · CPU {result['cpu']} · {sample_count} samples/item · fresh sessions, warm OS cache · mock font backend · excludes image decode, app UI and first paint", ha='center', fontsize=8)
    fig.tight_layout(rect=(0, .12, 1, .94))
    destination.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(destination, dpi=160)
    plt.close(fig)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--epub', type=Path, required=True)
    parser.add_argument('--cpu', type=int, required=True)
    parser.add_argument('--samples', type=int, default=11)
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--chart', type=Path, help='Optional PNG/SVG chart; requires matplotlib')
    parser.add_argument('--deep', action='store_true', help='Collect nested exclusive/inclusive CPU and wall times; build binary with --features profiling')
    parser.add_argument('--wall-scopes', action='store_true', help='With --deep, omit CPU syscalls inside scopes to reduce instrumentation overhead; whole-load CPU time is still recorded')
    args = parser.parse_args()
    if args.samples < 1 or args.rounds < 1:
        parser.error('--samples and --rounds must be positive')
    if args.wall_scopes and not args.deep:
        parser.error('--wall-scopes requires --deep')
    args.binary = args.binary.resolve()
    args.epub = args.epub.resolve()
    os.sched_setaffinity(0, {args.cpu})
    archive_samples = {key: [] for key in ['zip_directory', 'container_and_package_xml', 'first_spine_decompress', 'total']}
    for iteration in range(args.samples + 3):
        start = time.perf_counter()
        with ZipFile(args.epub) as archive:
            opened = time.perf_counter()
            title, spine = package(archive)
            packaged = time.perf_counter()
            archive.read(spine[0])
            finished = time.perf_counter()
        if iteration >= 3:
            for name, duration in [
                ('zip_directory', opened - start),
                ('container_and_package_xml', packaged - opened),
                ('first_spine_decompress', finished - packaged),
                ('total', finished - start),
            ]:
                archive_samples[name].append(duration * 1000)
    result = {
        'measured_at_utc': datetime.now(timezone.utc).isoformat(),
        'book': title,
        'epub': str(args.epub),
        'epub_sha256': hashlib.sha256(args.epub.read_bytes()).hexdigest(),
        'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        'platform': platform.platform(),
        'cpu_model': next(line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),
        'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
        'cpu': args.cpu,
        'method': {
            'samples_per_round': args.samples,
            'rounds': args.rounds,
            'warmups_per_process': 3,
            'settings': '600px viewport, 16px canonical font, BookOptimized composition, publication colors and linked CSS, deterministic mock glyph shaper',
            'cache': 'Fresh session/factory/shaper for every sample; warm OS page cache; no shared compiled stylesheet cache',
            'baseline': 'cold_book: complete PipelineSession::update, source already loaded; no heap instrumentation',
            'profile': 'open_profile_book: source read + parse + timed factory + paint preparation + shape + production layout; separate second layout for nested timers',
            'validation': 'Render fingerprints must match detailed layout and independent baseline for every spine item and round',
            'aggregation': 'Sum of independent per-spine medians; eager processing estimate, not measured time to first page',
            'nested_timers': 'Style/layout detail timers overlap and must not be summed; detailed layout is separately instrumented and can be slower',
            'excludes': 'Real font loading/shaping/rasterization, image decoding, pagination, GPU/display, app startup and storage; archive timings are a Python host reference only',
            'deep_scopes': args.deep,
        },
        'archive_reference': {name: describe(values) for name, values in archive_samples.items()},
        'spine': [],
    }
    if args.deep:
        result['method']['profile'] = 'open_deep_book: optional synchronous scope instrumentation of preparation, shaping and production layout; inclusive/exclusive process CPU and wall clocks'
        result['method']['validation'] = 'Profile fingerprints must match independent normal-session baselines in all rounds; exclusive scope times must exactly partition each capture root'
        result['method']['nested_timers'] = 'Inclusive scope times overlap; exclusive scope times partition the root per sample. CPU clock and recorder overhead are not subtracted. Scopes on other threads are not recorded.'
        result['method']['scope_cpu_measured'] = not args.wall_scopes
        if args.wall_scopes:
            result['method']['profile'] = 'open_deep_wall_book: optional synchronous wall-clock scopes of preparation, shaping and production layout; whole-load process CPU measured outside capture; per-scope CPU omitted'
    with tempfile.TemporaryDirectory(prefix='html-opening-bench-') as directory:
        with ZipFile(args.epub) as archive:
            archive.extractall(directory)
        for index, href in enumerate(spine):
            path = Path(directory, href)
            runs = {'baseline': [], 'profile': []}
            for round_index in range(args.rounds):
                order = ['baseline', 'profile'] if round_index % 2 == 0 else ['profile', 'baseline']
                for variant in order:
                    case = 'cold_book' if variant == 'baseline' else (('open_deep_wall_book' if args.wall_scopes else 'open_deep_book') if args.deep else 'open_profile_book')
                    process = subprocess.run([str(args.binary), str(path), case, str(args.samples), 'default'], capture_output=True, text=True, check=True)
                    data = json.loads(process.stdout)
                    if data['heap_instrumented']:
                        raise RuntimeError('Use a binary built without the heap feature')
                    if variant == 'baseline':
                        data = {key: data[key] for key in ['wall_median_ms', 'cpu_median_ms', 'glyphs', 'lines', 'render_fingerprint', 'user_instructions']}
                    else:
                        data.pop('path')
                    runs[variant].append(data)
            fingerprints = {run['render_fingerprint'] for group in runs.values() for run in group}
            assert len(fingerprints) == 1, f'Output mismatch for {href}: {fingerprints}'
            row = {
                'href': href,
                'source_bytes': path.stat().st_size,
                'glyphs': runs['baseline'][0]['glyphs'],
                'lines': runs['baseline'][0]['lines'],
                'baseline_wall_ms': statistics.median(run['wall_median_ms'] for run in runs['baseline']),
                'baseline_cpu_ms': statistics.median(run['cpu_median_ms'] for run in runs['baseline']),
                'timings': {key: describe([sample for run in runs['profile'] for sample in run['timings'][key]['samples_ms']]) for key in runs['profile'][0]['timings']},
                'rounds': runs,
            }
            result['spine'].append(row)
            if args.deep:
                assert all(run['exclusive_times_partition_root'] for run in runs['profile'])
                names = sorted({name for run in runs['profile'] for name in run['scopes']})
                row['scopes'] = {}
                for name in names:
                    scope = {}
                    for metric in (['inclusive_cpu', 'exclusive_cpu'] if not args.wall_scopes else []) + ['inclusive_wall', 'exclusive_wall', 'calls']:
                        values = []
                        for run in runs['profile']:
                            entry = run['scopes'].get(name)
                            values.extend(entry[metric]['samples' if metric == 'calls' else 'samples_ms'] if entry else [0.0] * args.samples)
                        scope[metric] = ({'median': statistics.median(values), 'min': min(values), 'max': max(values), 'samples': values} if metric == 'calls' else describe(values))
                    row['scopes'][name] = scope
                row['empty_scope_cpu_ns'] = statistics.median(run['empty_scope_cpu_ns'] for run in runs['profile'])
            print(f"{index + 1}/{len(spine)} {href}: {row['baseline_wall_ms']:.3f} ms", flush=True)
    result['aggregate'] = {
        'spine_items': len(spine),
        'source_bytes': sum(row['source_bytes'] for row in result['spine']),
        'glyphs': sum(row['glyphs'] for row in result['spine']),
        'lines': sum(row['lines'] for row in result['spine']),
        'baseline_wall_ms': sum(row['baseline_wall_ms'] for row in result['spine']),
        'baseline_cpu_ms': sum(row['baseline_cpu_ms'] for row in result['spine']),
        'sum_profile_medians_ms': {key: sum(row['timings'][key]['median_ms'] for row in result['spine']) for key in result['spine'][0]['timings']},
        'all_fingerprints_match': True,
    }
    if args.deep:
        names = sorted({name for row in result['spine'] for name in row['scopes']})
        result['aggregate']['scopes_sum_item_medians'] = {
            name: {metric: sum(row['scopes'][name][metric]['median' if metric == 'calls' else 'median_ms'] for row in result['spine'] if name in row['scopes']) for metric in (['inclusive_cpu', 'exclusive_cpu'] if not args.wall_scopes else []) + ['inclusive_wall', 'exclusive_wall', 'calls']}
            for name in names
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    if args.chart:
        plot(result, args.chart)
    print(json.dumps(result['aggregate'], indent=2))


if __name__ == '__main__':
    main()
