#!/usr/bin/env python3
"""Verify complete native-oracle prompts on real Codex and Claude transports."""
import json
import sys
from pathlib import Path

fixture = json.loads((Path(__file__).resolve().parents[1] / 'composer-context.json').read_text())
expected = {case['expected'] for case in fixture['projections']}
claude = '-p' in sys.argv
initialized = False
turn = 0
for line in sys.stdin:
    frame = json.loads(line)
    if claude:
        if frame['type'] == 'control_request':
            print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':frame['request_id']}}), flush=True)
            continue
        if frame['type'] != 'user':
            continue
        text = frame['message']['content'][0]['text']
        if not initialized:
            print(json.dumps({'type':'system','subtype':'init','session_id':'composer-native'}), flush=True)
            initialized = True
    else:
        method, ident = frame.get('method'), frame.get('id')
        if ident is None:
            continue
        result = {}
        if method in ('thread/start','thread/resume'):
            result = {'thread':{'id':'composer-native'}}
        if method == 'turn/start':
            turn += 1
            result = {'turn':{'id':f'turn-{turn}'}}
        print(json.dumps({'id':ident,'result':result}), flush=True)
        if method not in ('turn/start','turn/steer'):
            continue
        text = frame['params']['input'][0]['text']
    if text == 'hold':
        continue
    assert text in expected, f'Provider prompt did not match pinned native oracle: {text!r}'
    assert 'Additional context supplied by the user:' not in text
    if claude:
        print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'verified-composer'}}}), flush=True)
        print(json.dumps({'type':'result','subtype':'success','session_id':'composer-native'}), flush=True)
    else:
        print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'composer-native','turnId':f'turn-{turn}','delta':'verified-composer'}}), flush=True)
        print(json.dumps({'method':'turn/completed','params':{'threadId':'composer-native','turn':{'id':f'turn-{turn}','status':'completed'}}}), flush=True)
