#!/usr/bin/env python3
"""Deterministic native peer: two permissions withdrawn before the result frame."""
import json
import sys
import time
from pathlib import Path


def emit(frame):
    print(json.dumps(frame), flush=True)


def await_marker(name):
    deadline = time.monotonic() + 12
    while not Path(name).exists():
        if time.monotonic() > deadline:
            raise RuntimeError('fixture barrier timed out: ' + name)
        time.sleep(0.01)


for line in sys.stdin:
    frame = json.loads(line)
    if frame['type'] == 'control_request':
        emit({'type': 'control_response', 'response': {'subtype': 'success', 'request_id': frame['request_id']}})
    elif frame['type'] == 'user':
        question = frame['message']['content'][0]['text'] == 'withdraw-question'
        emit({'type': 'system', 'subtype': 'init', 'session_id': 'withdrawal-fixture'})
        for number in (1, 2):
            emit({'type': 'control_request', 'request_id': 'native-' + str(number), 'request': {
                'subtype': 'can_use_tool', 'tool_name': 'AskUserQuestion' if question else 'Bash',
                'input': {'questions': [{'question': 'Which target?', 'header': 'Target', 'options': [{'label': 'Local', 'description': 'Fixture only'}], 'multiSelect': False}]} if question else {'command': 'printf fixture'}
            }})
        await_marker('withdraw-first')
        for request in ('unknown', 'native-1', 'native-1'):
            emit({'type': 'control_cancel_request', 'request_id': request})
        await_marker('withdraw-last')
        emit({'type': 'control_cancel_request', 'request_id': 'native-2'})
        emit({'type': 'result', 'subtype': 'success', 'session_id': 'withdrawal-fixture'})
    else:
        raise RuntimeError('fixture must not receive a response to a withdrawn request')
