"""Generate the chapter used by query_search, query_roles, and query_restore.

Usage: python3 tools/pipeline-bench/query_fixtures.py /tmp/query-chapter.html
query_table generates its fixed table internally.
"""

import pathlib
import sys


def chapter():
    parts = []
    for index in range(1000):
        parts.append(
            f'<p id="p{index}">The reader follows the history of office typography, '
            'café culture, Unicode words and meaningful structure. Each chapter '
            'contains the same searchable phrase and a unique reference '
            f'{index}.</p>'
        )
        if index % 10 == 0:
            parts.append(
                '<ol><li>A structured list item with emphasis and searchable '
                'words.</li><li>Another item.</li></ol>'
            )
    return '<!doctype html><html><body>' + ''.join(parts) + '</body></html>'


if __name__ == '__main__':
    pathlib.Path(sys.argv[1]).write_text(chapter(), encoding='utf-8')
