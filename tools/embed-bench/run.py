#!/usr/bin/env python3
r"""Measure the added cost of HTML in an existing empty embedding window on Linux.

Example:
  cargo build --release --manifest-path tools/embed-bench/wry/Cargo.toml
  python3 tools/embed-bench/run.py --host wry=tools/embed-bench/wry/target/release/embed-bench-wry \
    --output /tmp/embed-results.json

Each fixture gets one document load per host. First render an empty document,
let it settle and read baseline memory. Then time loading the fixture through a
completed viewport snapshot, including GPU/IPC readback, excluding PNG encoding.
After the same settling interval, read memory again and subtract the baseline.
Window startup is excluded. Both measurements come from the same document load;
there are no warmup loads, repeated trials or periodic memory sampling.

PSS covers all processes in the host's private session, sharing resident memory
proportionally. It does not cover all GPU VRAM. Before/after counters are retained
for auditing; the summary contains only added time and retained PSS. Inspect any
saved screenshots before interpreting cross-engine results: faster output with
missing content is not a valid result. Screenshot encoding can affect retained
memory, so omit --screenshots for memory comparisons.

--cpu-cgroup-parent additionally counts all host and helper CPU in a private
cgroup, including exited helpers. It uses the same controller load-to-output
boundary for elapsed time on both hosts. This mode forbids PNG encoding.
"""

import argparse
import hashlib
import json
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


def validate_viewport(event, width, height, scale=None):
    page = event["page"]
    if page["width"] != width or page["height"] != height or page["scale"] <= 0:
        raise ValueError(f"host viewport differs from the requested viewport: {page}")
    if scale is not None and page["scale"] != scale:
        raise ValueError(f"host scale {page['scale']} differs from requested scale {scale}")
    expected = (round(width * page["scale"]), round(height * page["scale"]))
    actual = (event["snapshot_width"], event["snapshot_height"])
    if actual != expected:
        raise ValueError(f"snapshot dimensions {actual} differ from viewport pixels {expected}")


def run_host(binary, fixture, args, screenshot=None):
    command = [str(binary), "--fixture", str(fixture), "--width", str(args.width), "--height", str(args.height)]
    if screenshot:
        command += ["--screenshot", str(screenshot)]
    environment = os.environ.copy()
    environment.pop("WAYLAND_DISPLAY", None)
    environment.pop("ZED_HEADLESS", None)
    environment.update(GDK_BACKEND="x11", GDK_SCALE=str(args.scale_factor),
                       GDK_DPI_SCALE="1", GPUI_X11_SCALE_FACTOR=str(args.scale_factor))
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
                validate_viewport(event, args.width, args.height, args.scale_factor)
                return event

            ready("blank")
            time.sleep(args.settle_ms / 1000)
            baseline = process_memory(proc.pid)
            cpu_baseline = cpu.read()
            started = time.monotonic_ns()
            proc.stdin.write("load\n")
            proc.stdin.flush()
            document = ready("document")
            rendered_at = time.monotonic_ns()
            cpu_rendered = cpu.read()
            time.sleep(args.settle_ms / 1000)
            loaded = process_memory(proc.pid)
            result = {
                "host": document,
                "additional_time_ms": document["document_to_snapshot_ms"],
                "additional_pss_bytes": loaded["pss_bytes"] - baseline["pss_bytes"],
                "memory": {"blank": baseline, "loaded": loaded},
                "diagnostics": diagnostics,
                "load_command_to_output_ms": (rendered_at - started) / 1_000_000,
            }
            if cpu_baseline is not None:
                delta = cpu_rendered["usage_usec"] - cpu_baseline["usage_usec"]
                if delta < 0:
                    raise RuntimeError("cgroup CPU counter decreased")
                result["additional_cpu_ms"] = delta / 1000
                result["cpu"] = {"blank": cpu_baseline, "rendered": cpu_rendered,
                                 "scope": "all threads and helper processes in a private cgroup; exited helpers retained"}
                # Use the same controller boundary for both hosts, including
                # stdin delivery and readiness notification. No PNG is encoded.
                result["additional_time_ms"] = result["load_command_to_output_ms"]
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
    return [{"label": row["label"], "fixture": row["fixture"], "samples": 1,
             **({"additional_cpu_ms": row["additional_cpu_ms"]} if "additional_cpu_ms" in row
                else {"additional_time_ms": row["additional_time_ms"]}),
             "additional_pss_bytes": row["additional_pss_bytes"]} for row in rows]


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
    parser.add_argument("--cpu-cgroup-parent", type=Path,
                        help="writable delegated cgroup v2 parent; enables total host and helper CPU accounting")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error(f"refusing to overwrite existing results: {args.output}")
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
        "memory": "settled document PSS minus settled empty-window PSS, including helper processes",
        "endpoint": "viewport snapshot including readback, excluding PNG encoding",
        "pixel_equivalence_verified": False,
        "hosts": [{"label": label, "binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()} for label, binary in hosts],
        "fixtures": [{"path": str(p), "sha256": hashlib.sha256(p.read_bytes()).hexdigest()} for p in fixtures],
        "runs": [], "summary": [],
    }
    if args.cpu_cgroup_parent:
        report["timing"] = "controller load command through rendered snapshot readiness, including readback and IPC; excludes startup and PNG encoding"
        report["cpu"] = "private cgroup v2 usage increase over the same load interval; all host threads and helper processes, including exited helpers; no tracing"
        report["cpu_cgroup_parent"] = str(args.cpu_cgroup_parent)
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
            row = run_host(binary, fixture, args, screenshot=image)
            row.update(label=label, fixture=str(fixture))
            report["runs"].append(row)
            save()
            print(f"{label}: {fixture.name} 1/1", flush=True)
    report["environment"]["load_average_after"] = os.getloadavg()
    report["complete"] = True
    save()
    print(json.dumps(report["summary"], indent=2))


if __name__ == "__main__":
    main()
