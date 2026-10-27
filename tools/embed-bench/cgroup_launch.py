#!/usr/bin/env python3
"""Join the benchmark cgroup before the host can create helper processes."""

import os
import json
from pathlib import Path
import sys
import time

if __name__ == "__main__":
    (Path(sys.argv[1]) / "cgroup.procs").write_text("0\n")
    if os.environ.pop("SKRIFT_EMBED_STARTUP", None) == "1":
        values = dict(line.split() for line in (Path(sys.argv[1]) / "cpu.stat").read_text().splitlines())
        baseline = {key: int(values[key]) for key in ("usage_usec", "user_usec", "system_usec")}
        marker = {"event": "exec", "cpu": baseline, "monotonic_ns": time.monotonic_ns()}
        os.write(1, (json.dumps(marker) + "\n").encode())
    os.execv(sys.argv[2], sys.argv[2:])
