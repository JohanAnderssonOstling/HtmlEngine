#!/usr/bin/env python3
"""One sample through layout, with no paint, rasterization or display submission.

Uses Skrift's real CPU text backend and Chromium headless shell with manual
BeginFrame control. The old native-window runner and results are separate.
Requires websocket-client. See README.md in this directory for timing boundaries.
"""

import argparse
from collections import Counter
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import queue
import random
import signal
import subprocess
import tempfile
import threading
import time

import websocket

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("embed_memory", ROOT / "run.py")
accounting = importlib.util.module_from_spec(spec)
spec.loader.exec_module(accounting)
BLANK = "<!doctype html><html><head><style>html,body{margin:0;background:white}</style></head><body></body></html>"
STAGES = {"ParseHTML", "ParseAuthorStyleSheet", "UpdateLayoutTree", "Layout"}
PAINT_EVENTS = {"Paint", "PaintImage", "RasterTask", "CompositeLayers", "DrawFrame", "LayerTreeHostImpl::DrawLayers"}
READY_SCRIPT = """(async () => {
    await document.fonts.ready;
    await Promise.all(Array.from(document.images, image => image.decode()));
    const rect = document.body.getBoundingClientRect();
    return {width:innerWidth,height:innerHeight,scale:devicePixelRatio,
        content_width:document.documentElement.scrollWidth,
        content_height:document.documentElement.scrollHeight,
        body_rect:rect.toJSON(), text:document.body.innerText,
        table_rows:document.querySelectorAll('table tr').length,
        intrinsic_images:Array.from(document.images, image => ({width:image.naturalWidth,height:image.naturalHeight})),
        image_rects:Array.from(document.images, image => image.getBoundingClientRect().toJSON())};
})()"""


def union_duration(intervals):
    """Union nested/overlapping intervals rather than double-counting stages."""
    total, end = 0, None
    for start, stop in sorted(intervals):
        if end is None or start > end:
            total += stop - start
        else:
            total += max(0, stop - end)
        end = stop if end is None else max(end, stop)
    return total


def analyze_trace(events):
    invalid = Counter(event["name"] for event in events if event.get("name") in PAINT_EVENTS)
    if invalid:
        raise ValueError(f"browser painted or rasterized during the layout-only run: {dict(invalid)}")
    intervals, stages, elapsed = {}, {}, []
    for event in events:
        if event.get("ph") != "X" or event.get("name") not in STAGES:
            continue
        if "tts" not in event or "tdur" not in event:
            raise ValueError(f"CPU timestamps are unavailable for {event['name']}")
        if event["tdur"] < 0:
            raise ValueError("negative CPU trace duration")
        elapsed.append((event["ts"], event["ts"] + event["dur"]))
        key = (event["pid"], event["tid"])
        intervals.setdefault(key, []).append((event["tts"], event["tts"] + event["tdur"]))
        stages.setdefault(event["name"], {}).setdefault(key, []).append((event["tts"], event["tts"] + event["tdur"]))
    if not intervals:
        raise ValueError("no parse/style/layout CPU events were captured")
    return {"engine_elapsed_ms": union_duration(elapsed) / 1000,
            "engine_cpu_ms": sum(union_duration(values) for values in intervals.values()) / 1000,
            "stage_cpu_ms": {name: sum(union_duration(values) for values in threads.values()) / 1000
                             for name, threads in stages.items()},
            "paint_events": dict(invalid),
            "prepaint_events": sum(e.get("name") == "PrePaint" for e in events),
            "trace_event_counts": dict(Counter(e.get("name", "") for e in events if e.get("ph") == "X"))}


def validate_layout(layout, width, height):
    if (layout["width"], layout["height"], layout["scale"]) != (width, height, 1):
        raise ValueError(f"unexpected layout viewport: {layout}")
    images, rects = layout["intrinsic_images"], layout["image_rects"]
    if len(images) != len(rects):
        raise ValueError("not all decoded images have layout rectangles")
    if any(image["width"] <= 0 or image["height"] <= 0 for image in images):
        raise ValueError("an image has no intrinsic dimensions")


def compare_geometry(skrift, chromium):
    if skrift["table_rows"] != chromium["table_rows"]:
        raise ValueError("hosts laid out different numbers of table rows")
    images = lambda layout: [(image["width"], image["height"]) for image in layout["intrinsic_images"]]
    if images(skrift) != images(chromium):
        raise ValueError("hosts decoded different image dimensions")
    if len(skrift["image_rects"]) != len(chromium["image_rects"]):
        raise ValueError("hosts laid out different numbers of images")
    for left, right in zip(skrift["image_rects"], chromium["image_rects"]):
        if any(abs(left[name] - right[name]) > .5 for name in ["x", "y", "width", "height"]):
            raise ValueError(f"image geometry differs: {left}, {right}")
    return {"table_rows":skrift["table_rows"],"images":len(skrift["intrinsic_images"]),
            "image_geometry_matches":True,"pixel_equivalence_verified":False}


def stop_process(proc):
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait(timeout=5)


def headless_environment(fontconfig=None):
    environment = os.environ.copy()
    environment.pop("DISPLAY", None)
    environment.pop("WAYLAND_DISPLAY", None)
    if fontconfig is not None:
        environment["FONTCONFIG_FILE"] = str(fontconfig)
    return environment


def skrift_run(binary, fixture, args):
    with tempfile.TemporaryFile(mode="w+") as errors:
        proc = subprocess.Popen([str(binary), "--fixture", str(fixture), "--width", str(args.width),
                                 "--height", str(args.height)], stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=errors, text=True,
                                start_new_session=True, env=headless_environment(args.fontconfig))
        events = queue.Queue()
        reader = threading.Thread(target=accounting.read_stdout, args=(proc.stdout, events), daemon=True)
        reader.start()
        try:
            def ready(phase):
                event = events.get(timeout=args.timeout)
                if event.get("event") != "layout_ready" or event.get("phase") != phase:
                    raise RuntimeError(f"expected layout_ready/{phase}, received {event}")
                validate_layout(event["layout"], args.width, args.height)
                return event

            blank = ready("blank")
            time.sleep(args.settle_ms / 1000)
            baseline = accounting.process_memory(proc.pid)
            proc.stdin.write("load\n")
            proc.stdin.flush()
            document = ready("document")
            time.sleep(args.settle_ms / 1000)
            loaded = accounting.process_memory(proc.pid)
            proc.stdin.write("quit\n")
            proc.stdin.flush()
            if proc.wait(timeout=5):
                raise RuntimeError(f"Skrift exited with code {proc.returncode}")
            return {"label":"skrift", "engine_cpu_ms":document["engine_cpu_ms"],
                    "engine_elapsed_ms":document["engine_elapsed_ms"],
                    "load_to_layout_ms":document["load_to_layout_ms"],
                    "additional_pss_bytes":loaded["pss_bytes"] - baseline["pss_bytes"],
                    "memory":{"blank":baseline,"loaded":loaded}, "blank":blank,"document":document}
        except Exception as error:
            errors.seek(0)
            raise RuntimeError(f"Skrift {fixture.name}: {error}\n{errors.read()[-5000:]}") from error
        finally:
            stop_process(proc)
            proc.stdin.close()
            proc.stdout.close()
            reader.join(timeout=1)


class Cdp:
    def __init__(self, address, timeout):
        self.socket = websocket.create_connection(address, timeout=timeout, suppress_origin=True)
        self.sequence = 0
        self.events = []

    def call(self, method, params=None, session=None):
        self.sequence += 1
        request = {"id":self.sequence, "method":method, "params":params or {}}
        if session:
            request["sessionId"] = session
        self.socket.send(json.dumps(request))
        while True:
            response = json.loads(self.socket.recv())
            if response.get("id") == self.sequence:
                if "error" in response:
                    raise RuntimeError(f"{method}: {response['error']}")
                return response.get("result", {})
            self.events.append(response)

    def end_trace(self):
        self.call("Tracing.end")
        while not any(event.get("method") == "Tracing.tracingComplete" for event in self.events):
            self.events.append(json.loads(self.socket.recv()))
        events = [value for event in self.events if event.get("method") == "Tracing.dataCollected"
                  for value in event["params"]["value"]]
        complete = next(e for e in self.events if e.get("method") == "Tracing.tracingComplete")
        self.events.clear()
        if complete.get("params", {}).get("dataLossOccurred"):
            raise RuntimeError("Chromium lost trace events")
        return events


def chromium_run(binary, fixture, args, trace_directory):
    with tempfile.TemporaryDirectory(prefix="skrift-layout-") as profile, tempfile.TemporaryFile(mode="w+") as errors:
        command = [str(binary), "--remote-debugging-port=0", f"--user-data-dir={profile}",
                   "--enable-begin-frame-control", "--run-all-compositor-stages-before-draw", "--disable-gpu",
                   "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "about:blank"]
        proc = subprocess.Popen(command, stdout=errors, stderr=errors, start_new_session=True,
                                env=headless_environment(args.fontconfig))
        client = None
        try:
            deadline = time.monotonic() + args.timeout
            port_file = Path(profile) / "DevToolsActivePort"
            while not port_file.exists():
                if proc.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("Chromium failed to start its debugging endpoint")
                time.sleep(.02)
            port, path = port_file.read_text().splitlines()
            client = Cdp(f"ws://127.0.0.1:{port}{path}", args.timeout)
            version = client.call("Browser.getVersion")
            target = client.call("Target.createTarget", {"url":"about:blank", "width":args.width,
                    "height":args.height, "enableBeginFrameControl":True})["targetId"]
            session = client.call("Target.attachToTarget", {"targetId":target,"flatten":True})["sessionId"]
            for other in client.call("Target.getTargets")["targetInfos"]:
                if other["type"] == "page" and other["targetId"] != target:
                    client.call("Target.closeTarget", {"targetId":other["targetId"]})

            def page(method, params=None):
                return client.call(method, params, session)

            page("Page.enable")
            page("Performance.enable", {"timeDomain":"threadTicks"})
            page("Emulation.setDeviceMetricsOverride", {"width":args.width, "height":args.height,
                 "deviceScaleFactor":1, "mobile":False})
            frame = page("Page.getFrameTree")["frameTree"]["frame"]["id"]

            def load(source, phase):
                client.events.clear()
                client.call("Tracing.start", {"categories":"devtools.timeline,disabled-by-default-devtools.timeline,cc",
                                             "transferMode":"ReportEvents"})
                before = {metric["name"]:metric["value"] for metric in page("Performance.getMetrics")["metrics"]}
                started = time.monotonic()
                page("Page.setDocumentContent", {"frameId":frame,"html":source})
                evaluation = page("Runtime.evaluate", {"expression":READY_SCRIPT, "returnByValue":True,
                                  "awaitPromise":True})
                elapsed = (time.monotonic() - started) * 1000
                if "exceptionDetails" in evaluation:
                    raise RuntimeError(f"layout readiness failed: {evaluation['exceptionDetails']}")
                layout = evaluation["result"]["value"]
                validate_layout(layout, args.width, args.height)
                after = {metric["name"]:metric["value"] for metric in page("Performance.getMetrics")["metrics"]}
                # Keep the trace active through settling so a late paint also
                # invalidates the sample. No BeginFrame is ever requested.
                time.sleep(args.settle_ms / 1000)
                trace = client.end_trace()
                analysis = analyze_trace(trace)
                (trace_directory / f"{fixture.stem}-{phase}.json").write_text(json.dumps(trace) + "\n")
                return {**analysis, "layout":layout, "load_to_layout_ms":elapsed,
                        "performance_delta":{name:after[name] - before[name] for name in after if name.endswith("Duration") or name.endswith("Count")}}

            blank = load(BLANK, "blank")
            baseline = accounting.process_memory(proc.pid)
            document = load(fixture.read_text(), "document")
            loaded = accounting.process_memory(proc.pid)
            # Optional diagnostics run after both timing and memory readings.
            # Manual BeginFrame control still prevents drawing any frames.
            fonts = []
            if args.inspect_fonts:
                page("DOM.enable")
                page("CSS.enable")
                root = page("DOM.getDocument")["root"]["nodeId"]
                nodes = page("DOM.querySelectorAll", {"nodeId":root,"selector":"p"})["nodeIds"]
                fonts = [page("CSS.getPlatformFontsForNode", {"nodeId":node})["fonts"] for node in nodes]
            return {"label":"chromium", "engine_cpu_ms":document["engine_cpu_ms"],
                    "engine_elapsed_ms":document["engine_elapsed_ms"],
                    "load_to_layout_ms":document["load_to_layout_ms"],
                    "additional_pss_bytes":loaded["pss_bytes"] - baseline["pss_bytes"],
                    "memory":{"blank":baseline,"loaded":loaded}, "blank":blank,"document":document,
                    "browser":version, "flags":command[3:], "paragraph_fonts":fonts}
        except Exception as error:
            errors.seek(0)
            raise RuntimeError(f"Chromium {fixture.name}: {error}\n{errors.read()[-5000:]}") from error
        finally:
            if client:
                client.socket.close()
            stop_process(proc)


def default_chromium():
    candidates = list((Path.home() / ".cache/ms-playwright").glob("chromium_headless_shell-*/chrome-headless-shell-linux64/chrome-headless-shell"))
    return max(candidates, key=lambda path:int(path.parents[1].name.rsplit("-",1)[1])) if candidates else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skrift", type=Path, required=True)
    parser.add_argument("--chromium", type=Path, default=default_chromium(), help="chrome-headless-shell binary with BeginFrame control")
    parser.add_argument("--fixtures", type=Path, nargs="+")
    parser.add_argument("--fontconfig", type=Path, help="Fontconfig file shared by both hosts; system fonts remain the default")
    parser.add_argument("--inspect-fonts", action="store_true", help="record Chromium paragraph font selection after measurement")
    parser.add_argument("--width", type=int, default=800)
    parser.add_argument("--height", type=int, default=600)
    parser.add_argument("--settle-ms", type=int, default=500)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--output", type=Path, required=True, help="new report path; existing files are never overwritten")
    args = parser.parse_args()
    if platform.system() != "Linux" or args.chromium is None:
        parser.error("requires Linux procfs and Chromium headless shell")
    if not 1 <= args.width <= 8192 or not 1 <= args.height <= 8192 or args.settle_ms < 0 or args.timeout <= 0:
        parser.error("invalid viewport, settling interval or timeout")
    if args.output.exists():
        parser.error(f"refusing to overwrite existing results: {args.output}")
    args.skrift = args.skrift.resolve(strict=True)
    args.chromium = args.chromium.resolve(strict=True)
    if args.fontconfig is not None:
        args.fontconfig = args.fontconfig.resolve(strict=True)
    fixtures = [p.resolve(strict=True) for p in (args.fixtures or sorted((ROOT / "fixtures").glob("*.html")))]
    trace_directory = args.output.parent / (args.output.stem + "-traces")
    trace_directory.mkdir(parents=True, exist_ok=False)
    report = {"schema":1,"complete":False,"mode":"layout_without_paint","samples_per_fixture":1,
              "viewport":[args.width,args.height],"scale":1,"settle_ms":args.settle_ms,
              "environment":{"platform":platform.platform(),"cpu_count":os.cpu_count(),"load_average_before":os.getloadavg(),
                             "DISPLAY":None,"WAYLAND_DISPLAY":None},
              "timing":"Engine elapsed: Skrift time inside preparation/shaping/layout calls; Chromium union of traced parse/style/layout wall intervals. CPU time is reported separately. Resource preparation, protocol overhead and asynchronous gaps are excluded from engine elapsed.",
              "memory":"settled process-session PSS increase over empty document, after trace collection; includes retained instrumentation allocations",
              "hosts":[{"label":label,"binary":str(binary),"sha256":hashlib.sha256(binary.read_bytes()).hexdigest()}
                       for label,binary in [("skrift",args.skrift),("chromium",args.chromium)]],
              "fixtures":[{"path":str(p),"sha256":hashlib.sha256(p.read_bytes()).hexdigest()} for p in fixtures],
              "correctness_note":"Headless layout does not verify pixels. Mixed RTL placement differed in the older window benchmark; these measurements do not establish a fix.",
              "geometry_checks":{},"runs":[],"summary":[]}
    fontconfig = args.fontconfig or os.environ.get("FONTCONFIG_FILE")
    if fontconfig:
        path = Path(fontconfig).resolve(strict=True)
        report["environment"]["fontconfig"] = {"path":str(path),"sha256":hashlib.sha256(path.read_bytes()).hexdigest()}

    def save():
        report["summary"] = [{"label":r["label"],"fixture":r["fixture"],"samples":1,"engine_cpu_ms":r["engine_cpu_ms"],
                              "engine_elapsed_ms":r["engine_elapsed_ms"],
                              "load_to_layout_ms":r["load_to_layout_ms"],"additional_pss_bytes":r["additional_pss_bytes"]} for r in report["runs"]]
        temporary = args.output.with_suffix(args.output.suffix + ".tmp")
        temporary.write_text(json.dumps(report, indent=2) + "\n")
        temporary.replace(args.output)

    save()
    rng = random.Random(0)
    for fixture in fixtures:
        order = ["skrift", "chromium"]
        rng.shuffle(order)
        for label in order:
            row = skrift_run(args.skrift, fixture, args) if label == "skrift" else chromium_run(args.chromium, fixture, args, trace_directory)
            row["fixture"] = str(fixture)
            report["runs"].append(row)
            save()
            print(f"{label}: {fixture.name} 1/1", flush=True)
        paired = {row["label"]:row["document"]["layout"] for row in report["runs"] if row["fixture"] == str(fixture)}
        report["geometry_checks"][str(fixture)] = compare_geometry(paired["skrift"], paired["chromium"])
        save()
    report["complete"] = True
    report["environment"]["load_average_after"] = os.getloadavg()
    save()
    print(json.dumps(report["summary"], indent=2))


if __name__ == "__main__":
    main()
