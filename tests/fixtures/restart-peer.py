#!/usr/bin/python3
"""Native peer with delayed interrupt, stale output, repeated item IDs and same-session resume."""
import json,sys,os,pathlib,threading,time
lock=threading.Lock();claude='-p' in sys.argv;root=pathlib.Path.cwd();config=None
if claude:config=json.load(open(sys.argv[sys.argv.index('--mcp-config')+1]))['mcpServers']['t3-code']
threads={};next_thread=0

def send(v):
    with lock:print(json.dumps(v),flush=True)
def capture(root,v):
    with os.fdopen(os.open(root/'restart-observe.json',os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600),'w') as f:json.dump(v,f)
def finish_interrupt(root,native,turn):
    (root/'interrupt-seen').touch()
    while (root/'hold-interrupt').exists() and not (root/'release-interrupt').exists():time.sleep(.01)
    send({'method':'item/agentMessage/delta','params':{'threadId':native,'turnId':turn,'delta':'STALE-OLD\n\n'}})
    send({'method':'item/completed','params':{'threadId':native,'turnId':turn,'item':{'id':'same','type':'commandExecution','command':'old command','status':'completed','exitCode':0,'aggregatedOutput':'STALE-OUTPUT'}}})
    send({'method':'item/commandExecution/requestApproval','id':900,'params':{'threadId':native,'turnId':turn,'itemId':'late','command':'STALE-TOOL'}})
    send({'method':'turn/completed','params':{'threadId':native,'turn':{'id':turn,'status':'interrupted'}}})
for line in sys.stdin:
    v=json.loads(line);m=v.get('method');i=v.get('id');p=v.get('params',{})
    if claude:
        if v['type']=='control_request':send({'type':'control_response','response':{'request_id':v['request_id'],'subtype':'success'}})
        elif v['type']=='user':
            capture(root,{'config':config,'pid':os.getpid(),'native':'native'})
            send({'type':'system','subtype':'init','session_id':'native'})
            send({'type':'assistant','message':{'id':'message-1','role':'assistant','content':[{'type':'text','text':'initial\n\n'}]}})
    elif m in ('thread/start','thread/resume'):
        native=p.get('threadId')
        if native is None:next_thread+=1;native='native-'+str(next_thread)
        old=threads.get(native,{})
        threads[native]={'root':pathlib.Path(p['cwd']),'config':p['config']['mcp_servers.t3-code'],'n':old.get('n',0)}
        send({'id':i,'result':{'thread':{'id':native}}})
    elif m=='turn/start':
        native=p['threadId'];entry=threads[native];entry['n']+=1;turn='turn-'+native+'-'+str(entry['n']);entry['turn']=turn;root=entry['root']
        text=p['input'][0]['text'];capture(root,{'config':entry['config'],'pid':os.getpid(),'native':native,'input':p['input'],'turn':turn})
        with open(root/'starts.jsonl','a') as f:f.write(json.dumps({'turn':turn,'input':p['input'],'model':p.get('model')})+'\n')
        send({'id':i,'result':{'turn':{'id':turn}}})
        send({'method':'item/agentMessage/delta','params':{'threadId':native,'turnId':turn,'delta':('initial' if entry['n']==1 else 'replacement')+'\n\n'}})
        send({'method':'item/started','params':{'threadId':native,'turnId':turn,'item':{'id':'same','type':'commandExecution','command':'fixture command','status':'inProgress'}}})
        if not text.startswith('hold'):
            send({'method':'item/completed','params':{'threadId':native,'turnId':turn,'item':{'id':'same','type':'commandExecution','command':'fixture command','status':'completed','exitCode':0,'aggregatedOutput':'NEW-OUTPUT'}}})
            send({'method':'turn/completed','params':{'threadId':native,'turn':{'id':turn,'status':'completed'}}})
    elif m=='turn/interrupt':
        entry=threads[p['threadId']]
        if (entry['root']/'fail-interrupt').exists():send({'id':i,'error':{'code':-32000,'message':'injected interrupt failure'}})
        else:
            send({'id':i,'result':{}})
            threading.Thread(target=finish_interrupt,args=(entry['root'],p['threadId'],p['turnId']),daemon=True).start()
    elif m=='thread/unsubscribe':send({'id':i,'result':{}})
    elif i is not None:send({'id':i,'result':{}})
