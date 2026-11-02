#!/usr/bin/env python3
"""Hold the owner turn until a continuation, without inference."""
import json, sys
claude = '-p' in sys.argv
started = False
for line in sys.stdin:
    f = json.loads(line)
    if claude:
        if f['type'] == 'control_request':
            assert f['request']['subtype'] != 'interrupt', 'owner must survive child stop'
            print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}}), flush=True)
            continue
        if f['type'] != 'user': continue
        print(json.dumps({'type':'system','subtype':'init','session_id':'owner'}), flush=True)
        print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'owner-ready\n\n'}}}), flush=True)
        if started:
            print(json.dumps({'type':'result','subtype':'success','session_id':'owner'}), flush=True)
    else:
        m, i = f.get('method'), f.get('id')
        if i is None: continue
        assert m != 'turn/interrupt', 'owner must survive child stop'
        result = {'capabilities':{}} if m == 'initialize' else {'thread':{'id':'owner'}} if m == 'thread/start' else {'turn':{'id':'turn'}} if m == 'turn/start' else {}
        print(json.dumps({'id':i,'result':result}), flush=True)
        if m not in ('turn/start', 'turn/steer'): continue
        print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'owner','turnId':'turn','delta':'owner-ready\n\n'}}), flush=True)
        if started:
            print(json.dumps({'method':'turn/completed','params':{'threadId':'owner','turn':{'id':'turn','status':'completed'}}}), flush=True)
    started = True
