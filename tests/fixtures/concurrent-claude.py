#!/usr/bin/env python3
"""Gated Claude control windows; isolated scratch profile only."""
import json, os, sys, time
from pathlib import Path
if '--version' in sys.argv:
    print('2.1.316 (Claude Code)')
    sys.exit(0)
root = Path(os.environ['T3CODE_HOME'])
started = False
def send(v): print(json.dumps(v), flush=True)
def gate(name):
    end = time.monotonic() + 30
    while not (root / name).exists():
        if time.monotonic() > end: raise RuntimeError(name)
        time.sleep(.01)
for line in sys.stdin:
    f = json.loads(line)
    if f['type'] == 'control_request':
        subtype = f['request']['subtype']
        if subtype == 'initialize' and (root / 'gate-init').exists():
            (root / 'init-received').touch()
            gate('release-init')
        if subtype == 'interrupt':
            if (root / 'die-interrupt').exists(): os._exit(7)
            (root / 'interrupt-received').touch()
            gate('release-interrupt')
        send(dict(type='control_response', response=dict(request_id=f['request_id'], subtype='success')))
        if subtype == 'interrupt':
            send(dict(type='result', subtype='error_during_execution', is_error=True, result='interrupted', session_id='native-fixture'))
    elif f['type'] == 'user':
        if started:
            (root / 'steer-received').touch()
            gate('release-steer')
        else:
            started = True
            send(dict(type='system', subtype='init', session_id='native-fixture'))
            (root / 'started-1').touch()
