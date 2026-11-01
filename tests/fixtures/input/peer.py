#!/usr/bin/env python3
"""Assert native inputs, then return one cheap deterministic result."""
import base64, json, sys
from pathlib import Path
claude = '-p' in sys.argv
for line in sys.stdin:
    f = json.loads(line)
    if claude:
        if f['type'] == 'control_response' and f['response']['request_id'] == 'exit-plan':
            assert f['response']['response']['behavior'] == 'deny'
            print(json.dumps({'type':'result','subtype':'success','session_id':'native','usage':{}}),flush=True)
            continue
        if f['type'] == 'control_response':
            assert f['response']['response']['updatedInput']['answers'] == {'Which approach?':'Simple, Safe'}
            print(json.dumps({'type':'result','subtype':'success','session_id':'native','usage':{}}),flush=True)
            continue
        if f['type'] == 'control_request':
            print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}}), flush=True)
            continue
        content = f['message']['content']
        if content[0]['text'] == 'plan-proof':
            assert '--permission-mode' in sys.argv and sys.argv[sys.argv.index('--permission-mode')+1] == 'plan'
            print(json.dumps({'type':'control_request','request_id':'exit-plan','request':{'subtype':'can_use_tool','tool_name':'ExitPlanMode','input':{'plan':'# Test plan\nImplement safely.'}}}),flush=True)
            continue
        if content[0]['text'].startswith('PLEASE IMPLEMENT THIS PLAN:'):
            print(json.dumps({'type':'result','subtype':'success','session_id':'native','usage':{}}),flush=True)
            continue
        if content[0]['text'] == 'question-proof':
            print(json.dumps({'type':'control_request','request_id':'question','request':{'subtype':'can_use_tool','tool_name':'AskUserQuestion','input':{'questions':[{'question':'Which approach?','header':'Approach','multiSelect':True,'options':[{'label':'Simple','description':'Fewer parts'},{'label':'Safe','description':'Keep setup'}]}]}}}),flush=True)
            continue
    else:
        m, i = f.get('method'), f.get('id')
        if m == 'initialized': continue
        if m == 'initialize': result = {'capabilities':{}}
        elif m == 'model/list': result = {'data':[]}
        elif m == 'account/read': result = {'account':None}
        elif m in ('thread/start','thread/resume'): result = {'thread':{'id':'native'}}
        elif m == 'turn/start': content = f['params']['input']
        else:
            print(json.dumps({'id':i,'error':{'code':-32601,'message':'unused'}}),flush=True)
            continue
        if m != 'turn/start':
            print(json.dumps({'id':i,'result':result}),flush=True)
            continue
    if not claude and content[0]['text'] == 'plan-proof':
        assert f['params']['collaborationMode']['mode'] == 'plan'
        print(json.dumps({'id':i,'result':{'turn':{'id':'turn'}}}),flush=True)
        print(json.dumps({'method':'item/completed','params':{'threadId':'native','turnId':'turn','item':{'id':'plan','type':'plan','text':'# Test plan\nImplement safely.'}}}),flush=True)
        print(json.dumps({'method':'turn/completed','params':{'threadId':'native','turn':{'id':'turn','status':'completed'}}}),flush=True)
        continue
    if not claude and content[0]['text'].startswith('PLEASE IMPLEMENT THIS PLAN:'):
        assert 'collaborationMode' not in f['params'] or f['params']['collaborationMode']['mode'] != 'plan'
        print(json.dumps({'id':i,'result':{'turn':{'id':'turn2'}}}),flush=True)
        print(json.dumps({'method':'turn/completed','params':{'threadId':'native','turn':{'id':'turn2','status':'completed'}}}),flush=True)
        continue
    if claude:
        assert sys.argv[sys.argv.index('--effort')+1] == 'high'
        assert json.loads(sys.argv[sys.argv.index('--settings')+1])['fastMode'] is True
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
        print(json.dumps({'type':'rate_limit_event','rate_limit_info':{'unifiedWindows':{'five_hour':{'utilization':0.12,'resetsAt':1793491200},'seven_day':{'utilization':0.34,'resetsAt':1793944800}}}}),flush=True)
        print(json.dumps({'type':'system','subtype':'init','session_id':'native','slash_commands':['compact'],'skills':['test-skill'],'claude_code_version':'fixture'}),flush=True)
        print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'verified-input'}}}),flush=True)
        print(json.dumps({'type':'result','subtype':'success','session_id':'native','usage':{}}),flush=True)
    else:
        print(json.dumps({'id':i,'result':{'turn':{'id':'turn'}}}),flush=True)
        print(json.dumps({'method':'item/agentMessage/delta','params':{'threadId':'native','turnId':'turn','delta':'verified-input'}}),flush=True)
        print(json.dumps({'method':'turn/completed','params':{'threadId':'native','turn':{'id':'turn','status':'completed'}}}),flush=True)
