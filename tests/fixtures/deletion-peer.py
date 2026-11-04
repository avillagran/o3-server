#!/usr/bin/python3
"""Bounded native peer: independent idle/waiting sessions on a shared Codex process."""
import json,sys,os,pathlib,threading,time
output_lock=threading.Lock()
claude='-p' in sys.argv
root=pathlib.Path.cwd()
config=None
if claude:
    config=json.load(open(sys.argv[sys.argv.index('--mcp-config')+1]))['mcpServers']['t3-code']
threads={};pending={};next_thread=0;next_request=100
def send(v):
    with output_lock:print(json.dumps(v),flush=True)
def label(text):
    labels=('idle-cached','idle-switched','wait-target','wait-survivor')
    return max(labels,key=lambda name:text.rfind(name))
def capture(root,text,config,native):
    with os.fdopen(os.open(root/('observe-'+text+'.json'),os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600),'w') as f:
        json.dump({'config':config,'native':native,'pid':os.getpid()},f)
def bind_later(root,reply):
    while not (root/'release-binding').exists():time.sleep(.01)
    send(reply)
def terminal(native,status='completed'):
    if claude:send({'type':'result','subtype':'success','session_id':native})
    else:send({'method':'turn/completed','params':{'threadId':native,'turn':{'id':'turn-'+native,'status':status}}})
for line in sys.stdin:
    v=json.loads(line);m=v.get('method');i=v.get('id');p=v.get('params',{})
    if claude:
        if v['type']=='control_request':
            send({'type':'control_response','response':{'request_id':v['request_id'],'subtype':'success'}})
        elif v['type']=='control_response':terminal('native')
        elif v['type']=='user':
            text=label(v['message']['content'][0]['text']);capture(root,text,config,'native')
            send({'type':'system','subtype':'init','session_id':'native'})
            if text.startswith('idle'):terminal('native')
            else:send({'type':'control_request','request_id':'approval','request':{'subtype':'can_use_tool','tool_name':'Bash','input':{'command':'fixture-only'}}})
    elif m in ('thread/start','thread/resume'):
        next_thread+=1;native='native-'+str(next_thread)
        threads[native]={'root':pathlib.Path(p['cwd']),'config':p['config']['mcp_servers.t3-code']}
        reply={'id':i,'result':{'thread':{'id':native}}}
        if (threads[native]['root']/'hold-binding').exists():
            capture(threads[native]['root'],'binding-pending',threads[native]['config'],native)
            threading.Thread(target=bind_later,args=(threads[native]['root'],reply),daemon=True).start()
        else:send(reply)
    elif m=='turn/start':
        native=p['threadId'];text=label(p['input'][0]['text']);entry=threads[native]
        entry['label']=text;capture(entry['root'],text,entry['config'],native)
        send({'id':i,'result':{'turn':{'id':'turn-'+native}}})
        if text.startswith('idle'):terminal(native)
        else:
            next_request+=1;request=next_request;pending[request]=native
            send({'id':request,'method':'item/commandExecution/requestApproval','params':{'threadId':native,'turnId':'turn-'+native,'itemId':'approval-'+native,'command':'fixture-only','cwd':str(entry['root'])}})
    elif m=='thread/unsubscribe':
        native=p['threadId'];entry=threads.get(native)
        if entry and entry.get('label')=='wait-target' and not (entry['root']/'unload-retried').exists():
            (entry['root']/'unload-retried').touch()
            send({'id':i,'error':{'code':-32000,'message':'fixture transient unload failure'}})
            continue
        if entry:
            with open(entry['root']/'unloaded.jsonl','a') as f:f.write(json.dumps(native)+'\n')
            threads.pop(native,None)
        send({'id':i,'result':{}})
    elif m=='turn/interrupt':send({'id':i,'result':{}});terminal(p['threadId'],'interrupted')
    elif i in pending and m is None:terminal(pending.pop(i))
    elif i is not None:send({'id':i,'result':{}})
