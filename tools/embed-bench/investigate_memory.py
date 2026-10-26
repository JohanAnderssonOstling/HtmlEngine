#!/usr/bin/env python3
"""Untimed pipeline, allocator and resident mapping investigation on Linux."""

import argparse
from collections import defaultdict
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import signal
import subprocess
import tempfile
import threading
import time


def mapping_memory(text):
    groups = defaultdict(lambda: defaultdict(int))
    name = None
    for line in text.splitlines():
        if re.match(r"^[0-9a-f]+-[0-9a-f]+ ", line):
            fields = line.split(maxsplit=5)
            name = fields[5] if len(fields) == 6 else "[anonymous]"
        elif name is not None:
            parts = line.split()
            if len(parts) == 3 and parts[2] == "kB":
                groups[name][parts[0].rstrip(":")] += int(parts[1]) * 1024
    return {name: dict(values) for name, values in groups.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("refusing to overwrite existing investigation")
    binary = args.binary.resolve(strict=True)
    fixture = args.fixture.resolve(strict=True)
    environment = os.environ.copy()
    environment.pop("WAYLAND_DISPLAY", None)
    environment.pop("ZED_HEADLESS", None)
    environment.update(GDK_BACKEND="x11", GPUI_X11_SCALE_FACTOR="1")
    report = {"purpose":"Untimed memory investigation; checkpoints stop the host for smaps",
              "binary":str(binary), "binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),
              "fixture":str(fixture), "fixture_sha256":hashlib.sha256(fixture.read_bytes()).hexdigest(),
              "fontconfig":environment.get("FONTCONFIG_FILE"), "checkpoints":[]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    events = queue.Queue()
    phase = "blank_initialization"
    with tempfile.TemporaryFile(mode="w+") as errors:
        proc = subprocess.Popen([str(binary), "--fixture", str(fixture), "--width", "800", "--height", "600"],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors,
                                env=environment, text=True, start_new_session=True)

        def reader():
            try:
                for line in proc.stdout:
                    events.put(json.loads(line))
            except Exception as error:
                events.put({"event":"error", "message":str(error)})
            events.put({"event":"eof"})

        thread = threading.Thread(target=reader, daemon=True)
        thread.start()

        def save():
            args.output.write_text(json.dumps(report, indent=2) + "\n")

        def receive(kind, expected):
            while True:
                event = events.get(timeout=90)
                if event.get("event") in {"error", "eof"}:
                    raise RuntimeError(event)
                if event.get("event") == "memory_checkpoint":
                    deadline = time.monotonic() + 5
                    while Path(f"/proc/{proc.pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "T":
                        if time.monotonic() > deadline:
                            raise RuntimeError("host did not stop at the memory checkpoint")
                        time.sleep(.001)
                    mappings = mapping_memory(Path(f"/proc/{proc.pid}/smaps").read_text())
                    report["checkpoints"].append({"phase":phase, **event, "mappings":mappings,
                        "pss_bytes":sum(values.get("Pss", 0) for values in mappings.values())})
                    save()
                    os.kill(proc.pid, signal.SIGCONT)
                    if kind == "memory_checkpoint" and event["stage"] == expected:
                        return
                elif event.get("event") == kind and event.get("phase") == expected:
                    return

        def command(text):
            proc.stdin.write(text + "\n")
            proc.stdin.flush()

        try:
            receive("ready", "blank")
            for name, action, ready in [
                ("blank_settled", "memory", None),
                ("document", "load", "document"),
                ("document_settled", "memory", None),
                ("document_trimmed", "trim", None),
                ("clear", "clear", "clear"),
                ("clear_settled", "memory", None),
                ("clear_trimmed", "trim", None),
                ("reset", "reset", "reset"),
                ("reset_settled", "memory", None),
                ("reset_trimmed", "trim", None),
            ]:
                phase = name
                if name.endswith("settled"):
                    time.sleep(.5)
                command(action)
                receive("ready", ready) if ready else receive("memory_checkpoint", "after_malloc_trim" if action == "trim" else "settled")
            command("quit")
            proc.wait(timeout=10)
            if proc.returncode:
                raise RuntimeError(f"host exited with {proc.returncode}")
            report["complete"] = True
            save()
        except Exception:
            errors.seek(0)
            print(errors.read()[-5000:])
            raise
        finally:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            proc.wait(timeout=10)
            proc.stdin.close()
            proc.stdout.close()
            thread.join(timeout=1)
    for row in report["checkpoints"]:
        heap = row["heap"]
        print(row["phase"], row["stage"], "PSS MiB", round(row["pss_bytes"] / 2**20, 3),
              "Rust live MiB", round(heap["rust_live_requested_bytes"] / 2**20, 3),
              "engine MiB", round(sum(e["bytes"] for e in row["engine_entries"]) / 2**20, 3))


if __name__ == "__main__":
    main()
