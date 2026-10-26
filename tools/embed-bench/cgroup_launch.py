#!/usr/bin/env python3
"""Join the benchmark cgroup before the host can create helper processes."""

import os
from pathlib import Path
import sys

if __name__ == "__main__":
    (Path(sys.argv[1]) / "cgroup.procs").write_text("0\n")
    os.execv(sys.argv[2], sys.argv[2:])
