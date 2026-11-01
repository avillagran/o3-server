#!/usr/bin/env python3
"""Assert native inputs, then return one cheap deterministic result."""
import base64, json, sys
from pathlib import Path
claude = '-p' in sys.argv
for line in sys.stdin:
    f = json.loads(line)
    if claude:
        if f['type'] == 'control_request':
            print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}}), flush=True)
            continue
        content = f['message']['content']
    else:
        m, i = f.get('method'), f.get('id')
        if m == 'initialized': continue
        if m == 'initialize': result = {'capabilities':{}}
        elif m == 'model/list': result = {'data':[]}
        elif m == 'account/read': result = {'account':None}
        elif m == 'thread/start': result = {'thread':{'id':'native'}}
        elif m == 'turn/start': content = f['params']['input']
        else:
            print(json.dumps({'id':i,'error':{'code':-32601,'message':'unused'}}),flush=True)
            continue
        if m != 'turn/start':
            print(json.dumps({'id':i,'result':result}),flush=True)
            continue
    assert 'context-proof' in content[0]['text']
    assert content[0]['text'].startswith('input-proof')
    image = content[1]
    if claude:
        assert image['type'] == 'image'
        assert base64.b64decode(image['source']['data']).startswith(b'\x89PNG')
    else:
        assert image['type'] == 'localImage'
        assert Path(image['path']).read_bytes().startswith(b'\x89PNG')
    assert 'notes.txt' in content[2]['text']
    # File path is JSON quoted at the end of the attachment text.
    path = json.JSONDecoder().raw_decode(content[2]['text'].split('available at ',1)[1])[0]
    assert Path(path).read_text() == 'file-proof'
    if claude:
        print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'verified-input'}}}),flush=True)
        print(json.dumps({'type':'result','subtype':'success','session_id':'native','usage':{}}),flush=True)
    else:
        print(json.dumps({'id':i,'result':{'turn':{'id':'turn'}}}),flush=True)
        print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'native','turnId':'turn','delta':'verified-input'}}),flush=True)
        print(json.dumps({'method':'turn/completed','params':{'threadId':'native','turn':{'id':'turn','status':'completed'}}}),flush=True)
