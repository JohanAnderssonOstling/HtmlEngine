#!/usr/bin/env python3
r"""Measure HTML embedding CPU and total process-session RAM on Linux.

Build the shared host with:
  cargo build --release --manifest-path tools/embed-bench/gpui/Cargo.toml \
    --bin embed-bench-gpui-culled --features wry-in-gpui

Run with --native-window --shared-gpui and two --host arguments, labelled
skrift and wry, pointing to that executable. Use --warm-engine to initialize
each engine with an empty page before measurement. Add --full-document to
render successive viewports through the document's end.
Use --simulate-scroll instead of --full-document to traverse the document inside
each host with two frame callbacks per viewport, one completion reply, and no
measured snapshots/readback. Loading and document preparation remain measured.
Use --document-surface with a --height covering the laid-out document to capture
one complete document-sized surface, including GPU readback, without scrolling.
Both engines use the requested viewport height; viewport-relative CSS may change.
Use --document-frames with that same document-sized viewport to omit snapshots
and readback. It ends after two frame callbacks in each engine; GPU completion
and display presentation are not guaranteed, and frame pacing enters wall time.
Add --cpu-window-ms 100 250 500 1000 to sample cumulative CPU at fixed times
after loading, independently of frame callbacks. The final interval is the
measurement endpoint; compare checkpoints to check for unfinished CPU work.
Use --viewport-frames for the first viewport with the same no-readback CPU
windows. Without --warm-engine, this includes engine initialization after an
empty native window is ready.
Add --cpus 8 10 12 14 to restrict the host and inherited helper/thread affinity
before startup. This is CPU affinity, not exclusive core reservation. Both
engines use the same mask. Startup affinity resets are corrected before timing,
and live thread masks are checked after measurement. A CPU cgroup is required
so detached helpers are checked too.
Use fixed_frequency.py and its SKRIFT_EMBED_CPU_CGROUP_PARENT for a kernel
cpuset ceiling that also prevents affinity resets during rendering.
Add --phase-timings to record host intervals separately: source reading, engine
setup, document/scroll readiness, GPUI drawing (Skrift), and snapshot API work.
With a CPU cgroup, each interval also counts all host/helper CPU. Snapshot API
work includes rendering and readback; neither backend exposes pure readback here.

One document load per host and fixture. CPU counts every thread and helper in
a private cgroup when --cpu-cgroup-parent is supplied, including exited helpers.
RAM is total process-session PSS before and after loading, including helpers;
shared resident pages count proportionally. GPU VRAM and peak memory are excluded.

The default endpoint is a completed viewport snapshot including readback;
simulated scrolling ends after frame callbacks without a GPU completion fence.
Verification PNG encoding occurs after CPU and RAM sampling. Measured results are saved before
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


def parse_cpu_set(value):
    cpus = set()
    for entry in value.strip().split(","):
        if entry:
            first, separator, last = entry.partition("-")
            cpus.update(range(int(first), int(last) + 1) if separator else [int(first)])
    return cpus


class CpuGroup:
    """A private cgroup retains CPU usage even when helper processes exit."""

    def __init__(self, parent):
        self.path = None if parent is None else parent / f"skrift-embed-bench-{uuid.uuid4().hex}"

    def __enter__(self):
        if self.path is not None:
            self.path.mkdir()
        return self

    def command(self, command, cpus=None):
        if self.path is None:
            return command
        affinity = [] if cpus is None else ["--cpus", ",".join(map(str, cpus))]
        return [sys.executable, str(Path(__file__).with_name("cgroup_launch.py")), str(self.path), *affinity, *command]

    def affinity(self, cpus, assign=False):
        if cpus is None:
            return None
        requested = set(cpus)
        effective = None
        cpuset_file = self.path / "cpuset.cpus.effective"
        if cpuset_file.exists():
            effective = parse_cpu_set(cpuset_file.read_text())
        expected = os.environ.get("SKRIFT_EMBED_CPUSET_CPUS")
        if expected is not None and (effective != requested or
                                      requested != parse_cpu_set(expected)):
            raise RuntimeError(f"kernel CPU-set limit differs from requested CPUs: {effective}")
        observed, checked, corrected = set(), 0, []
        pids = self.read()["pids"]
        for pid in pids:
            try:
                tasks = list((Path("/proc") / str(pid) / "task").iterdir())
            except (FileNotFoundError, ProcessLookupError):
                continue
            for task in tasks:
                try:
                    actual = os.sched_getaffinity(int(task.name))
                    if assign and not actual <= requested:
                        previous = sorted(actual)
                        name = (task / "comm").read_text().strip()
                        os.sched_setaffinity(int(task.name), actual & requested or requested)
                        actual = os.sched_getaffinity(int(task.name))
                        corrected.append({"thread": int(task.name), "name": name, "previous_cpus": previous})
                except (FileNotFoundError, ProcessLookupError):
                    continue
                if not actual or not actual <= requested:
                    raise RuntimeError(f"thread {task.name} escaped requested CPU affinity: {sorted(actual)}")
                checked += 1
                observed.add(tuple(sorted(actual)))
        if not checked:
            raise RuntimeError("no live benchmark threads available for CPU affinity verification")
        return {"requested_cpus": sorted(requested), "checked_threads": checked,
                "kernel_cpuset_enforced": effective == requested,
                "cpuset_effective_cpus": None if effective is None else sorted(effective),
                "observed_cpu_sets": [list(mask) for mask in sorted(observed)], "pids": pids,
                "corrected_startup_threads": corrected}

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


def cpu_frequency(cpus):
    if cpus is None:
        return None
    expected = os.environ.get("SKRIFT_EMBED_FIXED_CPU_KHZ")
    result = {}
    for cpu in cpus:
        policy = Path(f"/sys/devices/system/cpu/cpu{cpu}/cpufreq")
        values = {}
        for name in ("scaling_driver", "scaling_governor", "scaling_min_freq",
                     "scaling_max_freq", "scaling_cur_freq", "cpuinfo_avg_freq"):
            try:
                values[name] = (policy / name).read_text().strip()
            except OSError:
                values[name] = None
        if expected and any(values[name] != expected for name in ("scaling_min_freq", "scaling_max_freq")):
            raise RuntimeError(f"CPU {cpu} frequency limits changed: expected {expected} kHz, read {values}")
        result[str(cpu)] = values
    return {"expected_khz": expected, "policies": result,
            "scope": "policy limits and reported clocks at sampling time; hardware can throttle below requested limits"}


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
    elif event.get("simulated_scroll") is True or event.get("document_frames") is True:
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
    simulate_scroll = args.simulate_scroll
    document_surface = args.document_surface
    viewport_frames = getattr(args, "viewport_frames", False)
    document_frames = args.document_frames or viewport_frames
    cpu_windows = getattr(args, "cpu_window_ms", None)
    cpus = getattr(args, "cpus", None)
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
    if args.phase_timings:
        command.append("--phase-timings")
    if simulate_scroll:
        command.append("--simulate-scroll")
    if document_surface:
        command.append("--document-surface")
    if document_frames:
        command.append("--document-frames")
    if screenshot:
        command += ["--screenshot", str(screenshot)]
    environment = os.environ.copy()
    environment.pop("WAYLAND_DISPLAY", None)
    environment.pop("ZED_HEADLESS", None)
    environment.update(GDK_BACKEND="x11", GDK_SCALE=str(args.scale_factor),
                       GDK_DPI_SCALE="1", GPUI_X11_SCALE_FACTOR=str(args.scale_factor))
    # Never inherit the marker request into a normal incremental run.
    environment.pop("SKRIFT_EMBED_STARTUP", None)
    environment.pop("SKRIFT_EMBED_CPU_STAT", None)
    if startup:
        environment["SKRIFT_EMBED_STARTUP"] = "1"
    events = queue.Queue()
    diagnostics = []
    with tempfile.TemporaryFile(mode="w+") as errors, CpuGroup(getattr(args, "cpu_cgroup_parent", None)) as cpu:
        proc = subprocess.Popen(cpu.command(command, cpus), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
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
                if phase == "document" and bool(event.get("simulated_scroll")) != simulate_scroll:
                    raise RuntimeError("host completed the wrong rendering workload")
                if phase == "document" and bool(event.get("document_surface")) != document_surface:
                    raise RuntimeError("host completed the wrong surface workload")
                if phase in ("warm", "document"):
                    if bool(event.get("document_frames")) != document_frames:
                        raise RuntimeError("host completed the wrong frame workload")
                    if document_frames and (event.get("frame_callbacks") != 2
                            or event.get("readback") is not False
                            or event.get("gpu_completion_fenced") is not False
                            or "snapshot_width" in event or "snapshot_height" in event):
                        raise RuntimeError("invalid no-readback frame endpoint")
                if args.phase_timings and phase != "native":
                    phases = event.get("phase_timings")
                    expected = {"scroll_ready"} if phase == "tile" else {"source_read", "engine_setup", "document_ready"}
                    if document_frames and phase in ("warm", "document"):
                        if content_engine == "wry":
                            expected.remove("document_ready")
                            expected.add("document_and_frames")
                        else:
                            expected.add("frame_callbacks")
                    elif simulate_scroll and phase == "document":
                        if content_engine == "wry":
                            expected.remove("document_ready")
                            expected.add("document_and_scroll")
                        else:
                            expected.add("scroll_frames")
                    else:
                        expected.add("snapshot")
                        if content_engine == "skrift":
                            expected.add("host_draw")
                    if not isinstance(phases, dict) or set(phases) != expected:
                        raise RuntimeError(f"missing or incompatible host phase timings: {event}")
                    for name, values in phases.items():
                        counters = {"wall_ms", "cpu_ms"} if args.cpu_cgroup_parent else {"wall_ms"}
                        if not isinstance(values, dict) or set(values) != counters:
                            raise RuntimeError(f"invalid phase counters: {name}={values}")
                        if any(not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0
                               for value in values.values()):
                            raise RuntimeError(f"invalid phase timings: {name}={values}")
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
                cpu_before_settle = cpu.read() if cpu_windows else None
                time.sleep(args.settle_ms / 1000)
                baseline = process_memory(proc.pid)
                baseline_affinity = cpu.affinity(cpus, assign=True)
                baseline_frequency = cpu_frequency(cpus)
                cpu_baseline = cpu.read()
                started = time.monotonic_ns()
                proc.stdin.write("load\n" if viewport_frames else "frame-all\n" if document_frames else "render-all\n" if document_surface else "scroll-all\n" if simulate_scroll else "load-all\n" if full_document else "load\n")
                proc.stdin.flush()
            window_samples = []
            if cpu_windows:
                for window_ms in cpu_windows:
                    deadline = started + round(window_ms * 1_000_000)
                    while (remaining := deadline - time.monotonic_ns()) > 0:
                        time.sleep(min(remaining / 1_000_000_000, 0.5))
                    cpu_rendered = cpu.read()
                    rendered_at = time.monotonic_ns()
                    window_samples.append({
                        "requested_ms": window_ms,
                        "wall_ms": (rendered_at - started) / 1_000_000,
                        "cpu_ms": (cpu_rendered["usage_usec"] - cpu_baseline["usage_usec"]) / 1000,
                        "cpu": cpu_rendered,
                    })
            document = ready("document")
            if cpu_windows and document["document_to_render_ms"] > window_samples[-1]["wall_ms"]:
                raise ValueError("document readiness exceeded the CPU measurement window; increase --cpu-window-ms")
            if not full_document and not cpu_windows:
                rendered_at = time.monotonic_ns()
                cpu_rendered = cpu.read()
            viewports = [document]
            if viewport_frames:
                offset = document['page'].get('scroll_y')
                if not isinstance(offset, (int, float)) or not math.isfinite(offset) or abs(offset) > 1:
                    raise ValueError('first-viewport rendering must stay at the document top')
            if document_surface or args.document_frames:
                content_height = document['page']['content_height']
                offset = document['page'].get('scroll_y')
                if (not isinstance(content_height, (int, float)) or not math.isfinite(content_height)
                        or content_height <= 0 or math.ceil(content_height) > args.height
                        or not isinstance(offset, (int, float)) or not math.isfinite(offset) or abs(offset) > 1):
                    raise ValueError('document-sized surface does not cover the whole document; increase --height')
                content_width = document['page'].get('content_width', args.width)
                if not isinstance(content_width, (int, float)) or not math.isfinite(content_width) or content_width > args.width:
                    raise ValueError('document-sized surface has horizontal overflow; increase --width')
            if simulate_scroll:
                content_height = document['page']['content_height']
                offset = document['page']['scroll_y']
                scroll = document.get('scroll', {})
                if (not isinstance(content_height, (int, float)) or not math.isfinite(content_height)
                        or content_height <= 0 or not isinstance(offset, (int, float))
                        or not math.isfinite(offset)
                        or abs(offset - max(0, content_height - args.height)) > 1):
                    raise ValueError('simulated scroll did not reach the document bottom')
                if (scroll.get('start_y') != 0 or scroll.get('step_px') != args.height
                        or scroll.get('frame_callbacks_per_viewport') != 2
                        or type(scroll.get('viewports')) is not int
                        or scroll['viewports'] != max(1, math.ceil((content_height - 1) / args.height))):
                    raise ValueError(f'invalid simulated scroll path: {scroll}')
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
            cpu_settled = cpu.read() if document_frames else None
            loaded = process_memory(proc.pid)
            result = {
                "host": document,
                "diagnostics": diagnostics,
            }
            if cpus is not None:
                result["cpu_affinity"] = {"loaded": cpu.affinity(cpus)}
                result["cpu_frequency"] = {"loaded": cpu_frequency(cpus)}
                if not startup:
                    result["cpu_affinity"]["baseline"] = baseline_affinity
                    result["cpu_frequency"]["baseline"] = baseline_frequency
            if startup:
                result.update(startup_elapsed_ms=(rendered_at - started) / 1_000_000,
                              total_pss_bytes=loaded["pss_bytes"], memory={"loaded": loaded})
            else:
                baseline_name = "initialized" if warm_engine else "native" if native_window else "blank"
                result.update(additional_time_ms=document["document_to_render_ms" if simulate_scroll or document_frames else "document_to_snapshot_ms"],
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
                if simulate_scroll:
                    result.update(simulate_scroll=True, scroll=document['scroll'],
                                  scroll_path_covered=True,
                                  additional_time_ms=result['load_command_to_output_ms'])
                if document_surface:
                    result.update(document_surface=True, full_document_covered=True,
                                  surface_pixels=[document['snapshot_width'], document['snapshot_height']],
                                  additional_time_ms=result['load_command_to_output_ms'])
                if document_frames:
                    result.update(document_frames=not viewport_frames,
                                  viewport_frames=viewport_frames,
                                  document_geometry_covered=not viewport_frames,
                                  viewport_pixels=[document['frame_width'], document['frame_height']],
                                  readback=False, gpu_completion_fenced=False,
                                  additional_time_ms=result['load_command_to_output_ms'])
            if cpu_baseline is not None:
                delta = cpu_rendered["usage_usec"] - cpu_baseline["usage_usec"]
                if delta < 0:
                    raise RuntimeError("cgroup CPU counter decreased")
                result["startup_cpu_ms" if startup else "additional_cpu_ms"] = delta / 1000
                cpu_baseline_name = "exec" if startup else "initialized" if warm_engine else "native" if native_window else "blank"
                result["cpu"] = {cpu_baseline_name: cpu_baseline, "rendered": cpu_rendered,
                                 "scope": "all threads and helper processes in a private cgroup; exited helpers retained"}
                if document_frames:
                    result["cpu"]["settled"] = cpu_settled
                    result["post_endpoint_cpu_ms"] = (cpu_settled["usage_usec"] - cpu_rendered["usage_usec"]) / 1000
                    result["cpu_through_settle_ms"] = (cpu_settled["usage_usec"] - cpu_baseline["usage_usec"]) / 1000
                # Use the same controller boundary for both hosts, including
                # stdin delivery and readiness notification. No PNG is encoded.
                if not startup:
                    result["additional_time_ms"] = result["load_command_to_output_ms"]
            if args.phase_timings:
                result["additional_time_ms"] = result["load_command_to_output_ms"]
                totals = {}
                for viewport in viewports:
                    for name, values in viewport["phase_timings"].items():
                        accumulated = totals.setdefault(name, {counter: 0.0 for counter in values})
                        for counter, value in values.items():
                            accumulated[counter] += value
                measured = {"wall_ms": result["load_command_to_output_ms"]}
                if cpu_baseline is not None:
                    measured["cpu_ms"] = result["additional_cpu_ms"]
                # This residual includes IPC, event notification, scheduling,
                # controller gaps and host/helper activity outside host phases.
                # The controller itself is outside the measured CPU cgroup.
                totals["outside_host_phases"] = {
                    counter: total - sum(phase[counter] for phase in totals.values())
                    for counter, total in measured.items()
                }
                if any(value < -0.001 for value in totals["outside_host_phases"].values()):
                    raise RuntimeError(f"host phase intervals exceed controller interval: {totals}")
                result["phase_timings"] = totals
            if cpu_windows:
                result.update(cpu_windows=window_samples,
                              cpu_window_ms=cpu_windows[-1],
                              measurement_wall_ms=window_samples[-1]["wall_ms"],
                              frame_callback_elapsed_ms=document["document_to_render_ms"],
                              warmup_settling_cpu_ms=(cpu_baseline["usage_usec"] - cpu_before_settle["usage_usec"]) / 1000,
                              endpoint="fixed CPU measurement interval; no snapshot/readback or GPU completion signal")
                result.pop("load_command_to_output_ms")
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
                if full_document or simulate_scroll:
                    captures = [{'path':str(verify_screenshot), 'scroll_y':document['page']['scroll_y']
                                 if simulate_scroll else viewports[-1]['page']['scroll_y']}]
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
        if "phase_timings" in row:
            item["phase_timings"] = row["phase_timings"]
        if row.get("simulate_scroll"):
            item.update(simulate_scroll=True, scroll=row["scroll"],
                        load_command_to_output_ms=row["load_command_to_output_ms"])
        if row.get("document_surface"):
            item.update(document_surface=True, surface_pixels=row["surface_pixels"],
                        load_command_to_output_ms=row["load_command_to_output_ms"])
        if row.get("document_frames") or row.get("viewport_frames") or "cpu_windows" in row:
            item.update(document_frames=bool(row.get("document_frames")),
                        viewport_frames=bool(row.get("viewport_frames")),
                        readback=False, gpu_completion_fenced=False)
            if "viewport_pixels" in row:
                item["viewport_pixels"] = row["viewport_pixels"]
            if "cpu_windows" in row:
                item.update(cpu_windows=[{key: sample[key] for key in ("requested_ms", "wall_ms", "cpu_ms")}
                                         for sample in row["cpu_windows"]],
                            measurement_wall_ms=row["measurement_wall_ms"],
                            cpu_window_ms=row["cpu_window_ms"])
            else:
                item["load_command_to_output_ms"] = row["load_command_to_output_ms"]
            if "post_endpoint_cpu_ms" in row:
                item["post_endpoint_cpu_ms"] = row["post_endpoint_cpu_ms"]
                item["cpu_through_settle_ms"] = row["cpu_through_settle_ms"]
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
    parser.add_argument("--cpu-window-ms", type=int, nargs="+", metavar="MS",
                        help="sample cumulative CPU at fixed times after loading; final checkpoint ends measurement; requires --document-frames or --viewport-frames and --cpu-cgroup-parent")
    parser.add_argument("--cpus", type=int, nargs="+", metavar="CPU",
                        help="restrict host/helpers to these logical CPUs before startup; affinity is inherited and verified; requires --cpu-cgroup-parent")
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
    traversal = parser.add_mutually_exclusive_group()
    traversal.add_argument("--full-document", action="store_true",
                        help="render successive viewport snapshots covering the entire document; requires workload host")
    traversal.add_argument("--simulate-scroll", action="store_true",
                           help="load and scroll through all viewports inside each host using frame callbacks; no measured snapshots; requires --shared-gpui")
    traversal.add_argument("--document-surface", action="store_true",
                           help="capture one document-sized surface including readback; set --height to cover the entire document; requires --shared-gpui")
    traversal.add_argument("--document-frames", action="store_true",
                           help="document-sized viewport after two frame callbacks; no snapshots/readback or GPU completion fence; requires --shared-gpui")
    traversal.add_argument("--viewport-frames", action="store_true",
                           help="first viewport after two frame callbacks; no snapshot/readback; includes engine initialization unless --warm-engine; requires --shared-gpui")
    parser.add_argument("--phase-timings", action="store_true",
                        help="separate host phases and snapshot API work; requires --shared-gpui; adds cgroup counter reads when CPU accounting is enabled")
    parser.add_argument("--verify-screenshots", type=Path,
                        help="shared GPUI host only: save verification PNGs after all timing and memory samples")
    parser.add_argument("--cpu-cgroup-parent", type=Path,
                        default=os.environ.get("SKRIFT_EMBED_CPU_CGROUP_PARENT"),
                        help="writable delegated cgroup v2 parent; defaults to SKRIFT_EMBED_CPU_CGROUP_PARENT; enables total host and helper CPU accounting")
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
    if args.simulate_scroll and not args.shared_gpui:
        parser.error("--simulate-scroll requires --shared-gpui")
    if args.simulate_scroll and args.screenshots:
        parser.error("simulated scrolling excludes measured snapshots; use --verify-screenshots")
    if args.document_surface and not args.shared_gpui:
        parser.error("--document-surface requires --shared-gpui")
    if args.document_surface and args.screenshots:
        parser.error("document surface timing excludes PNG encoding; use --verify-screenshots")
    if (args.document_frames or args.viewport_frames) and not args.shared_gpui:
        parser.error("frame workloads require --shared-gpui")
    if (args.document_frames or args.viewport_frames) and args.screenshots:
        parser.error("frame timing excludes snapshots; use --verify-screenshots")
    if args.cpu_window_ms is not None:
        if not (args.document_frames or args.viewport_frames) or args.cpu_cgroup_parent is None:
            parser.error("--cpu-window-ms requires --document-frames or --viewport-frames and --cpu-cgroup-parent")
        if (args.cpu_window_ms != sorted(set(args.cpu_window_ms))
                or args.cpu_window_ms[0] <= 0 or args.cpu_window_ms[-1] >= args.timeout * 1000):
            parser.error("CPU checkpoints must be positive, strictly increasing and below --timeout")
    if args.phase_timings and not args.shared_gpui:
        parser.error("--phase-timings requires --shared-gpui")
    if args.phase_timings and args.screenshots:
        parser.error("phase timings exclude PNG encoding; use --verify-screenshots")
    if args.verify_screenshots and not args.shared_gpui:
        parser.error("--verify-screenshots requires --shared-gpui")
    if args.cpu_cgroup_parent is not None:
        args.cpu_cgroup_parent = args.cpu_cgroup_parent.resolve(strict=True)
        if args.screenshots:
            parser.error("CPU comparisons exclude PNG encoding; omit --screenshots")
    if platform.system() != "Linux":
        parser.error("this first harness uses Linux process memory accounting")
    if args.cpus is not None:
        if args.cpu_cgroup_parent is None:
            parser.error("--cpus requires --cpu-cgroup-parent to verify helper affinity")
        allowed = os.sched_getaffinity(0)
        if len(args.cpus) != len(set(args.cpus)) or not set(args.cpus) <= allowed:
            parser.error(f"--cpus must contain distinct available CPU IDs from {sorted(allowed)}")
        args.cpus.sort()
    if os.environ.get("SKRIFT_EMBED_FIXED_CPU_KHZ") and args.cpus is None:
        parser.error("fixed-frequency benchmark runs require --cpus for frequency limit verification")
    expected_cpuset = os.environ.get("SKRIFT_EMBED_CPUSET_CPUS")
    if expected_cpuset is not None:
        if args.cpus is None or set(args.cpus) != parse_cpu_set(expected_cpuset):
            parser.error("--cpus must match the fixed-frequency helper's kernel CPU-set limit")
        cpuset_file = args.cpu_cgroup_parent / "cpuset.cpus.effective"
        if not cpuset_file.exists() or parse_cpu_set(cpuset_file.read_text()) != set(args.cpus):
            parser.error("--cpu-cgroup-parent must use the helper's constrained benchmark group")
    if not os.environ.get("DISPLAY"):
        parser.error("an X11 display is required for matching native window backends")
    limit = 16384 if args.document_surface or args.document_frames else 8192
    if not 1 <= args.width <= limit or not 1 <= args.height <= limit or args.settle_ms < 0 or args.timeout <= 0:
        parser.error("invalid viewport, settling interval or timeout")
    if (args.document_surface or args.document_frames) and max(args.width, args.height) * args.scale_factor > limit:
        parser.error("document surface exceeds the 16384 physical-pixel limit; reduce dimensions or scale")
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
    if args.cpus is not None:
        report["environment"]["cpu_affinity"] = {
            "requested_cpus": args.cpus,
            "controller_allowed_cpus": sorted(os.sched_getaffinity(0)),
            "scope": "host/helper affinity set before exec; startup resets corrected at baseline before timing; live thread masks checked after loading; kernel cpuset ceiling also verified when configured",
            "exclusive_cores": False,
            "topology": [{"cpu": cpu, **{name: (Path('/sys/devices/system/cpu') / f'cpu{cpu}' / 'topology' / name).read_text().strip()
                                           for name in ('physical_package_id', 'core_id', 'thread_siblings_list')}}
                         for cpu in args.cpus],
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
    if args.phase_timings:
        report.update(schema=7, phase_timings={
            "scope": "sequential host wall intervals; optional private-cgroup CPU deltas across all threads and helpers; summed over measured viewports",
            "source_read": "fixture reading and syntax selection in the host",
            "engine_setup": "content host setup; cold includes engine construction and Wry GTK bridge initialization; warm includes reuse/configuration",
            "document_ready": "Skrift document load, layout and paint-command preparation; Wry navigation through fonts/images ready and forced layout metadata received over WebKit IPC; neither guarantees presentation",
            "scroll_ready": "Skrift scroll state update; Wry scroll plus two animation callbacks and WebKit IPC",
            "scroll_frames": "host-driven traversal from top to bottom with two frame callbacks per viewport, including normal drawing and frame pacing; no per-viewport controller IPC or snapshots; GPU completion is not fenced",
            "document_and_scroll": "Wry simulated scrolling: navigation, document readiness and the page-driven traversal through its final reply; combined because the page starts scrolling directly without a readiness round trip to the host",
            "document_and_frames": "Wry navigation, fonts/images, forced layout and two page animation callbacks through one readiness reply; no snapshot or GPU completion fence",
            "frame_callbacks": "Skrift normal drawing opportunities through two GPUI frame callbacks; includes frame pacing; no snapshot or GPU completion fence",
            "host_draw": "Skrift GPUI draw and scene construction/submission before snapshot; WebKit drawing is internal to its readiness/snapshot intervals",
            "snapshot": "complete snapshot API call including rendering, synchronization, transfer/readback and callback dispatch; excludes PNG encoding; pure readback is not isolated",
            "outside_host_phases": "controller interval minus summed host intervals; includes command/reply IPC, notification, scheduling and bookkeeping; CPU counts only host/helper activity, not the controller",
            "limitations": "interval accounting, not exclusive attribution; background work can overlap phases; instrumentation adds counter reads; existing published measurements are unchanged",
        })
    if args.simulate_scroll:
        report.update(schema=9, simulate_scroll=True,
                      endpoint="internal viewport scroll traversal after two frame callbacks per viewport; no measured snapshot/readback; GPU completion is not fenced",
                      timing="controller scroll-all command through one completion reply; includes loading, document preparation, normal rendering, frame pacing and engine initialization unless warm; no per-viewport controller IPC",
                      scroll="top-to-bottom traversal in viewport-height steps; Wry starts directly at page readiness using one reused animation callback and cached dimensions, validates final geometry and sends only completion; both hosts validate progress and coverage; frame callbacks give draw opportunities, not GPU completion or display scanout guarantees")
    if args.document_surface:
        report.update(schema=10, document_surface=True,
                      endpoint="one completed document-sized surface snapshot including GPU readback; no scrolling or PNG encoding",
                      timing="controller render-all command through completed full-surface snapshot reply; includes loading, preparation, rendering and readback; includes engine initialization unless warm",
                      coverage="requested common viewport must contain the whole laid-out document; overflow is rejected rather than tiled; viewport-relative CSS uses the requested document-sized viewport")
    if args.document_frames:
        report.update(schema=11, document_frames=True, readback=False, gpu_completion_fenced=False,
                      endpoint="document-sized viewport after two frame callbacks; no snapshot/readback; GPU completion and display presentation are not guaranteed",
                      timing="controller frame-all command through frame callback readiness; includes loading, preparation, normal drawing opportunities and frame pacing; excludes snapshots; engine initialization excluded only with --warm-engine",
                      coverage="requested common viewport must contain the whole laid-out document; overflow is rejected; this checks layout coverage, not completed GPU rasterization",
                      limitations="frame callbacks provide drawing opportunities, not GPU completion; wall time includes display pacing and CPU work may continue after the endpoint",
                      post_endpoint_cpu="diagnostic CPU from endpoint through the settling interval; includes deferred rendering and background work; excludes verification snapshots",
                      cpu_through_settle="CPU from load command through the settling interval; includes deferred rendering and background work; elapsed time still ends at frame callbacks; neither boundary is a GPU completion fence")
    if args.viewport_frames:
        report.update(schema=13, viewport_frames=True, readback=False, gpu_completion_fenced=False,
                      endpoint="first viewport after two frame callbacks; no snapshot/readback or GPU completion fence",
                      timing="controller load command through first-viewport frame callbacks; engine initialization included unless --warm-engine",
                      coverage="first viewport only; document layout may extend below it; offscreen paint commands are culled")
    if args.cpu_window_ms:
        report.update(schema=12, cpu_window_ms=args.cpu_window_ms,
                      endpoint="final fixed CPU measurement checkpoint after loading; no snapshot/readback or completion signal required",
                      timing="fixed observation interval, not render latency; host frame callback timing is retained only as a readiness diagnostic",
                      cpu="private cgroup CPU over the fixed interval; all host/helper threads and exited helpers; includes idle/background work; compare checkpoints for convergence",
                      limitations="CPU convergence must be checked with longer intervals; this measures CPU consumption, not GPU duration or presentation latency",
                      post_endpoint_cpu="diagnostic CPU between the final fixed checkpoint and the subsequent settling sample; excludes verification snapshots",
                      cpu_through_settle="CPU through the final fixed checkpoint plus subsequent settling; diagnostic only")
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
