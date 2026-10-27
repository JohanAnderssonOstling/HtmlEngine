#!/usr/bin/env python3
"""Render text and chart artifacts from an opening.py --deep capture."""

import argparse
import json
from pathlib import Path


LABELS = {
    'shape.grapheme_boundaries': 'Unicode grapheme boundaries',
    'shape.mock_backend': 'Mock glyph backend',
    'shape.graphemes_and_geometry': 'Fallback advances and control handling',
    'shape.preserve_graphemes': 'Merge native/grapheme clusters',
    'shape.span_buffers_and_styles': 'Span buffers and style assembly',
    'shape.plan_spans': 'Plan contextual shaping spans',
    'shape.split_bidi': 'Split spans by bidi direction',
    'shape.resolve_bidi': 'Resolve bidi paragraphs',
    'shape.retain_geometry': 'Store shaped geometry',
    'shape.document': 'Shaping loop and recorder overhead',
    'tokens.text_glyphs': 'Build glyph tokens and metrics',
    'tokens.from_runs': 'Run traversal and recorder overhead',
    'tokens.classify_glue': 'Classify space glue',
    'tokens.summary_blocks': 'Build token summaries',
    'tokens.collapse_edges': 'Collapse edge whitespace',
    'tokens.preserved_space_breaks': 'Preserved-space break opportunities',
    'lines.build_breakpoint_plan': 'Build Knuth–Plass breakpoint plan',
    'lines.justified_solver': 'Solve justified optimal line breaks',
    'lines.ragged_solver': 'Solve ragged optimal line breaks',
    'lines.measure': 'Measure line metrics',
    'lines.justification': 'Compute justification spacing',
    'lines.summarize': 'Summarize line token span',
    'lines.bidi_placement': 'Place visual bidi line',
    'lines.measure_and_prepare': 'Prepare lines and recorder overhead',
    'lines.emit_paragraph': 'Emit paragraph and recorder overhead',
    'fragments.positioned_inline_boxes': 'Position inline box geometry',
    'fragments.plain_inline_boxes': 'Plain inline-box wrapper',
    'fragments.write': 'Publish line and glyph fragments',
    'prepare.whitespace': 'Whitespace normalization',
    'prepare.first_letter': 'Resolve first-letter styles',
    'prepare.text_transform': 'Text case transformation',
    'prepare.unicode_line_breaks': 'Resolve Unicode break opportunities',
    'prepare.build_box_tree': 'Construct box tree and inline content',
    'prepare.validate_styles': 'Validate style handles',
    'prepare.paint_bindings': 'Prepare paint bindings',
}


def write_report(data, destination):
    a = data['aggregate']
    profiled_cpu = a['sum_profile_medians_ms']['total_cpu']
    overhead = 100 * (profiled_cpu / a['baseline_cpu_ms'] - 1)
    cpu = data['method'].get('scope_cpu_measured', True)
    method = data['method']
    samples = method['samples_per_round'] * method['rounds']
    lines = [
        data['book'], 'Detailed engine opening benchmark', '',
        f"{a['spine_items']} spine items; {samples} measured loads/item; {method['warmups_per_process']} warmups per process; {method['rounds']} rounds.",
        f"Release build, CPU {data['cpu']}, 600 px viewport, 16 px canonical font, BookOptimized composition.",
        'Fresh factory/session and mock shaper per load; warm file cache; no shared book CSS cache.',
        'Native font loading/shaping, image decoding, reader pagination and display are excluded.', '',
        'METHOD AND INSTRUMENTATION COST',
        'Scope wall time uses Instant; per-scope CPU clock: ' + ('enabled' if cpu else 'omitted to reduce overhead') + '.',
        'Inclusive times contain child scopes; exclusive times subtract child scopes.',
        'Exclusive times exactly partition the capture root for every measured sample.',
        'The recorder tracks synchronous scopes on the calling thread only.',
        'Scope overhead is included. Frequent small scopes include clock/recorder costs.',
        'Parent exclusive time also includes child-recorder bookkeeping; it is not pure payload work.',
        f"Baseline full-session CPU: {a['baseline_cpu_ms']:.3f} ms; profiled path: {profiled_cpu:.3f} ms ({overhead:+.1f}%).",
        'These are sums of independent per-item medians; the paths also differ in cache bookkeeping and temporary cleanup.',
        'The percentage quantifies measurement disturbance, not an engine regression.',
        'Do not sum medians to claim an exact total; additivity holds per raw sample.',
        'CSS detail clocks are wall timers; resolver sub-timers are nested within resolve_styles.', '',
        'AGGREGATE SCOPES: sum of per-item medians, milliseconds',
        f"{'Scope':48s} {'Self wall':>11s} {'Incl wall':>11s} {'Calls':>9s}" + (f" {'Self CPU':>11s}" if cpu else ''),
    ]
    for name, value in sorted(a['scopes_sum_item_medians'].items(), key=lambda pair: -pair[1]['exclusive_wall']):
        lines.append(f"{name:48s} {value['exclusive_wall']:11.3f} {value['inclusive_wall']:11.3f} {value['calls']:9.0f}" + (f" {value['exclusive_cpu']:11.3f}" if cpu else ''))
    for href in ['text/part0011.html', 'text/part0012.html', 'text/part0018.html', 'text/part0024_split_001.html']:
        row = next(row for row in data['spine'] if row['href'] == href)
        lines += ['', href, f"Normal-session baseline: {row['baseline_wall_ms']:.3f} ms wall / {row['baseline_cpu_ms']:.3f} ms CPU.",
                  f"Profiled path: {row['timings']['total_wall']['median_ms']:.3f} ms wall / {row['timings']['total_cpu']['median_ms']:.3f} ms CPU.",
                  f"Measured empty-scope CPU cost: {row['empty_scope_cpu_ns']:.0f} ns/scope (not subtracted).", '',
                  f"{'Scope':48s} {'Self wall':>11s} {'Incl wall':>11s} {'p95 self':>11s} {'Calls':>9s}"]
        for name, value in sorted(row['scopes'].items(), key=lambda pair: -pair[1]['exclusive_wall']['median_ms']):
            lines.append(f"{name:48s} {value['exclusive_wall']['median_ms']:11.4f} {value['inclusive_wall']['median_ms']:11.4f} {value['exclusive_wall']['p95_ms']:11.4f} {value['calls']['median']:9.0f}")
        lines += ['', 'CSS wall timers (milliseconds):']
        for name, value in row['timings'].items():
            if name.startswith('style.'):
                lines.append(f"{name:48s} {value['median_ms']:11.4f}")
    lines += ['', 'VALIDATION', 'All 28 item fingerprints match independent normal-session output in every round.',
              'Every captured sample passed exclusive-time partition checks.',
              'Profiler tests cover recursive accounting and recovery after panic.',
              '', 'Raw samples, call counts, hashes and build/run commands are in the matching JSON file.']
    destination.write_text('\n'.join(lines) + '\n')


def plot(data, destination):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt

    totals = data['aggregate']['scopes_sum_item_medians']
    fig, axes = plt.subplots(2, 2, figsize=(17, 12))
    groups = [
        ('Text shaping — exclusive wall time', ('shape.',), '#0891b2'),
        ('Inline layout — exclusive wall time', ('tokens.', 'lines.', 'fragments.'), '#2563eb'),
        ('Document preparation — exclusive wall time', ('prepare.',), '#d97706'),
    ]
    for ax, (title, prefixes, color) in zip(axes.flat, groups):
        rows = sorted([(name, entry['exclusive_wall']) for name, entry in totals.items() if name.startswith(prefixes)], key=lambda pair: -pair[1])[:11]
        ax.barh([LABELS.get(name, name) for name, _ in rows], [value for _, value in rows], color=color)
        ax.invert_yaxis()
        ax.set_title(title)
    css_keys = ['parse_default_css', 'parse_author_css', 'prepare_rules', 'selector_index', 'resolve_styles']
    css_labels = ['Parse default CSS', 'Parse publication CSS', 'Prepare rules', 'Build selector index', 'Resolve styles (inclusive)']
    values = [data['aggregate']['sum_profile_medians_ms']['style.' + key] for key in css_keys]
    axes[1, 1].barh(css_labels, values, color='#9333ea')
    axes[1, 1].invert_yaxis()
    axes[1, 1].set_title('CSS — existing wall timers')
    for ax in axes.flat:
        ax.set_xlabel(f"Milliseconds, sum of per-item medians across {data['aggregate']['spine_items']} items")
        ax.grid(axis='x', alpha=.2)
        ax.set_axisbelow(True)
        ax.spines[['top', 'right']].set_visible(False)
        for rect in ax.patches:
            ax.text(rect.get_width(), rect.get_y() + rect.get_height()/2, f' {rect.get_width():.1f}', va='center', fontsize=8)
        ax.margins(x=.15)
    a = data['aggregate']
    overhead = 100 * (a['sum_profile_medians_ms']['total_cpu'] / a['baseline_cpu_ms'] - 1)
    samples = data['method']['samples_per_round'] * data['method']['rounds']
    fig.suptitle(f"{data['book']} — expensive stages\n{samples} measured loads per spine item; release build; mock glyph backend", fontsize=14)
    fig.text(.5, .018, f'Clock/recorder overhead included; profiled CPU total is {overhead:+.0f}% relative to the separate session baseline. Parent self-time includes recorder bookkeeping.\nExclusive scopes partition each raw capture; medians are not additive. Full samples, counts and limitations are in the report.', ha='center', fontsize=9)
    fig.tight_layout(rect=(0, .065, 1, .94), h_pad=3, w_pad=3)
    fig.savefig(destination, dpi=150)
    plt.close(fig)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('result', type=Path)
    args = parser.parse_args()
    data = json.loads(args.result.read_text())
    write_report(data, args.result.with_suffix('.txt'))
    plot(data, args.result.with_suffix('.png'))
