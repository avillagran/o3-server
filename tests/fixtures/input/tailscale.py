#!/usr/bin/python3
"""Synthetic Tailscale argv protocol; all mutable files live in the supplied private fixture."""
import json
import os
import signal
import sys
import time
from pathlib import Path

path = Path(os.environ['O3_TAILSCALE_FIXTURE'])
state = json.loads(path.read_text())
args = sys.argv[1:]
with path.with_suffix('.calls').open('a') as calls:
    calls.write(json.dumps(args) + '\n')
if state.get('sleep'):
    if state.get('fork'):
        child = os.fork()
        if child == 0:
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            path.with_suffix('.descendant').write_text(str(os.getpid()))
            time.sleep(60)
            os._exit(0)
    time.sleep(60)
if state.get('exit'):
    print(state.get('stderr', ''), file=sys.stderr)
    sys.exit(state['exit'])
if args == ['serve', 'status', '--json']:
    print(state.get('raw', json.dumps(state.get('serve'))))
elif args == ['status', '--json']:
    print(json.dumps(state.get('status', {})))
elif args and args[-1] == 'off':
    if not state.get('offPersists'):
        config = state.get('serve') or {}
        tcp = next((arg.split('=', 1)[1] for arg in args if arg.startswith('--tcp=')), None)
        mount = next((arg.split('=', 1)[1] for arg in args if arg.startswith('--set-path=')), None)
        port = next((arg.split('=', 1)[1] for arg in args if arg.startswith('--https=')), None)
        if tcp is not None:
            config.get('TCP', {}).pop(tcp, None)
        if mount is not None:
            current = state.get('status', {}).get('Self', {}).get('DNSName', '').strip().removesuffix('.').lower()
            for host_port, site in config.get('Web', {}).items():
                host, _, site_port = host_port.rpartition(':')
                if site_port == port and host.lower() == current:
                    site.get('Handlers', {}).pop(mount, None)
        state['serve'] = config
        path.write_text(json.dumps(state))
else:
    print('Unsupported synthetic command', file=sys.stderr)
    sys.exit(2)
