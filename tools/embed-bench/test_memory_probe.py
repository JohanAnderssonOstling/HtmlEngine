import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("memory_investigation", Path(__file__).with_name("investigate_memory.py"))
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class MappingMemoryTests(unittest.TestCase):
    def test_anonymous_mappings_are_merged_without_counting_rss_as_pss(self):
        result = probe.mapping_memory("""1000-2000 rw-p 00000000 00:00 0
Rss: 100 kB
Pss: 60 kB
Anonymous: 60 kB
3000-4000 rw-p 00000000 00:00 0
Rss: 20 kB
Pss: 10 kB
Anonymous: 10 kB
""")
        self.assertEqual(result["[anonymous]"]["Pss"], 70 * 1024)
        self.assertEqual(result["[anonymous]"]["Rss"], 120 * 1024)

    def test_mapping_names_with_spaces_and_heap_are_kept_separate(self):
        result = probe.mapping_memory("""1000-2000 rw-p 00000000 00:00 0 [heap]
Pss: 10 kB
3000-4000 r--p 00000000 00:00 1 /fonts/Font Name.ttf
Pss: 5 kB
""")
        self.assertEqual(result["[heap]"]["Pss"], 10 * 1024)
        self.assertEqual(result["/fonts/Font Name.ttf"]["Pss"], 5 * 1024)


if __name__ == "__main__":
    unittest.main()
