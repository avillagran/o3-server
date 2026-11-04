#!/usr/bin/env python3
"""Executable native protocol peer: anchored forks, history injection and CLI resume."""
import json, os, sys, uuid, re, hashlib, urllib.request, urllib.error
claude = '-p' in sys.argv
native = None
cwd = os.getcwd()
cwd_known = claude
model = ''
ambiguous = 0
unload_failures = 0
injected = {}
credentials = {}
turns_by_native = {}
retired = None
if claude:
    model = sys.argv[sys.argv.index('--model')+1] if '--model' in sys.argv else ''
    if '--resume' in sys.argv: native = sys.argv[sys.argv.index('--resume')+1]
    if '--fork-session' in sys.argv or native is None:
        native = sys.argv[sys.argv.index('--session-id')+1] if '--session-id' in sys.argv else str(uuid.uuid4())
def emit(v): print(json.dumps(v), flush=True)
def log(v):
    value=json.loads(json.dumps(v))
    def redact(node):
        if isinstance(node,dict):
            for k in list(node):
                if k.lower() in ('authorization','api_key','access_token'):node[k]='[redacted]'
                else:redact(node[k])
        elif isinstance(node,list):
            for child in node:redact(child)
    auth=v.get('params',{}).get('config',{}).get('mcp_servers.t3-code',{}).get('http_headers',{}).get('Authorization')
    if auth:value['_mcpDigest']=hashlib.sha256(auth.encode()).hexdigest()
    redact(value)
    with open(os.path.join(cwd,'transfer-trace.jsonl'),'a') as out: out.write(json.dumps(value)+'\n')
for line in sys.stdin:
    f=json.loads(line)
    if claude:
        if f['type']=='control_request':
            if f['request']['subtype']=='initialize' and '--resume' in sys.argv and model in ('resume-missing','resume-in-use'):
                log({'method':'claude/resume-refused','native':native,'model':model})
                emit({'type':'control_response','response':{'subtype':'error','request_id':f['request_id'],'error':'Thread is in use by another client' if model=='resume-in-use' else 'No conversation found'}});sys.exit(1)
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
        if method=='thread/resume' and p.get('model') in ('resume-missing','resume-in-use'):
            emit({'id':i,'error':{'code':-32600 if p['model']=='resume-in-use' else -32000,'message':'Thread already has an active writer' if p['model']=='resume-in-use' else 'Thread not found'}});continue
        if method=='thread/fork' and p.get('lastTurnId') and p['lastTurnId'] not in turns_by_native.get(p['threadId'],[]):
            emit({'id':i,'error':{'code':-32602,'message':'Source turn no longer belongs to the native conversation'}});continue
        native=p.get('threadId') if method=='thread/resume' else str(uuid.uuid4())
        model=p.get('model','')
        config=p.get('config',{}).get('mcp_servers.t3-code',{})
        if config.get('url'):
            credentials[native]=config
            if retired in credentials and method=='thread/start':
                old=credentials[retired]
                request=urllib.request.Request(old['url'],data=json.dumps({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-06-18','capabilities':{},'clientInfo':{'name':'retired-test','version':'1'}}}).encode(),headers={'Authorization':old['http_headers']['Authorization'],'Content-Type':'application/json','Accept':'application/json'})
                try:
                    with urllib.request.urlopen(request,timeout=3) as reply:status=reply.status
                except urllib.error.HTTPError as error:status=error.code
                log({'method':'retired-credential-probe','status':status})
        result={'thread':{'id':native}}
    elif method=='thread/inject_items':
        if model=='inject-inline': emit({'id':i,'error':{'code':-32601,'message':'method absent'}});continue
        item_ids={match.group(1) for item in p['items'] for block in item['content'] if (match:=re.search(r'; item=([^;]+); provider-thread=',block.get('text','')))}
        assert not item_ids.intersection(injected.get(p['threadId'],set())), 'history replayed in the same native conversation'
        injected.setdefault(p['threadId'],set()).update(item_ids)
        if model.startswith('inject-ambiguous') and ambiguous==0:
            ambiguous+=1;emit({'id':i,'error':{'code':-32000,'message':'unknown delivery outcome'}});continue
        result={}
    elif method=='thread/unsubscribe':
        if model=='inject-ambiguous-unload-error' and unload_failures==0:
            unload_failures+=1;emit({'id':i,'error':{'code':-32000,'message':'unload temporarily failed'}});continue
        retired=p['threadId'];result={}
    elif method=='turn/start':
        if p['input'][0]['text']=='REJECT-ME':emit({'id':i,'error':{'code':-32602,'message':'known pre-admission refusal'}});continue
        result={'turn':{'id':str(uuid.uuid4())}}
        turns_by_native.setdefault(p['threadId'],[]).append(result['turn']['id'])
    else:result={}
    emit({'id':i,'result':result})
    if method=='turn/start':
        text=p['input'][0]['text'].split('User message:\n')[-1]
        turn=result['turn']['id']
        emit({'method':'item/agentMessage/delta','params':{'threadId':p['threadId'],'turnId':turn,'delta':'native-response:'+text}})
        emit({'method':'turn/completed','params':{'threadId':p['threadId'],'turn':{'id':turn,'status':'completed'}}})
