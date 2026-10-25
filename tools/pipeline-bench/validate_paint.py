#!/usr/bin/env python3
"""Compare retained paint with a frozen full-cascade engine on synthetic fixtures."""
import argparse
import json
import subprocess
from pathlib import Path

parser=argparse.ArgumentParser()
parser.add_argument('--before',required=True)
parser.add_argument('--after',required=True)
parser.add_argument('--fixtures',required=True)
parser.add_argument('--output',required=True)
args=parser.parse_args()
rows=[]
for fixture in sorted(Path(args.fixtures).glob('*.html')):
    for palette in ['default','foreground','theme']:
        def run(binary,case):
            proc=subprocess.run(['taskset','-c','3',binary,str(fixture.resolve()),case,'1',palette],capture_output=True,text=True,check=True)
            return json.loads(proc.stdout)
        before,after=run(args.before,'reference'),run(args.after,'cold')
        for key in ['glyphs','lines','render_fingerprint','source_fingerprint','text_color_fingerprint']:
            assert before[key]==after[key],(fixture.name,palette,key,before[key],after[key])
        row={'fixture':fixture.name,'palette':palette,'matches':True,'reference':before,'candidate':after}
        rows.append(row)
        Path(args.output).write_text(json.dumps(rows,indent=2)+'\n')
        print(f'{fixture.name} {palette}: matches',flush=True)
