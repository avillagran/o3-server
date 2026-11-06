#!/usr/bin/python3
"""Private systemd command peer; never contacts a service manager or real cgroup."""
import json
import os
import pathlib
import sys

root = pathlib.Path(sys.argv[0]).parent
args = sys.argv[1:]
with (root / 'calls.jsonl').open('a') as log:
    log.write(json.dumps({'tool': pathlib.Path(sys.argv[0]).name, 'args': args,
                          'oomScore': pathlib.Path('/proc/self/oom_score_adj').read_text().strip(),
                          'runtime': os.environ.get('XDG_RUNTIME_DIR')}) + '\n')
if pathlib.Path(sys.argv[0]).name == 'systemd-run':
    if (root / 'probe-disabled').exists():
        sys.exit(1)
    if (root / 'probe-no-oom').exists() and any(arg.startswith('--property=OOMPolicy=') for arg in args):
        sys.exit(1)
    launch = args[args.index('--') + 1:]
    os.execvp(launch[0], launch)
verb = args[1]
unit = args[2] if len(args) > 2 else ''
if verb == 'show':
    units = json.loads((root / 'units.json').read_text())
    state = units.get(unit, {'LoadState': 'not-found', 'ActiveState': 'inactive', 'Result': 'success'})
    for key, value in state.items():
        print(f'{key}={value}')
elif verb == 'stop' and (root / 'fail-stop').exists():
    sys.exit(5)
