import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("layout_bench", Path(__file__).with_name("run.py"))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class LayoutTraceTests(unittest.TestCase):
    def test_shared_fonts_do_not_require_a_display(self):
        with patch.dict(os.environ, {"DISPLAY":":1", "WAYLAND_DISPLAY":"wayland-0", "FONTCONFIG_FILE":"old.conf"}):
            environment = bench.headless_environment(Path("/private/fonts.conf"))
            self.assertNotIn("DISPLAY", environment)
            self.assertNotIn("WAYLAND_DISPLAY", environment)
            self.assertEqual(environment["FONTCONFIG_FILE"], "/private/fonts.conf")
            self.assertEqual(os.environ["FONTCONFIG_FILE"], "old.conf")

    def test_nested_stages_do_not_double_count_cpu(self):
        events = [
            {"ph":"X", "name":"ParseHTML", "pid":1, "tid":1, "ts":0, "dur":2000, "tts":0, "tdur":1000},
            {"ph":"X", "name":"ParseAuthorStyleSheet", "pid":1, "tid":1, "ts":100, "dur":400, "tts":100, "tdur":200},
            {"ph":"X", "name":"UpdateLayoutTree", "pid":1, "tid":1, "ts":2500, "dur":700, "tts":1500, "tdur":500},
            {"ph":"X", "name":"Layout", "pid":1, "tid":1, "ts":3200, "dur":1000, "tts":2000, "tdur":700},
            {"ph":"X", "name":"Layout", "pid":1, "tid":2, "ts":0, "dur":100, "tts":0, "tdur":100},
        ]
        result = bench.analyze_trace(events)
        self.assertAlmostEqual(result["engine_cpu_ms"], 2.3)
        self.assertAlmostEqual(result["engine_elapsed_ms"], 3.7)
        self.assertAlmostEqual(result["stage_cpu_ms"]["ParseAuthorStyleSheet"], .2)
        self.assertEqual(result["paint_events"], {})

    def test_paint_and_rasterization_invalidate_the_sample(self):
        for name in bench.PAINT_EVENTS:
            with self.subTest(name=name), self.assertRaises(ValueError):
                bench.analyze_trace([{"name":name, "ph":"X"}])

    def test_missing_cpu_timestamps_cannot_be_reported_as_cpu_time(self):
        with self.assertRaises(ValueError):
            bench.analyze_trace([{"ph":"X", "name":"Layout", "pid":1, "tid":1, "ts":10, "dur":200}])

    def test_layout_validation_rejects_missing_images(self):
        layout = {"width":800,"height":600,"scale":1,
                  "intrinsic_images":[{"width":640,"height":180}],"image_rects":[]}
        with self.assertRaises(ValueError):
            bench.validate_layout(layout, 800, 600)

    def test_different_image_geometry_is_rejected(self):
        left = {"table_rows":0,"intrinsic_images":[{"width":640,"height":180}],
                "image_rects":[{"x":24,"y":122,"width":640,"height":180}]}
        right = {**left,"image_rects":[{"x":24,"y":122,"width":640,"height":640}]}
        with self.assertRaises(ValueError):
            bench.compare_geometry(left, right)

    def test_existing_results_are_not_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "existing.json"
            report.write_text("preserved\n")
            command = [sys.executable, str(Path(__file__).with_name("run.py")),
                       "--skrift", sys.executable, "--chromium", sys.executable, "--output", str(report)]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("refusing to overwrite", result.stderr)
            self.assertEqual(report.read_text(), "preserved\n")


if __name__ == "__main__":
    unittest.main()
