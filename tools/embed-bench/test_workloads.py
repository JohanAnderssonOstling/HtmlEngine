import importlib.util
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('workload_bench', Path(__file__).with_name('run.py'))
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)

PRELUDE = '''#!/usr/bin/env python3
import json,time
base={'event':'ready','phase':'native','skrift_initialized':False,
      'page':{'width':800,'height':600,'scale':1,'content_height':1450,'scroll_y':0},
      'frame_width':800,'frame_height':600,'snapshot_width':800,'snapshot_height':600,
      'document_to_snapshot_ms':1}
def ready(phase,y=0):
 base['phase']=phase;base['page']['scroll_y']=y
 print(json.dumps(base),flush=True)
def cpu(ms):
 end=time.process_time()+ms/1000
 while time.process_time()<end:pass
ready('native')
'''


class WorkloadTests(unittest.TestCase):
    def run_script(self, body, **options):
        verify = options.pop('verify', False)
        on_measured = options.pop('on_measured', None)
        args = SimpleNamespace(width=800, height=600, scale_factor=1, settle_ms=0,
                               timeout=5, native_window=True, **options)
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'host'
            binary.write_text(PRELUDE + body)
            binary.chmod(0o755)
            with patch.object(bench, 'process_memory', side_effect=[
                {'pss_bytes':10000}, {'pss_bytes':14096}
            ]) as memory:
                result = bench.run_host(binary, Path('fixture.xhtml'), args,
                                        verify_screenshot=Path(directory) / 'capture.png' if verify else None,
                                        on_measured=on_measured)
                self.assertEqual(memory.call_count, 2)
        return result

    def test_completed_measurement_is_available_before_failed_capture(self):
        measured = []
        with self.assertRaisesRegex(RuntimeError, 'invalid verification capture'):
            self.run_script('''
assert input()=='load'
ready('document')
assert input().startswith('capture ')
print(json.dumps({'event':'failed_capture'}),flush=True)
input()
''', verify=True, on_measured=measured.append)
        self.assertEqual(len(measured), 1)
        self.assertEqual(measured[0]['memory']['loaded']['pss_bytes'], 14096)
        self.assertNotIn('verification_screenshot', measured[0])

    def test_whole_document_captures_ignore_paint_diagnostics(self):
        result = self.run_script('''
assert input()=='load-all'
ready('document')
assert input()=='tile 600.0'
ready('tile',600)
assert input()=='tile 850'
ready('tile',850)
for position in range(3):
 if position:
  command=input()
  assert command.startswith('tile ')
  ready('tile',float(command.split()[1]))
 command=input()
 assert command.startswith('capture ')
 print('HTML_GPUI_SCENE commands=43 duration_ms=159',flush=True)
 print(json.dumps({'event':'capture','path':command[8:], 'width':800,'height':600}),flush=True)
assert input()=='quit'
''', full_document=True, verify=True)
        self.assertEqual(len(result['verification_viewports']), 3)
        self.assertEqual([v['scroll_y'] for v in result['verification_viewports']], [850,0,425])

    def test_all_viewports_are_rendered_before_endpoint(self):
        result = self.run_script('''
assert input()=='load-all'
ready('document')
assert input()=='tile 600.0'
ready('tile',600)
assert input()=='tile 850'
time.sleep(.1)
ready('tile',850)
assert input()=='quit'
''', full_document=True)
        self.assertTrue(result['full_document_covered'])
        self.assertEqual([x['page']['scroll_y'] for x in result['rendered_viewports']], [0,600,850])
        self.assertEqual(result['additional_pss_bytes'], 4096)
        self.assertGreaterEqual(result['additional_time_ms'], 100)

    def test_missing_scroll_coverage_is_rejected(self):
        with self.assertRaisesRegex(RuntimeError, 'does not match'):
            self.run_script('''
assert input()=='load-all'
ready('document')
assert input()=='tile 600.0'
ready('tile',500)
input()
''', full_document=True)

    def test_warm_cpu_excludes_initialization(self):
        parent = Path(f'/sys/fs/cgroup/user.slice/user-{os.getuid()}.slice/user@{os.getuid()}.service')
        if not os.access(parent, os.W_OK):
            self.skipTest('requires a writable delegated cgroup')
        result = self.run_script('''
assert input()=='warm'
cpu(100)
ready('warm')
assert input()=='load'
cpu(20)
ready('document')
assert input()=='quit'
''', warm_engine=True, cpu_cgroup_parent=parent)
        self.assertIn('initialized', result['memory'])
        self.assertIn('initialized', result['cpu'])
        self.assertGreaterEqual(result['additional_cpu_ms'], 20)
        self.assertLess(result['additional_cpu_ms'], 80)
        self.assertEqual(result['initialized_baseline_host']['phase'], 'warm')


if __name__ == '__main__':
    unittest.main()
