#!/usr/bin/env python3
"""Record synthetic ADB argv and return oracle inputs; never contact a device."""
import json
import os
import pathlib
import sys

args = sys.argv[1:]
with pathlib.Path(os.environ['O3_DEVICE_ACTION_TRACE']).open('a') as stream:
    stream.write(json.dumps({'command': 'adb', 'args': args}) + '\n')
if args == ['devices', '-l']:
    print('List of devices attached\nemulator-5554\tdevice model:Synthetic')
    sys.exit(0)
if args[2:] == ['get-state']:
    print('device')
    sys.exit(0)
spec = json.loads(pathlib.Path(os.environ['O3_DEVICE_ACTION_CASE']).read_text()).get('spec', {})
reads = {'night': 'Night mode: yes\n', 'font': '1.15\n', 'animator': '0\n', 'wifi': '1\n',
         'focus': 'mFocusedApp=ActivityRecord{abcd u12 com.example.app/.MainActivity t12}\n'}
reads.update(spec.get('reads', {}))
key = ' '.join(args[2:])
read_commands = {'shell cmd uimode night': 'night',
                 'shell settings get system font_scale': 'font',
                 'shell settings get global animator_duration_scale': 'animator',
                 'shell settings get global wifi_on': 'wifi',
                 'shell dumpsys window': 'focus'}
if key in read_commands:
    value = reads[read_commands[key]]
    if value is None:
        print('synthetic command failure', file=sys.stderr)
        sys.exit(1)
    sys.stdout.write(value)
elif key == 'shell wm size':
    if spec.get('failSize'):
        print('synthetic command failure', file=sys.stderr)
        sys.exit(1)
    sys.stdout.write(spec.get('size', 'Physical size: 1080x1920\n'))
elif args[-1] in spec.get('failPermissions', []):
    print('synthetic command failure', file=sys.stderr)
    sys.exit(1)
