import importlib.util
from pathlib import Path
import os
import signal
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("embed_bench", Path(__file__).with_name("run.py"))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class MemoryAccountingTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == 'linux', 'requires cgroup v2')
    def test_verification_encoding_happens_after_cpu_and_ram_samples(self):
        parent = Path(f'/sys/fs/cgroup/user.slice/user-{os.getuid()}.slice/user@{os.getuid()}.service')
        if not os.access(parent, os.W_OK):
            self.skipTest('no writable delegated cgroup')
        script = '''#!/usr/bin/env python3
import json, sys, time
event = {'event': 'ready', 'phase': 'native', 'wry_initialized': False,
         'page': {'width': 800, 'height': 600, 'scale': 1}, 'frame_width': 800, 'frame_height': 600}
print(json.dumps(event), flush=True)
assert input() == 'load'
end = time.process_time() + .02
while time.process_time() < end: pass
event.update(phase='document', snapshot_width=800, snapshot_height=600, document_to_snapshot_ms=20)
print(json.dumps(event), flush=True)
command = input()
assert command.startswith('capture ')
end = time.process_time() + .08
while time.process_time() < end: pass
print(json.dumps({'event': 'capture', 'path': command[8:], 'width': 800, 'height': 600}), flush=True)
assert input() == 'quit'
'''
        args = SimpleNamespace(width=800, height=600, scale_factor=1, settle_ms=0, timeout=5,
                               native_window=True, cpu_cgroup_parent=parent)
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'host'
            binary.write_text(script); binary.chmod(0o755)
            capture = Path(directory) / 'capture.png'
            with patch.object(bench, 'process_memory', side_effect=[
                {'pss_bytes': 10000}, {'pss_bytes': 14096}
            ]) as memory:
                result = bench.run_host(binary, Path('fixture.html'), args, verify_screenshot=capture)
        self.assertEqual(memory.call_count, 2)
        self.assertEqual(result['additional_pss_bytes'], 4096)
        self.assertGreaterEqual(result['additional_cpu_ms'], 20)
        self.assertLess(result['additional_cpu_ms'], 70)
        self.assertEqual(result['verification_screenshot'], str(capture))

    def test_shared_gpui_selects_each_engine_in_the_same_executable(self):
        script = '''#!/usr/bin/env python3
import json, sys
content = sys.argv[sys.argv.index('--content') + 1]
event = {'event': 'ready', 'phase': 'native', 'wry_initialized': False,
         'skrift_initialized': False, 'page': {'width': 800, 'height': 600, 'scale': 1},
         'frame_width': 800, 'frame_height': 600}
print(json.dumps(event), flush=True)
assert input() == 'load'
event.update(phase='document', engine=content, snapshot_width=800, snapshot_height=600,
             document_to_snapshot_ms=1.0)
print(json.dumps(event), flush=True)
assert input() == 'quit'
'''
        args = SimpleNamespace(width=800, height=600, scale_factor=1, settle_ms=0,
                               timeout=5, native_window=True)
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'host'
            binary.write_text(script)
            binary.chmod(0o755)
            with patch.object(bench, 'process_memory', return_value={'pss_bytes': 10000}):
                for content in ('skrift', 'wry'):
                    result = bench.run_host(binary, Path('fixture.html'), args, content_engine=content)
                    self.assertEqual(result['host']['engine'], content)

    def test_shared_gpui_rejects_different_executables(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([sys.executable, str(Path(__file__).with_name('run.py')),
                '--shared-gpui', '--native-window', '--host', f'skrift={sys.executable}',
                '--host', 'wry=/bin/true', '--output', str(Path(directory) / 'report.json')],
                capture_output=True, text=True, env={**os.environ, 'DISPLAY': ':0'})
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('same executable', result.stderr)

    def test_native_baseline_requires_an_uninitialized_webview(self):
        event = {"page": {"width": 800, "height": 600, "scale": 1},
                 "frame_width": 800, "frame_height": 600, "wry_initialized": False}
        bench.validate_viewport(event, 800, 600, 1, native=True)
        event['wry_initialized'] = True
        with self.assertRaisesRegex(ValueError, 'already initialized HTML content'):
            bench.validate_viewport(event, 800, 600, 1, native=True)

    def test_gpui_native_baseline_requires_skrift_to_be_uninitialized(self):
        event = {"page": {"width": 800, "height": 600, "scale": 1},
                 "frame_width": 800, "frame_height": 600, "skrift_initialized": False}
        bench.validate_viewport(event, 800, 600, 1, native=True)
        event['skrift_initialized'] = True
        with self.assertRaises(ValueError):
            bench.validate_viewport(event, 800, 600, 1, native=True)

    def test_native_window_baseline_is_sampled_before_webview_creation(self):
        script = '''#!/usr/bin/env python3
import json, sys
assert "--native-window" in sys.argv
event = {"event": "ready", "phase": "native", "wry_initialized": False,
         "page": {"width": 800, "height": 600, "scale": 1},
         "frame_width": 800, "frame_height": 600}
print(json.dumps(event), flush=True)
assert input() == "load"
event.update(phase="document", snapshot_width=800, snapshot_height=600,
             document_to_snapshot_ms=15.0)
print(json.dumps(event), flush=True)
assert input() == "quit"
'''
        args = SimpleNamespace(width=800, height=600, scale_factor=1, settle_ms=0,
                               timeout=5, native_window=True)
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'host'
            binary.write_text(script)
            binary.chmod(0o755)
            with patch.object(bench, 'process_memory', side_effect=[
                {'pss_bytes': 10000}, {'pss_bytes': 30000}
            ]) as memory:
                result = bench.run_host(binary, Path('fixture.html'), args)
        self.assertEqual(memory.call_count, 2)
        self.assertEqual(result['additional_pss_bytes'], 20000)
        self.assertEqual(result['memory']['native']['pss_bytes'], 10000)
        self.assertIs(result['baseline_host']['wry_initialized'], False)
        self.assertNotIn('blank', result['memory'])

    @unittest.skipUnless(sys.platform == "linux", "requires cgroup v2")
    def test_startup_renders_directly_and_counts_cpu_from_exec(self):
        parent = Path(f"/sys/fs/cgroup/user.slice/user-{os.getuid()}.slice/user@{os.getuid()}.service")
        if not os.access(parent, os.W_OK):
            self.skipTest("no writable delegated cgroup")
        script = '''#!/usr/bin/env python3
import json, sys, time
assert "--startup-document" in sys.argv
end = time.process_time() + .02
while time.process_time() < end: pass
print(json.dumps({"event": "ready", "phase": "document",
      "page": {"width": 800, "height": 600, "scale": 1},
      "snapshot_width": 800, "snapshot_height": 600}), flush=True)
assert input() == "quit"
'''
        args = SimpleNamespace(width=800, height=600, scale_factor=1, settle_ms=0,
                               timeout=5, startup=True, cpu_cgroup_parent=parent)
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "host"
            binary.write_text(script)
            binary.chmod(0o755)
            with patch.object(bench, "process_memory", return_value={"pss_bytes": 12345}) as memory:
                result = bench.run_host(binary, Path("fixture.html"), args)
        self.assertEqual(memory.call_count, 1)
        self.assertGreaterEqual(result["startup_cpu_ms"], 20)
        self.assertGreater(result["startup_elapsed_ms"], 0)
        self.assertEqual(result["total_pss_bytes"], 12345)
        self.assertNotIn("blank", result["memory"])
        self.assertEqual(bench.summarize([dict(result, label="fake", fixture="fixture.html")]), [
            {"label": "fake", "fixture": "fixture.html", "samples": 1,
             "startup_cpu_ms": result["startup_cpu_ms"], "total_pss_bytes": 12345}
        ])

    def test_cpu_comparison_summary_keeps_ram_and_omits_elapsed_time(self):
        row = {"label": "fake", "fixture": "fixture.html", "additional_cpu_ms": 20.0,
               "additional_time_ms": 12.0, "additional_pss_bytes": 4096,
               "memory":{"blank":{"pss_bytes":10000},"loaded":{"pss_bytes":14096}}}
        self.assertEqual(bench.summarize([row]), [
            {"label": "fake", "fixture": "fixture.html", "samples": 1,
             "additional_cpu_ms": 20.0, "memory_before_pss_bytes":10000,"memory_after_pss_bytes":14096}
        ])

    def test_existing_results_are_not_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "previous.json"
            report.write_text("preserved\n")
            result = subprocess.run([sys.executable, str(Path(__file__).with_name("run.py")),
                                     "--host", f"fake={sys.executable}", "--output", str(report)],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("refusing to overwrite", result.stderr)
            self.assertEqual(report.read_text(), "preserved\n")

    @unittest.skipUnless(sys.platform == "linux", "requires cgroup v2")
    def test_cgroup_cpu_includes_helpers_after_they_exit(self):
        parent = Path(f"/sys/fs/cgroup/user.slice/user-{os.getuid()}.slice/user@{os.getuid()}.service")
        if not os.access(parent, os.W_OK):
            self.skipTest("no writable delegated cgroup")
        child = "import time; end=time.process_time()+.04\nwhile time.process_time()<end: pass"
        script = f"import subprocess,sys,time\nsubprocess.run([sys.executable,'-c',{child!r}],check=True)\nend=time.process_time()+.04\nwhile time.process_time()<end: pass"
        with bench.CpuGroup(parent) as group:
            before = group.read()
            subprocess.run(group.command([sys.executable, "-c", script]), check=True, start_new_session=True)
            after = group.read()
            self.assertEqual(after["pids"], [])
            self.assertGreaterEqual(after["usage_usec"] - before["usage_usec"], 80000)
        self.assertFalse(group.path.exists())

    def test_wrong_pixel_density_is_rejected(self):
        event = {"page": {"width": 800, "height": 600, "scale": 2},
                 "snapshot_width": 800, "snapshot_height": 600}
        with self.assertRaises(ValueError):
            bench.validate_viewport(event, 800, 600)
        event.update(snapshot_width=1600, snapshot_height=1200)
        bench.validate_viewport(event, 800, 600)
        with self.assertRaises(ValueError):
            bench.validate_viewport(event, 800, 600, scale=1)

    def test_one_load_reports_time_after_blank_and_memory_difference(self):
        script = '''#!/usr/bin/env python3
import json, sys
event = {"event": "ready", "phase": "blank",
         "page": {"width": 800, "height": 600, "scale": 1},
         "snapshot_width": 800, "snapshot_height": 600}
print(json.dumps(event), flush=True)
assert input() == "load"
print("HTML_GPUI_SCENE commands=1 duration_ms=11", flush=True)
event.update(phase="document", document_to_snapshot_ms=12.5)
print(json.dumps(event), flush=True)
assert input() == "quit"
'''
        args = SimpleNamespace(width=800, height=600, scale_factor=1, settle_ms=0, timeout=5)
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "host"
            binary.write_text(script)
            binary.chmod(0o755)
            with patch.object(bench, "process_memory", side_effect=[
                {"pss_bytes": 10000}, {"pss_bytes": 14096}
            ]) as memory:
                result = bench.run_host(binary, Path("fixture.html"), args)
        self.assertEqual(memory.call_count, 2)
        self.assertEqual(result["additional_time_ms"], 12.5)
        self.assertEqual(result["additional_pss_bytes"], 4096)
        self.assertEqual(result["diagnostics"], ["HTML_GPUI_SCENE commands=1 duration_ms=11"])
        self.assertEqual(bench.summarize([dict(result, label="fake", fixture="fixture.html")]), [
            {"label": "fake", "fixture": "fixture.html", "samples": 1,
             "additional_time_ms": 12.5, "memory_before_pss_bytes":10000,"memory_after_pss_bytes":14096}
        ])

    def test_shared_memory_is_counted_proportionally(self):
        result = bench.parse_smaps("""0000-ffff ---p 00000000 00:00 0 [rollup]
Rss:                 100 kB
Pss:                  60 kB
Pss_Anon:             30 kB
Private_Clean:        10 kB
Private_Dirty:        20 kB
""")
        self.assertEqual(result, {"rss_bytes": 102400, "pss_bytes": 61440, "private_bytes": 30720})

    def test_incomplete_memory_is_rejected(self):
        with self.assertRaises(ValueError):
            bench.parse_smaps("Rss: 100 kB\n")

    @unittest.skipUnless(sys.platform == "linux", "requires Linux procfs")
    def test_helper_processes_are_included_and_runner_is_excluded(self):
        script = """
import subprocess, sys, time
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
print(child.pid, flush=True)
time.sleep(30)
"""
        process = subprocess.Popen([sys.executable, "-c", script], stdout=subprocess.PIPE,
                                   text=True, start_new_session=True)
        try:
            child = int(process.stdout.readline())
            measured = bench.process_memory(process.pid)
            self.assertIn(process.pid, measured["pids"])
            self.assertIn(child, measured["pids"])
            self.assertNotIn(os.getpid(), measured["pids"])
            self.assertGreater(measured["pss_bytes"], 0)
        finally:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=5)
            process.stdout.close()


if __name__ == "__main__":
    unittest.main()
