#!/usr/bin/python3
"""Owned pipe peer with independent pages and delayed unsolicited CDP notifications."""
import json, os, sys, threading, time
profile=next(arg.split('=',1)[1] for arg in sys.argv if arg.startswith('--user-data-dir='))
with open(profile+'/fixture.pid','w') as output:output.write(str(os.getpid()))
state_lock=threading.Lock(); output_lock=threading.Lock(); pages={}; sessions={}; sequence=0; reject_next_attach=False; hold_next_create=False

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
        method=command['method'];params=command.get('params',{});session=command.get('sessionId');result={};events=[];deferred=[];held=False;failure=None
        with state_lock:
            if method=='Target.createTarget':
                sequence+=1;target='target-'+str(sequence);pages[target]={'url':params.get('url','about:blank'),'title':'Initial '+str(sequence),'closed':False};result={'targetId':target}
                if hold_next_create:hold_next_create=False;time.sleep(6)
            elif method=='Target.attachToTarget':
                if reject_next_attach:reject_next_attach=False;failure={'message':'Synthetic attachment rejection'}
                else:
                    target=params['targetId'];session_id='opaque-session-'+target;sessions[session_id]=target;result={'sessionId':session_id}
            elif method=='Target.closeTarget':
                pages[params['targetId']]['closed']=True;result={'success':True}
            elif method=='Target.getTargets':result={'targetInfos':[{'type':'page','targetId':target,'url':page['url']} for target,page in pages.items() if not page['closed']]}
            elif session in sessions:
                target=sessions[session];page=pages[target]
                if method=='Page.getFrameTree':result={'frameTree':{'frame':{'id':target+'-frame','url':page['url']}}}
                elif method=='Page.getNavigationHistory':result={'currentIndex':0,'entries':[{'id':1,'url':page['url'],'title':page['title']}]}
                elif method=='Runtime.evaluate':
                    if page.get('held') and not params.get('fixtureHold'):
                        page.setdefault('blockedReads',[]).append((command['id'],session));held=True
                    elif params.get('expression')=='document.title' and page.pop('dialogOnTitle',False):
                        page['held']=(command['id'],session);held=True
                        events=[{'method':'Page.javascriptDialogOpening','sessionId':session,'params':{'type':'prompt','message':'Title race prompt','defaultPrompt':'race'}}]
                    elif params.get('expression')=='document.title':result={'result':{'value':page['title']}}
                    elif params.get('fixtureHold') or params.get('expression') in ('fixtureDialog()','fixtureDialogUtf16()','fixtureDialogRawUtf16()'):
                        page['held']=(command['id'],session);held=True
                        message='a'*3999+'🙂' if params.get('expression')=='fixtureDialogUtf16()' else 'Owned prompt'
                        default='b'*3999+'🙂' if params.get('expression')=='fixtureDialogUtf16()' else 'default'
                        if params.get('expression')=='fixtureDialogRawUtf16()':message='\ud800A\udfff🙂';default='\udc00'
                        events=[{'method':'Page.javascriptDialogOpening','sessionId':session,'params':{'type':'prompt','message':message,'defaultPrompt':default,'url':page['url']}}]
                    else:result={'result':{'value':{'url':page['url'],'title':page['title'],'readyState':'complete'}}}
                elif method=='Page.handleJavaScriptDialog':
                    page['title']='Post dialog title'
                    pending=page.pop('held',None)
                    if pending:
                        deferred=[{'id':pending[0],'sessionId':pending[1],'result':{'result':{'value':{'unblocked':True,'accepted':params['accept'],'promptText':params.get('promptText')}}}}]
                    for read_id,read_session in page.pop('blockedReads',[]):
                        deferred.append({'id':read_id,'sessionId':read_session,'result':{'result':{'value':{'url':page['url'],'title':page['title'],'readyState':'complete'}}}})
                    screenshot=page.pop('screenshot',None)
                    if screenshot:
                        deferred.append({'id':screenshot[0],'sessionId':screenshot[1],'result':{'data':'/9j/2Q=='}})
                        page['holdScreenshot']=False
                    events=[{'method':'Page.javascriptDialogClosed','sessionId':session,'params':{'result':params['accept']}}]
                elif method=='Page.navigate':
                    page['url']=params['url'];request_id='navigation-'+str(command['id']);result={'frameId':target+'-frame'}
                    events=[{'method':'Network.requestWillBeSent','sessionId':session,'params':{'requestId':request_id,'loaderId':request_id,'frameId':target+'-frame','type':'Document','request':{'url':page['url'],'method':'GET'}}},{'method':'Page.frameStartedLoading','sessionId':session,'params':{'frameId':target+'-frame'}},{'method':'Page.frameNavigated','sessionId':session,'params':{'frame':{'id':target+'-frame','url':page['url']}}},{'method':'Network.responseReceived','sessionId':session,'params':{'requestId':request_id,'response':{'url':page['url'],'status':200}}},{'method':'Network.loadingFinished','sessionId':session,'params':{'requestId':request_id}},{'method':'Page.loadEventFired','sessionId':session,'params':{'timestamp':1}}]
                elif method=='Page.captureScreenshot':
                    if page.get('holdScreenshot'):
                        page['screenshot']=(command['id'],session);held=True
                    else:result={'data':'/9j/2Q=='}
                elif method=='Fixture.holdScreenshot':page['holdScreenshot']=True
                elif method=='Fixture.dialogOnTitle':page['dialogOnTitle']=True
                elif method=='Fixture.holdNextCreate':hold_next_create=True
                elif method=='Fixture.rejectNextAttach':reject_next_attach=True
                elif method=='Fixture.setUrl':page['url']=params['url']
                elif method=='Fixture.emit':
                    events=[dict(event,sessionId=session) for event in params['events']]
                elif method=='Fixture.schedule':
                    threading.Thread(target=later,args=(session,dict(params)),daemon=True).start();result={'scheduled':True}
        if not held:write({'id':command['id'],**({'sessionId':session} if session is not None else {}),**({'error':failure} if failure is not None else {'result':result})})
        for event in events:write(event)
        for reply in deferred:write(reply)
