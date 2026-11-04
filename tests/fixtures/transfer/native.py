#!/usr/bin/env python3
"""Executable native protocol peer: anchored forks, history injection and CLI resume."""
import json, os, sys, uuid
claude = '-p' in sys.argv
native = None
cwd = os.getcwd()
cwd_known = claude
model = ''
if claude:
    if '--resume' in sys.argv: native = sys.argv[sys.argv.index('--resume')+1]
    if '--fork-session' in sys.argv or native is None:
        native = sys.argv[sys.argv.index('--session-id')+1] if '--session-id' in sys.argv else str(uuid.uuid4())
def emit(v): print(json.dumps(v), flush=True)
def log(v):
    with open(os.path.join(cwd,'transfer-trace.jsonl'),'a') as out: out.write(json.dumps(v)+'\n')
for line in sys.stdin:
    f=json.loads(line)
    if claude:
        if f['type']=='control_request':
            emit({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}});continue
        if f['type']!='user':continue
        log({'method':'claude/user','native':native,'input':f['message']['content'],'argv':sys.argv})
        text=f['message']['content'][0]['text'].split('User message:\n')[-1]
        emit({'type':'system','subtype':'init','session_id':native})
        emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'native-response:'+text}}})
        emit({'type':'result','subtype':'success','session_id':native});continue
    method, i, p=f.get('method'),f.get('id'),f.get('params',{})
    if i is None: continue
    if p.get('cwd'):cwd=p['cwd'];cwd_known=True
    if cwd_known:log(f)
    if method=='initialize': result={'capabilities':{}}
    elif method in ('thread/start','thread/fork','thread/resume'):
        native=p.get('threadId') if method=='thread/resume' else str(uuid.uuid4())
        model=p.get('model','')
        result={'thread':{'id':native}}
    elif method=='thread/inject_items':
        if model=='inject-inline': emit({'id':i,'error':{'code':-32601,'message':'method absent'}});continue
        if model=='inject-ambiguous': emit({'id':i,'error':{'code':-32000,'message':'unknown delivery outcome'}});continue
        result={}
    elif method=='turn/start':result={'turn':{'id':str(uuid.uuid4())}}
    else:result={}
    emit({'id':i,'result':result})
    if method=='turn/start':
        text=p['input'][0]['text'].split('User message:\n')[-1]
        turn=result['turn']['id']
        emit({'method':'item/agentMessage/delta','params':{'threadId':p['threadId'],'turnId':turn,'delta':'native-response:'+text}})
        emit({'method':'turn/completed','params':{'threadId':p['threadId'],'turn':{'id':turn,'status':'completed'}}})
