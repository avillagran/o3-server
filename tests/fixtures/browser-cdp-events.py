#!/usr/bin/python3
"""Owned pipe peer with independent pages and delayed unsolicited CDP notifications."""
import json, os, sys, threading, time
profile=next(arg.split('=',1)[1] for arg in sys.argv if arg.startswith('--user-data-dir='))
with open(profile+'/fixture.pid','w') as output:output.write(str(os.getpid()))
state_lock=threading.Lock(); output_lock=threading.Lock(); pages={}; sessions={}; sequence=0

def write(message):
    data=json.dumps(message).encode()+b'\0'
    with output_lock:
        # Deliberate fragmentation exercises persistent framing across idle/request boundaries.
        for offset in range(0,len(data),7):
            chunk=data[offset:offset+7]
            while chunk:
                count=os.write(4,chunk);chunk=chunk[count:]

def later(session,params):
    time.sleep(0.06)
    with state_lock:
        target=sessions[session];page=pages[target]
        if params.get('close'):
            page['closed']=True
        else:
            page.update(url=params.get('url',page['url']),title=params.get('title',page['title']))
    if params.get('close'):
        write({'method':'Target.detachedFromTarget','params':{'sessionId':session,'targetId':target}})
        write({'method':'Target.targetDestroyed','params':{'targetId':target}})
    else:
        route='unrelated-session' if params.get('wrongSession') else session
        write({'method':'Page.frameStartedLoading','sessionId':route,'params':{'frameId':target+'-frame'}})
        write({'method':'Page.frameNavigated','sessionId':route,'params':{'frame':{'id':target+'-frame','url':page['url']}}})
        write({'method':'Page.loadEventFired','sessionId':route,'params':{'timestamp':1}})
    with open(profile+'/event-sent','w') as output:output.write(str(params.get('serial',0)))

buffer=b''
while True:
    chunk=os.read(3,4096)
    if not chunk:break
    buffer+=chunk
    while b'\0' in buffer:
        raw,buffer=buffer.split(b'\0',1);command=json.loads(raw)
        with open(profile+'/commands.jsonl','a') as output:output.write(json.dumps(command)+'\n')
        method=command['method'];params=command.get('params',{});session=command.get('sessionId');result={}
        with state_lock:
            if method=='Target.createTarget':
                sequence+=1;target='target-'+str(sequence);pages[target]={'url':params.get('url','about:blank'),'title':'Initial '+str(sequence),'closed':False};result={'targetId':target}
            elif method=='Target.attachToTarget':
                target=params['targetId'];session_id='opaque-session-'+target;sessions[session_id]=target;result={'sessionId':session_id}
            elif session in sessions:
                target=sessions[session];page=pages[target]
                if method=='Page.getFrameTree':result={'frameTree':{'frame':{'id':target+'-frame'}}}
                elif method=='Page.getNavigationHistory':result={'currentIndex':0,'entries':[{'id':1,'url':page['url'],'title':page['title']}]}
                elif method=='Runtime.evaluate':result={'result':{'value':{'url':page['url'],'title':page['title'],'readyState':'complete'}}}
                elif method=='Page.captureScreenshot':result={'data':'/9j/2Q=='}
                elif method=='Fixture.schedule':
                    threading.Thread(target=later,args=(session,dict(params)),daemon=True).start();result={'scheduled':True}
        write({'id':command['id'],'result':result})
