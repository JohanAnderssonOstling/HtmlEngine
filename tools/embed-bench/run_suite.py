#!/usr/bin/env python3
"""Repeat the embedding fixtures with fixed CPU windows and no measured readback.

Use --prepare-only before running through fixed_frequency.py. Preparation records
untimed first-viewport geometry for both engines and writes run-benchmark.sh.
The measured suite covers cold/warm first viewports and warm whole documents.
Whole-document cases exceeding the surface limit are omitted. No scrolling.
"""

import argparse
from datetime import datetime
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import shlex
import statistics
import subprocess
import sys

BASE = Path(__file__).resolve().parent
CPUS = [8, 10, 12, 14]
KHZ = 3000000
WINDOWS = [100, 250, 500, 1000, 2000]
FIXTURES = [
    ("article", "article.html"), ("images", "images.html"),
    ("tables", "tables.html"), ("arabic", "languages/rtl-arabic.html"),
    ("japanese", "languages/cjk-japanese.html"),
    ("large", "large-document.html"),
    ("middlemarch", "books/middlemarch/chapter-37.xhtml"),
]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(path, data):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(data, indent=2) + "\n")
    temporary.replace(path)


def environment(fontconfig):
    result = os.environ.copy()
    result.pop("WAYLAND_DISPLAY", None)
    result.update(DISPLAY=result.get("DISPLAY", ":1"), FONTCONFIG_FILE=str(fontconfig))
    return result


def command(binary, fixture, mode, height, windows, engine, output):
    result = [sys.executable, str(BASE / "run.py"),
              "--host", f"{engine}={binary}", "--fixtures", str(fixture),
              "--width", "800", "--height", str(height),
              "--native-window", "--shared-gpui", "--phase-timings",
              "--cpus", *map(str, CPUS), "--cpu-window-ms", *map(str, windows),
              "--timeout", "120", "--output", str(output)]
    if mode != "cold-first":
        result.append("--warm-engine")
    result.append("--viewport-frames" if mode.endswith("first") else "--document-frames")
    return result


def prepare(args, env):
    fixtures = []
    for label, relative in FIXTURES:
        path = args.middlemarch_fixture if label == "middlemarch" else BASE / "fixtures" / relative
        path = path.resolve(strict=True)
        dependencies = [path]
        if label == "middlemarch":
            dependencies.extend(sorted((path.parent / "css").glob("*.css")))
        fixtures.append({"label": label, "path": str(path),
                         "dependencies": {str(p): digest(p) for p in dependencies}})
    identity = {"binary": str(args.binary), "binary_sha256": digest(args.binary),
                "fontconfig": str(args.fontconfig), "fontconfig_sha256": digest(args.fontconfig),
                "fixtures": fixtures, "samples_per_engine": args.samples,
                "cpus": CPUS, "khz": KHZ, "width": 800, "first_viewport_height": 600}
    plan_path = args.output / "plan.json"
    if plan_path.exists():
        plan = json.loads(plan_path.read_text())
        if plan["identity"] != identity:
            raise ValueError("saved plan differs from the executable, inputs or sample count; use a new output directory")
        if plan.get("planning_version") == 2:
            return plan
    geometry_path = args.output / "geometry.json"
    if geometry_path.exists():
        geometry = json.loads(geometry_path.read_text())
        if not geometry["complete"]:
            raise ValueError("incomplete geometry report; preserve it before preparing again")
        if geometry["hosts"][0]["sha256"] != identity["binary_sha256"]:
            raise ValueError("geometry used another executable; use a new output directory")
        observed = {str(p["path"]): p["sha256"] for p in geometry["fixtures"]}
        if observed != {f["path"]: f["dependencies"][f["path"]] for f in fixtures}:
            raise ValueError("geometry used different fixtures; use a new output directory")
    else:
        probe = [sys.executable, str(BASE / "run.py"),
                 "--host", f"skrift={args.binary}", "--host", f"wry={args.binary}",
                 "--fixtures", *(f["path"] for f in fixtures),
                 "--width", "800", "--height", "600", "--native-window", "--shared-gpui",
                 "--warm-engine", "--viewport-frames", "--settle-ms", "50",
                 "--timeout", "120", "--output", str(geometry_path)]
        print("Preparing untimed geometry for all seven fixtures and both engines…", flush=True)
        with (args.output / "geometry.log").open("x") as log:
            result = subprocess.run(probe, env=env, stdout=log, stderr=subprocess.STDOUT)
        if result.returncode:
            raise RuntimeError((args.output / "geometry.log").read_text()[-6000:])
        geometry = json.loads(geometry_path.read_text())
    cases, omitted, reused = [], [], []
    for fixture in fixtures:
        pages = {r["label"]: r["host"]["page"]
                 for r in geometry["runs"] if r["fixture"] == fixture["path"]}
        heights = {engine: page["content_height"] for engine, page in pages.items()}
        if set(heights) != {"skrift", "wry"} or any(
                not isinstance(h, (int, float)) or not math.isfinite(h) or h <= 0 for h in heights.values()):
            raise ValueError(f"invalid geometry: {fixture['label']}: {heights}")
        height = max(600, math.ceil(max(heights.values())))
        fits = height <= 600 and all(page.get("content_width", 800) <= 800 for page in pages.values())
        # Leave a small margin for whole-document sizing, then verify coverage
        # again on every measured run. Both engines get identical bounds.
        surface_height = max(height, 11026) if fixture["label"] == "middlemarch" else height + 2
        first_windows = WINDOWS + [5000] if height > 16384 else WINDOWS
        for mode in ("cold-first", "warm-first"):
            cases.append({"fixture": fixture["label"], "path": fixture["path"],
                          "mode": mode, "height": 600, "windows_ms": first_windows,
                          "document_fits_viewport": fits,
                          "coverage": "whole document" if fits else "first viewport"})
        if fits:
            reused.append({"fixture": fixture["label"], "mode": "whole-surface",
                           "source_mode": "warm-first", "height": 600,
                           "reason": "the whole document fits the first viewport in both engines"})
        elif surface_height > 16384:
            omitted.append({"fixture": fixture["label"], "mode": "whole-surface",
                            "reason": "document exceeds the 16,384-pixel single-surface limit",
                            "prepared_content_heights": heights})
        else:
            cases.append({"fixture": fixture["label"], "path": fixture["path"],
                          "mode": "whole-surface", "height": surface_height,
                          "windows_ms": WINDOWS, "prepared_content_heights": heights})
    plan = {"planning_version": 2, "identity": identity, "cases": cases,
            "omitted_cases": omitted, "reused_cases": reused,
            "total_measured_processes": len(cases) * args.samples * 2,
            "readback_in_measurement": False,
            "notes": ["Cold means engine initialization after an empty native GPUI window is ready; OS caches are not flushed.",
                      "Warm first and whole-document cases initialize the engine on an empty page before timing.",
                      "When the document fits the first viewport in both engines, the warm first-viewport result also covers the whole document; it is measured once.",
                      "No scroll benchmarks. The large document's whole-surface case exceeds the size limit and is omitted.",
                      "Longer windows are included for the large fixture; checkpoints before readiness may contain only partial work.",
                      "Fixture CSS is retained; Middlemarch uses the supplied hyphens-auto variant.",
                      "No GPU completion fence, presentation timing or exclusive core reservation."]}
    save(plan_path, plan)
    return plan


def validate(report, case, engine, plan):
    if not report["complete"] or len(report["runs"]) != 1:
        raise ValueError("measurement did not complete")
    if report["hosts"][0]["sha256"] != plan["identity"]["binary_sha256"]:
        raise ValueError("measurement used another executable")
    row = report["runs"][0]
    if row["label"] != engine or row["fixture"] != case["path"]:
        raise ValueError("measurement used the wrong engine or fixture")
    if row["host"]["page"]["width"] != 800 or row["host"]["page"]["height"] != case["height"]:
        raise ValueError("measurement used different viewport dimensions")
    if bool(row.get("warm_engine")) != (case["mode"] != "cold-first"):
        raise ValueError("measurement used the wrong engine baseline")
    flag = "viewport_frames" if case["mode"].endswith("first") else "document_frames"
    if not row.get(flag) or [w["requested_ms"] for w in row["cpu_windows"]] != case["windows_ms"]:
        raise ValueError("measurement used the wrong workload or CPU windows")
    if row.get("readback") is not False or "snapshot" in row["phase_timings"]:
        raise ValueError("measurement unexpectedly included readback")
    if case["mode"] == "whole-surface" and not row.get("document_geometry_covered"):
        raise ValueError("whole-document surface did not cover the document")
    if case.get("document_fits_viewport"):
        page = row["host"]["page"]
        height = page["content_height"]
        if not math.isfinite(height) or not 0 < height <= case["height"] or page.get("content_width", 800) > 800:
            raise ValueError("document no longer fits the first viewport; prepare a new plan")
    for phase in ("baseline", "loaded"):
        if (not row["cpu_affinity"][phase]["kernel_cpuset_enforced"]
                or row["cpu_affinity"][phase]["cpuset_effective_cpus"] != CPUS
                or row["cpu_frequency"][phase]["expected_khz"] != str(KHZ)):
            raise ValueError("measurement lacks the required CPU/frequency controls")
    if row["cpu_window_ms"] != case["windows_ms"][-1]:
        raise ValueError("measurement ended at the wrong CPU window")
    return row


def stats(values):
    return {"median": statistics.median(values), "min": min(values), "max": max(values), "samples": values}


def summarize(plan, measured):
    cases = []
    for case in plan["cases"]:
        result = {**case, "engines": {}}
        for engine in ("skrift", "wry"):
            rows = measured.get((case["mode"], case["fixture"], engine), [])
            if not rows:
                continue
            result["engines"][engine] = {
                "completed_samples": len(rows),
                "cpu_by_window_ms": {str(w): stats([next(v["cpu_ms"] for v in r["cpu_windows"]
                                                          if v["requested_ms"] == w) for r in rows])
                                     for w in case["windows_ms"]},
                "loaded_pss_mib": stats([r["memory"]["loaded"]["pss_bytes"] / 1048576 for r in rows]),
                "baseline_pss_mib": stats([r["memory"]["initialized" if r.get("warm_engine") else "native"]["pss_bytes"] / 1048576 for r in rows]),
                "post_window_cpu_ms": stats([r["post_endpoint_cpu_ms"] for r in rows]),
                "kernel_cpuset_verified": True, "frequency_limits_verified": True,
            }
        if set(result["engines"]) == {"skrift", "wry"}:
            last = str(case["windows_ms"][-1])
            skrift, wry = (result["engines"][e]["cpu_by_window_ms"][last]["median"] for e in ("skrift", "wry"))
            result["skrift_cpu_less_percent"] = 100 * (1 - skrift / wry)
        cases.append(result)
    count = sum(len(rows) for rows in measured.values())
    return {"complete": count == plan["total_measured_processes"],
            "completed_measurements": count, "planned_measurements": plan["total_measured_processes"],
            "configuration": plan["identity"], "notes": plan["notes"],
            "omitted_cases": plan["omitted_cases"],
            "reused_cases": plan["reused_cases"], "cases": cases}


def measure(args, plan, env):
    if (env.get("SKRIFT_EMBED_FIXED_CPU_KHZ") != str(KHZ)
            or env.get("SKRIFT_EMBED_CPUSET_CPUS") != ",".join(map(str, CPUS))
            or not env.get("SKRIFT_EMBED_CPU_CGROUP_PARENT")):
        raise ValueError("run the prepared run-benchmark.sh so the frequency and kernel cpuset limits are active")
    measured = {}
    summary = summarize(plan, measured)
    for sample in range(1, args.samples + 1):
        for case in plan["cases"]:
            for engine in (("skrift", "wry") if sample % 2 else ("wry", "skrift")):
                stem = f"{case['mode']}-{case['fixture']}-{engine}-{sample:02}"
                report_path = args.output / f"{stem}.json"
                log_path = args.output / f"{stem}.log"
                if report_path.exists():
                    report = json.loads(report_path.read_text())
                    if not report["complete"]:
                        archive = args.output / "attempts" / datetime.now().strftime("%Y%m%d-%H%M%S-%f")
                        archive.mkdir(parents=True)
                        report_path.rename(archive / report_path.name)
                        if log_path.exists():
                            log_path.rename(archive / log_path.name)
                    else:
                        row = validate(report, case, engine, plan)
                        measured.setdefault((case["mode"], case["fixture"], engine), []).append(row)
                        summary = summarize(plan, measured)
                        save(args.output / "summary.json", summary)
                        print(f"Retained {stem}", flush=True)
                        continue
                if log_path.exists():
                    archive = args.output / "attempts" / datetime.now().strftime("%Y%m%d-%H%M%S-%f")
                    archive.mkdir(parents=True)
                    log_path.rename(archive / log_path.name)
                run = command(args.binary, Path(case["path"]), case["mode"], case["height"],
                              case["windows_ms"], engine, report_path)
                save(args.output / "current-command.json", {"command": run, "cpu_cgroup_parent": env["SKRIFT_EMBED_CPU_CGROUP_PARENT"]})
                with log_path.open("x") as log:
                    process = subprocess.run(run, env=env, stdout=log, stderr=subprocess.STDOUT)
                if process.returncode:
                    raise RuntimeError(f"{stem} failed; completed samples are retained\n{log_path.read_text()[-6000:]}")
                row = validate(json.loads(report_path.read_text()), case, engine, plan)
                measured.setdefault((case["mode"], case["fixture"], engine), []).append(row)
                summary = summarize(plan, measured)
                save(args.output / "summary.json", summary)
                print(f"{summary['completed_measurements']}/{summary['planned_measurements']} {stem}: "
                      + ", ".join(f"{w['requested_ms']}ms: {w['cpu_ms']:.3f}ms CPU" for w in row["cpu_windows"]), flush=True)
    for case in summary["cases"]:
        last = str(case["windows_ms"][-1])
        a, b = (case["engines"][e]["cpu_by_window_ms"][last]["median"] for e in ("skrift", "wry"))
        print(f"{case['mode']:14} {case['fixture']:12} Skrift {a:9.3f}ms / WebKit {b:9.3f}ms CPU ({last}ms window)", flush=True)
    print(f"Saved {args.output / 'summary.json'}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fontconfig", type=Path, required=True)
    parser.add_argument("--middlemarch-fixture", type=Path, default=BASE / "fixtures/books/middlemarch/chapter-37.xhtml")
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("sample count must be positive")
    args.binary = args.binary.resolve(strict=True)
    args.fontconfig = args.fontconfig.resolve(strict=True)
    args.middlemarch_fixture = args.middlemarch_fixture.resolve(strict=True)
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    with (args.output / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        env = environment(args.fontconfig)
        plan = prepare(args, env)
        if args.prepare_only:
            invocation = [sys.executable, str(BASE / "fixed_frequency.py"),
                          "--cpus", *map(str, CPUS), "--khz", str(KHZ), "--",
                          sys.executable, str(Path(__file__).resolve()),
                          "--binary", str(args.binary), "--output", str(args.output),
                          "--fontconfig", str(args.fontconfig), "--middlemarch-fixture", str(args.middlemarch_fixture),
                          "--samples", str(args.samples)]
            (args.output / "run-benchmark.sh").write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + shlex.join(invocation) + "\n")
            print(f"Prepared {len(plan['cases'])} cases, {plan['total_measured_processes']} measured processes.\nRun: bash {args.output / 'run-benchmark.sh'}", flush=True)
        else:
            measure(args, plan, env)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
