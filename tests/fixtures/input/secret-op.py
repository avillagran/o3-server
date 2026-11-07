#!/usr/bin/python3
"""Private synthetic op transport; invoked only inside the rootless test container."""
import json
import os
from pathlib import Path
import re
import signal
import stat
import sys

root = Path(os.path.abspath(sys.argv[0])).parent
case = json.loads((root / 'case.json').read_text())
args = sys.argv[1:]
record = {'args': args, 'pid': os.getpid()}
if len(args) > 1 and args[1] == 'inject':
    template_path = Path(args[args.index('-i') + 1])
    template = template_path.read_text()
    record['templateMode'] = stat.S_IMODE(template_path.stat().st_mode)
    record['template'] = re.sub(r'__t3-secret-[a-f0-9-]+__', 'SEPARATOR', template)
    result = case.get('inject', {})
else:
    result = case.get('read', {}).get(args[-1], {})
with (root / 'calls.jsonl').open('a') as output:
    output.write(json.dumps(record) + '\n')
    output.flush()
if result.get('hang'):
    while True:
        signal.pause()
if result.get('signal'):
    os.kill(os.getpid(), signal.SIGTERM)
if 'substitute' in result:
    stdout = re.sub(r'\{\{ (.*?) \}\}', lambda match: result['substitute'].get(match[1], ''), template)
    stdout += result.get('trailing', '')
else:
    stdout = result.get('stdout', '')
sys.stdout.write(stdout)
sys.stderr.write(result.get('stderr', ''))
sys.exit(result.get('code', 0))
