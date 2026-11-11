#!/usr/bin/env python3
"""Reject foreign native IDs and require the previous turn's transcript."""
import json, sys
claude = '-p' in sys.argv
native = sys.argv[sys.argv.index('--session-id')+1] if '--session-id' in sys.argv else 'claude-native'
if claude:
    assert '--resume' not in sys.argv
for line in sys.stdin:
    f = json.loads(line)
    if claude:
        if f['type'] == 'control_request':
            print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}}), flush=True)
            continue
        if f['type'] != 'user': continue
        text = f['message']['content'][0]['text']
    else:
        m, i = f.get('method'), f.get('id')
        if i is None: continue
        assert m not in ('thread/resume', 'thread/fork')
        if m == 'initialize': result = {'capabilities':{}}
        elif m == 'thread/start': result = {'thread':{'id':'codex-native'}}
        elif m == 'thread/inject_items':
            print(json.dumps({'id':i,'error':{'code':-32601,'message':'history injection unavailable'}}), flush=True)
            continue
        elif m == 'turn/start':
            text = f['params']['input'][0]['text']
            result = {'turn':{'id':'turn'}}
        else: result = {}
        print(json.dumps({'id':i,'result':result}), flush=True)
        if m != 'turn/start': continue
    if 'continue-proof' in text:
        assert 'seed-proof' in text, text
        assert 'remembered-proof' in text, text
    else: assert text == 'seed-proof', text
    if claude:
        print(json.dumps({'type':'system','subtype':'init','session_id':native}), flush=True)
        print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'remembered-proof'}}}), flush=True)
        print(json.dumps({'type':'result','subtype':'success','session_id':native}), flush=True)
    else:
        print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'codex-native','turnId':'turn','delta':'remembered-proof'}}), flush=True)
        print(json.dumps({'method':'turn/completed','params':{'threadId':'codex-native','turn':{'id':'turn','status':'completed'}}}), flush=True)
