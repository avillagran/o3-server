#!/usr/bin/env python3
"""Resumed CLI flushes an unrelated result before answering the submitted prompt."""
import json
import sys


def emit(value):
    print(json.dumps(value), flush=True)


for line in sys.stdin:
    frame = json.loads(line)
    if frame['type'] == 'control_request':
        emit({'type': 'control_response', 'response': {'subtype': 'success', 'request_id': frame['request_id']}})
    elif frame['type'] == 'user':
        emit({'type': 'system', 'subtype': 'init', 'session_id': 'notification-fixture'})
        emit({'type': 'result', 'subtype': 'success', 'is_error': False,
              'origin': {'kind': 'task-notification'}, 'num_turns': 0, 'result': '',
              'usage': {'input_tokens': 0, 'output_tokens': 0}})
        emit({'type': 'stream_event', 'event': {'type': 'content_block_delta',
              'delta': {'type': 'text_delta', 'text': 'ACTUAL_PROMPT_REPLY'}}})
        emit({'type': 'result', 'subtype': 'success', 'origin': {'kind': 'human'},
              'num_turns': 1, 'result': 'ACTUAL_PROMPT_REPLY',
              'usage': {'input_tokens': 10, 'output_tokens': 2}})
