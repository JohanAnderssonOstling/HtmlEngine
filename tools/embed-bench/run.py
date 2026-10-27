#!/usr/bin/env python3
r"""Measure HTML embedding CPU and total process-session RAM on Linux.

Build the shared host with:
  cargo build --release --manifest-path tools/embed-bench/gpui/Cargo.toml \
    --bin embed-bench-gpui-culled --features wry-in-gpui

Run with --native-window --shared-gpui and two --host arguments, labelled
skrift and wry, pointing to that executable. Use --warm-engine to initialize
each engine with an empty page before measurement. Add --full-document to
render successive viewports through the document's end.

One document load per host and fixture. CPU counts every thread and helper in
a private cgroup when --cpu-cgroup-parent is supplied, including exited helpers.
RAM is total process-session PSS before and after loading, including helpers;
shared resident pages count proportionally. GPU VRAM and peak memory are excluded.

The endpoint is a completed viewport snapshot including readback. Verification
PNG encoding occurs after CPU and RAM sampling. Measured results are saved before
verification so a capture failure cannot discard a completed measurement.
Existing reports and captures are never overwritten.
"""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import queue
import random
import signal
import subprocess
import sys
import tempfile
import threading
import time
import uuid


class CpuGroup:
    """A private cgroup retains CPU usage even when helper processes exit."""

    def __init__(self, parent):
        self.path = None if parent is None else parent / f"skrift-embed-bench-{uuid.uuid4().hex}"

    def __enter__(self):
        if self.path is not None:
            self.path.mkdir()
        return self

    def command(self, command):
        if self.path is None:
            return command
        return [sys.executable, str(Path(__file__).with_name("cgroup_launch.py")), str(self.path), *command]

    def read(self):
        if self.path is None:
            return None
        values = dict(line.split() for line in (self.path / "cpu.stat").read_text().splitlines())
        return {"usage_usec": int(values["usage_usec"]),
                "user_usec": int(values["user_usec"]), "system_usec": int(values["system_usec"]),
                "pids": sorted(int(pid) for pid in (self.path / "cgroup.procs").read_text().split())}

    def __exit__(self, *_):
        if self.path is None:
            return
        # This path was uniquely created for this invocation. Helpers remain
        # charged here even if they created their own session or exited.
        if (self.path / "cgroup.procs").read_text().strip():
            (self.path / "cgroup.kill").write_text("1\n")
        deadline = time.monotonic() + 3
        while True:
            try:
                self.path.rmdir()
                break
            except OSError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.02)


def parse_smaps(text):
    fields = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[2] == "kB":
            fields[parts[0].rstrip(":")] = int(parts[1]) * 1024
    if "Pss" not in fields or "Rss" not in fields:
        raise ValueError("smaps_rollup is missing resident memory counters")
    return {
        "pss_bytes": fields["Pss"],
        "rss_bytes": fields["Rss"],
        "private_bytes": fields.get("Private_Clean", 0) + fields.get("Private_Dirty", 0),
    }


def process_memory(session_id):
    totals = {"pss_bytes": 0, "rss_bytes": 0, "private_bytes": 0}
    pids, unreadable = [], []
    for directory in Path("/proc").iterdir():
        if not directory.name.isdigit():
            continue
        try:
            # comm can contain spaces and parentheses. Fields after its final
            # ')' start with state, ppid, pgrp, session.
            fields = (directory / "stat").read_text().rsplit(")", 1)[1].split()
            if int(fields[3]) != session_id or fields[0] == "Z":
                continue
            pid = int(directory.name)
            try:
                counters = parse_smaps((directory / "smaps_rollup").read_text())
            except PermissionError:
                unreadable.append(pid)
                continue
            pids.append(pid)
            for name in totals:
                totals[name] += counters[name]
        except (FileNotFoundError, ProcessLookupError):
            continue  # Process exited while sampling.
        except PermissionError:
            continue  # Cannot inspect an unrelated process's stat.
    if unreadable or not pids:
        raise RuntimeError(f"incomplete process memory sample: readable={pids}, unreadable={unreadable}")
    return {**totals, "pids": pids}


def read_stdout(stream, events):
    try:
        for line in stream:
            if line.startswith(("HTML_GPUI_SCENE ", "HTML_GPUI_PAINT_DETAIL ")):
                events.put({"event": "diagnostic", "message": line.rstrip()})
                continue
            events.put(json.loads(line))
    except Exception as error:
        events.put({"event": "error", "message": f"invalid host output: {error}"})
    finally:
        events.put({"event": "eof"})


def validate_viewport(event, width, height, scale=None, native=False):
    page = event["page"]
    if page["width"] != width or page["height"] != height or page["scale"] <= 0:
        raise ValueError(f"host viewport differs from the requested viewport: {page}")
    if scale is not None and page["scale"] != scale:
        raise ValueError(f"host scale {page['scale']} differs from requested scale {scale}")
    expected = (round(width * page["scale"]), round(height * page["scale"]))
    if native:
        flags = [event[key] for key in ("wry_initialized", "skrift_initialized") if key in event]
        if not flags or any(flag is not False for flag in flags):
            raise ValueError("native window baseline already initialized HTML content or lacks an initialization flag")
        actual = (event["frame_width"], event["frame_height"])
    else:
        actual = (event["snapshot_width"], event["snapshot_height"])
    if actual != expected:
        raise ValueError(f"snapshot dimensions {actual} differ from viewport pixels {expected}")


def run_host(binary, fixture, args, screenshot=None, content_engine=None, verify_screenshot=None, on_measured=None):
    startup = getattr(args, "startup", False)
    native_window = getattr(args, "native_window", False)
    warm_engine = getattr(args, "warm_engine", False)
    full_document = getattr(args, "full_document", False)
    if startup and native_window:
        raise ValueError("startup and native window measurements are mutually exclusive")
    if verify_screenshot is not None and verify_screenshot.exists():
        raise FileExistsError(f"refusing to overwrite verification image: {verify_screenshot}")
    command = [str(binary), "--fixture", str(fixture), "--width", str(args.width), "--height", str(args.height)]
    if startup:
        if getattr(args, "cpu_cgroup_parent", None) is None:
            raise ValueError("startup measurement requires a CPU cgroup")
        command.append("--startup-document")
    if native_window:
        command.append("--native-window")
    if content_engine is not None:
        if content_engine not in ("skrift", "wry") or not native_window:
            raise ValueError("shared GPUI content requires skrift/wry and a native window baseline")
        command += ["--content", content_engine]
    if screenshot:
        command += ["--screenshot", str(screenshot)]
    environment = os.environ.copy()
    environment.pop("WAYLAND_DISPLAY", None)
    environment.pop("ZED_HEADLESS", None)
    environment.update(GDK_BACKEND="x11", GDK_SCALE=str(args.scale_factor),
                       GDK_DPI_SCALE="1", GPUI_X11_SCALE_FACTOR=str(args.scale_factor))
    # Never inherit the marker request into a normal incremental run.
    environment.pop("SKRIFT_EMBED_STARTUP", None)
    if startup:
        environment["SKRIFT_EMBED_STARTUP"] = "1"
    events = queue.Queue()
    diagnostics = []
    with tempfile.TemporaryFile(mode="w+") as errors, CpuGroup(getattr(args, "cpu_cgroup_parent", None)) as cpu:
        proc = subprocess.Popen(cpu.command(command), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=errors, text=True, bufsize=1, start_new_session=True,
                                env=environment)
        reader = threading.Thread(target=read_stdout, args=(proc.stdout, events), daemon=True)
        reader.start()
        try:
            def ready(phase):
                deadline = time.monotonic() + args.timeout
                while True:
                    event = events.get(timeout=max(0, deadline - time.monotonic()))
                    if event.get("event") != "diagnostic":
                        break
                    diagnostics.append(event["message"])
                if event.get("event") != "ready" or event.get("phase") != phase:
                    raise RuntimeError(f"expected ready/{phase}, received {event}")
                validate_viewport(event, args.width, args.height, args.scale_factor, native=phase == "native")
                return event

            if startup:
                marker = events.get(timeout=args.timeout)
                if marker.get("event") != "exec":
                    raise RuntimeError(f"expected executable launch marker, received {marker}")
                started = marker["monotonic_ns"]
                cpu_baseline = marker["cpu"]
            else:
                baseline_event = ready("native" if native_window else "blank")
                initialized_event = None
                if warm_engine:
                    proc.stdin.write("warm\n")
                    proc.stdin.flush()
                    initialized_event = ready("warm")
                time.sleep(args.settle_ms / 1000)
                baseline = process_memory(proc.pid)
                cpu_baseline = cpu.read()
                started = time.monotonic_ns()
                proc.stdin.write("load-all\n" if full_document else "load\n")
                proc.stdin.flush()
            document = ready("document")
            viewports = [document]
            if full_document:
                content_height = document['page']['content_height']
                if not isinstance(content_height, (int, float)) or not math.isfinite(content_height) or content_height <= 0:
                    raise ValueError('invalid document height')
                offset = float(document['page'].get('scroll_y', 0))
                if abs(offset) > 1:
                    raise ValueError('full document must start at its top')
                while offset + args.height < content_height - 1:
                    requested = min(offset + args.height, content_height - args.height)
                    proc.stdin.write(f"tile {requested}\n")
                    proc.stdin.flush()
                    tile = ready('tile')
                    actual = float(tile['page']['scroll_y'])
                    if not math.isfinite(actual) or actual <= offset or actual > offset + args.height + 1:
                        raise ValueError('tile offsets leave a gap or do not advance')
                    if abs(actual - requested) > 1:
                        raise ValueError('tile scroll position does not match its request')
                    viewports.append(tile)
                    offset = actual
                    if len(viewports) % 100 == 0:
                        print(f"{content_engine or binary.name}: {fixture.name}, {len(viewports)} viewports rendered", flush=True)
            rendered_at = time.monotonic_ns()
            cpu_rendered = cpu.read()
            time.sleep(args.settle_ms / 1000)
            loaded = process_memory(proc.pid)
            result = {
                "host": document,
                "diagnostics": diagnostics,
            }
            if startup:
                result.update(startup_elapsed_ms=(rendered_at - started) / 1_000_000,
                              total_pss_bytes=loaded["pss_bytes"], memory={"loaded": loaded})
            else:
                baseline_name = "initialized" if warm_engine else "native" if native_window else "blank"
                result.update(additional_time_ms=document["document_to_snapshot_ms"],
                              additional_pss_bytes=loaded["pss_bytes"] - baseline["pss_bytes"],
                              memory={baseline_name: baseline, "loaded": loaded},
                              load_command_to_output_ms=(rendered_at - started) / 1_000_000)
                if native_window:
                    result.update(native_window=True, baseline_host=baseline_event)
                if warm_engine:
                    result.update(warm_engine=True, initialized_baseline_host=initialized_event)
                if full_document:
                    result.update(full_document=True, rendered_viewports=viewports,
                                  full_document_covered=True,
                                  additional_time_ms=result['load_command_to_output_ms'])
            if cpu_baseline is not None:
                delta = cpu_rendered["usage_usec"] - cpu_baseline["usage_usec"]
                if delta < 0:
                    raise RuntimeError("cgroup CPU counter decreased")
                result["startup_cpu_ms" if startup else "additional_cpu_ms"] = delta / 1000
                cpu_baseline_name = "exec" if startup else "initialized" if warm_engine else "native" if native_window else "blank"
                result["cpu"] = {cpu_baseline_name: cpu_baseline, "rendered": cpu_rendered,
                                 "scope": "all threads and helper processes in a private cgroup; exited helpers retained"}
                # Use the same controller boundary for both hosts, including
                # stdin delivery and readiness notification. No PNG is encoded.
                if not startup:
                    result["additional_time_ms"] = result["load_command_to_output_ms"]
            if on_measured is not None:
                on_measured(result)
            if verify_screenshot is not None:
                # Capture and encode only after CPU, wall and loaded RAM have
                # been sampled, so validation does not enter those measurements.
                proc.stdin.write(f"capture {verify_screenshot}\n")
                proc.stdin.flush()
                deadline = time.monotonic() + args.timeout
                while True:
                    capture = events.get(timeout=max(0, deadline - time.monotonic()))
                    if capture.get('event') != 'diagnostic':
                        break
                expected = (args.width * args.scale_factor, args.height * args.scale_factor)
                if (capture.get('event') != 'capture' or capture.get('path') != str(verify_screenshot)
                        or (capture.get('width'), capture.get('height')) != expected):
                    raise RuntimeError(f"invalid verification capture: {capture}")
                result['verification_screenshot'] = str(verify_screenshot)
                if full_document:
                    captures = [{'path':str(verify_screenshot), 'scroll_y':viewports[-1]['page']['scroll_y']}]
                    for position, offset in [('first', 0), ('middle', max(0, (content_height - args.height) / 2))]:
                        proc.stdin.write(f"tile {offset}\n")
                        proc.stdin.flush()
                        ready('tile')
                        path = verify_screenshot.with_stem(verify_screenshot.stem + '-' + position)
                        if path.exists():
                            raise FileExistsError(path)
                        proc.stdin.write(f"capture {path}\n")
                        proc.stdin.flush()
                        deadline = time.monotonic() + args.timeout
                        while True:
                            capture = events.get(timeout=max(0, deadline - time.monotonic()))
                            if capture.get('event') != 'diagnostic':
                                break
                            diagnostics.append(capture['message'])
                        if capture.get('event') != 'capture' or capture.get('path') != str(path):
                            raise RuntimeError(f'invalid verification capture: {capture}')
                        captures.append({'path':str(path),'scroll_y':offset})
                    result['verification_viewports'] = captures
            proc.stdin.write("quit\n")
            proc.stdin.flush()
            proc.wait(timeout=5)
            if proc.returncode:
                raise RuntimeError(f"host exited with code {proc.returncode}")
            return result
        except Exception as error:
            errors.seek(0)
            log = errors.read()
            excerpt = log if len(log) <= 10000 else log[:6000] + "\n...\n" + log[-4000:]
            raise RuntimeError(f"{binary.name}, {fixture.name}: {error}\n{excerpt}") from error
        finally:
            # Webview helpers may outlive their parent. Only terminate processes
            # in the private session created by this invocation.
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=3)
            proc.stdin.close()
            proc.stdout.close()
            reader.join(timeout=1)


def summarize(rows):
    summary = []
    for row in rows:
        item = {"label": row["label"], "fixture": row["fixture"], "samples": 1}
        if "startup_cpu_ms" in row:
            item.update(startup_cpu_ms=row["startup_cpu_ms"], total_pss_bytes=row["total_pss_bytes"])
        else:
            timing = "additional_cpu_ms" if "additional_cpu_ms" in row else "additional_time_ms"
            baseline = "initialized" if row.get("warm_engine") else "native" if row.get("native_window") else "blank"
            item.update({timing: row[timing],
                         "memory_before_pss_bytes": row["memory"][baseline]["pss_bytes"],
                         "memory_after_pss_bytes": row["memory"]["loaded"]["pss_bytes"]})
        summary.append(item)
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", action="append", required=True, metavar="LABEL=BINARY")
    parser.add_argument("--fixtures", nargs="+", type=Path)
    parser.add_argument("--width", type=int, default=800)
    parser.add_argument("--height", type=int, default=600)
    parser.add_argument("--scale-factor", type=int, choices=range(1, 5), default=1)
    parser.add_argument("--settle-ms", type=int, default=500)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--screenshots", type=Path)
    parser.add_argument("--startup", action="store_true",
                        help="render the fixture directly at startup; report total CPU and loaded PSS")
    parser.add_argument("--native-window", action="store_true",
                        help="measure HTML content initialization and rendering over a plain native toolkit window")
    parser.add_argument("--shared-gpui", action="store_true",
                        help="select skrift/wry by host label in one GPUI executable; requires --native-window")
    parser.add_argument("--warm-engine", action="store_true",
                        help="initialize and render an empty document before measuring; requires workload host")
    parser.add_argument("--full-document", action="store_true",
                        help="render successive viewport snapshots covering the entire document; requires workload host")
    parser.add_argument("--verify-screenshots", type=Path,
                        help="shared GPUI host only: save verification PNGs after all timing and memory samples")
    parser.add_argument("--cpu-cgroup-parent", type=Path,
                        help="writable delegated cgroup v2 parent; enables total host and helper CPU accounting")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error(f"refusing to overwrite existing results: {args.output}")
    if args.startup and args.cpu_cgroup_parent is None:
        parser.error("--startup requires --cpu-cgroup-parent")
    if args.startup and args.native_window:
        parser.error("--startup and --native-window are mutually exclusive")
    if args.shared_gpui and not args.native_window:
        parser.error("--shared-gpui requires --native-window")
    if (args.warm_engine or args.full_document) and not args.shared_gpui:
        parser.error("--warm-engine and --full-document require --shared-gpui")
    if args.verify_screenshots and not args.shared_gpui:
        parser.error("--verify-screenshots requires --shared-gpui")
    if args.cpu_cgroup_parent is not None:
        args.cpu_cgroup_parent = args.cpu_cgroup_parent.resolve(strict=True)
        if args.screenshots:
            parser.error("CPU comparisons exclude PNG encoding; omit --screenshots")
    if platform.system() != "Linux":
        parser.error("this first harness uses Linux process memory accounting")
    if not os.environ.get("DISPLAY"):
        parser.error("an X11 display is required for matching native window backends")
    if not 1 <= args.width <= 8192 or not 1 <= args.height <= 8192 or args.settle_ms < 0 or args.timeout <= 0:
        parser.error("invalid viewport, settling interval or timeout")
    hosts = []
    for value in args.host:
        label, sep, binary = value.partition("=")
        if not sep or not label:
            parser.error("--host must be LABEL=BINARY")
        hosts.append((label, Path(binary).resolve(strict=True)))
    if args.shared_gpui:
        if any(label not in ("skrift", "wry") for label, _ in hosts) or len({p for _, p in hosts}) != 1:
            parser.error("--shared-gpui requires skrift/wry labels pointing to the same executable")
    fixtures = [p.resolve(strict=True) for p in (args.fixtures or sorted(Path(__file__).with_name("fixtures").glob("*.html")))]
    if not fixtures:
        parser.error("no fixtures found")
    report = {
        "schema": 3 if args.cpu_cgroup_parent else 2, "complete": False,
        "environment": {"platform": platform.platform(), "machine": platform.machine(),
                                      "display": os.environ.get("DISPLAY"), "wayland_display": os.environ.get("WAYLAND_DISPLAY"),
                                      "cpu_count": os.cpu_count(), "load_average_before": os.getloadavg()},
        "viewport": [args.width, args.height], "settle_ms": args.settle_ms,
        "window_backend": "x11", "scale_factor": args.scale_factor,
        "viewport_pixels": [args.width * args.scale_factor, args.height * args.scale_factor],
        "samples_per_fixture": 1,
        "baseline": "settled empty window with empty document",
        "timing": "load command received through viewport snapshot; excludes window startup",
        "memory": "total process-session PSS before and after loading, including helper processes",
        "endpoint": "viewport snapshot including readback, excluding PNG encoding",
        "pixel_equivalence_verified": False,
        "hosts": [{"label": label, "binary": str(binary), "size_bytes": binary.stat().st_size,
                   "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()} for label, binary in hosts],
        "fixtures": [{"path": str(p), "sha256": hashlib.sha256(p.read_bytes()).hexdigest()} for p in fixtures],
        "runs": [], "summary": [],
    }
    if args.cpu_cgroup_parent:
        report["timing"] = "controller load command through rendered snapshot readiness, including readback and IPC; excludes startup and PNG encoding"
        report["cpu"] = "private cgroup v2 usage increase over the same load interval; all host threads and helper processes, including exited helpers; no tracing"
        report["cpu_cgroup_parent"] = str(args.cpu_cgroup_parent)
    if args.startup:
        report.update(schema=4, baseline="none; fixture loaded directly at executable startup",
                      timing="immediately before exec through first document snapshot readiness, including dynamic loading, window and renderer initialization, readback and notification; no blank render or settling interval before endpoint",
                      cpu="private cgroup v2 usage increase from immediately before exec through snapshot readiness; all host threads and helper processes, including exited helpers",
                      memory="total loaded process session PSS after settling; includes helper processes",
                      startup=True)
    if args.native_window:
        report.update(schema=5, baseline="settled plain native toolkit window after its first frame; HTML content initialization deferred",
                      timing="controller load command through first document snapshot readiness; includes HTML content initialization, document processing, readback and notification; excludes native toolkit window startup",
                      memory="total process-session PSS before loading (settled native window) and after loading, including helpers",
                      native_window=True)
    if args.shared_gpui:
        report.update(shared_gpui=True,
                      baseline="same GPUI executable and painted empty window; neither content engine nor GTK initialized",
                      timing="controller load command through document snapshot readiness; includes engine-specific initialization and integration over the same GPUI host")
    if args.warm_engine:
        report.update(schema=6, warm_engine=True,
                      baseline="settled GPUI host after the selected content engine renders an empty document",
                      memory="total process-session PSS before loading (settled initialized engine) and after loading, including helpers",
                      timing="controller load command through snapshot readiness; selected content engine already initialized; document never loaded before measurement")
    if args.full_document:
        report.update(schema=6, full_document=True,
                      endpoint="final viewport snapshot/readback after sequentially rendering every vertical viewport; no PNG encoding",
                      timing="controller load-all command through final snapshot readiness for all content; includes content initialization unless --warm-engine is used")
    if args.verify_screenshots:
        args.verify_screenshots = args.verify_screenshots.resolve()
        args.verify_screenshots.mkdir(parents=True, exist_ok=True)
        report['verification_screenshots'] = 'captured and PNG encoded after CPU, wall and loaded memory samples'
    if os.environ.get("FONTCONFIG_FILE"):
        fontconfig = Path(os.environ["FONTCONFIG_FILE"]).resolve(strict=True)
        report["environment"]["fontconfig"] = {"path": str(fontconfig), "sha256": hashlib.sha256(fontconfig.read_bytes()).hexdigest()}
    args.output.parent.mkdir(parents=True, exist_ok=True)

    def save():
        report["summary"] = summarize(report["runs"])
        temporary = args.output.with_suffix(args.output.suffix + ".tmp")
        temporary.write_text(json.dumps(report, indent=2) + "\n")
        temporary.replace(args.output)

    save()
    rng = random.Random(0)
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    for fixture in fixtures:
        order = hosts.copy()
        rng.shuffle(order)
        for label, binary in order:
            image = args.screenshots / f"{label}-{fixture.stem}.png" if args.screenshots else None
            def checkpoint(measured):
                measured.update(label=label, fixture=str(fixture))
                measured['verification_pending'] = args.verify_screenshots is not None
                report['runs'].append(measured)
                save()

            row = run_host(binary, fixture, args, screenshot=image,
                           content_engine=label if args.shared_gpui else None,
                           verify_screenshot=args.verify_screenshots / f'{label}-{fixture.stem}.png'
                           if args.verify_screenshots else None, on_measured=checkpoint)
            row.pop('verification_pending', None)
            save()
            print(f"{label}: {fixture.name} 1/1", flush=True)
    report["environment"]["load_average_after"] = os.getloadavg()
    report["complete"] = True
    save()
    print(json.dumps(report["summary"], indent=2))


if __name__ == "__main__":
    main()
