#!/usr/bin/env python3
"""Untimed first/middle/last viewport checks against a preserved Skrift host."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import threading

from PIL import Image, ImageChops

from run import read_stdout, validate_viewport


def capture(binary, fixture, directory, label):
    environment = os.environ.copy()
    environment.pop('WAYLAND_DISPLAY', None)
    environment.update(GDK_BACKEND='x11', GDK_SCALE='1', GDK_DPI_SCALE='1', GPUI_X11_SCALE_FACTOR='1')
    command = [str(binary), '--fixture', str(fixture), '--width', '800', '--height', '600',
               '--native-window', '--content', 'skrift']
    with (directory / (label + '.log')).open('w') as log:
        proc = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
                                text=True, env=environment, start_new_session=True)
        events = queue.Queue()
        threading.Thread(target=read_stdout, args=(proc.stdout, events), daemon=True).start()
        def event(expected, phase=None):
            while True:
                result = events.get(timeout=90)
                if result.get('event') == 'diagnostic':
                    continue
                if result.get('event') != expected or (phase and result.get('phase') != phase):
                    raise RuntimeError(result)
                return result

        def send(command):
            proc.stdin.write(command + '\n')
            proc.stdin.flush()

        try:
            validate_viewport(event('ready', 'native'), 800, 600, 1, native=True)
            send('load-all')
            document = event('ready', 'document')
            validate_viewport(document, 800, 600, 1)
            height = float(document['page']['content_height'])
            captures = []
            for name, offset in [('first', 0), ('middle', max(0, (height - 600) / 2)), ('last', max(0, height - 600))]:
                send(f'tile {offset}')
                tile = event('ready', 'tile')
                path = directory / f'{label}-{name}.png'
                send(f'capture {path}')
                result = event('capture')
                assert result['path'] == str(path)
                captures.append({'path': str(path), 'scroll_y': tile['page']['scroll_y']})
            send('quit')
            proc.wait(timeout=10)
            assert proc.returncode == 0
            return {'height': height, 'captures': captures}
        finally:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGTERM)
                proc.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', type=Path, required=True)
    parser.add_argument('--after', type=Path, required=True)
    parser.add_argument('--fixtures', type=Path, nargs='+', required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    args.output_dir = args.output_dir.resolve()
    args.output_dir.mkdir(parents=True, exist_ok=False)
    report = {'timed': False, 'binaries': {}, 'fixtures': []}
    for label in ('before', 'after'):
        path = getattr(args, label).resolve(strict=True)
        setattr(args, label, path)
        report['binaries'][label] = {'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
    for fixture in args.fixtures:
        fixture = fixture.resolve(strict=True)
        directory = args.output_dir / fixture.stem
        directory.mkdir()
        row = {'fixture': str(fixture)}
        for label in ('before', 'after'):
            row[label] = capture(getattr(args, label), fixture, directory, label)
        row['matching_height'] = abs(row['before']['height'] - row['after']['height']) < .01
        row['different_pixels'] = []
        for before, after in zip(row['before']['captures'], row['after']['captures']):
            with Image.open(before['path']) as a, Image.open(after['path']) as b:
                difference = ImageChops.difference(a.convert('RGBA'), b.convert('RGBA'))
                row['different_pixels'].append(sum(pixel != 0 for pixel in memoryview(difference.tobytes()).cast('I')))
        report['fixtures'].append(row)
        (args.output_dir / 'verification.json').write_text(json.dumps(report, indent=2) + '\n')
        print(f"{fixture.name}: same height={row['matching_height']}, different pixels={row['different_pixels']}", flush=True)
    assert all(row['matching_height'] and not any(row['different_pixels']) for row in report['fixtures'])


if __name__ == '__main__':
    main()
