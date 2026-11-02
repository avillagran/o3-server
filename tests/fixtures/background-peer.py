#!/usr/bin/env python3
import json,sys,pathlib,time,threading
claude='-p' in sys.argv
root=pathlib.Path.cwd()
lock=threading.Lock()
interrupted=threading.Event()
def send(v):
 with lock: print(json.dumps(v),flush=True)
def complete_later():
 gate("finish")
 if interrupted.is_set(): return
 if claude:
  for task in ("bg","child"): send({"type":"system","subtype":"task_notification","task_id":task,"status":"stopped" if task=="child" else "completed","summary":"done"})
 else:
  send({"method":"item/completed","params":{"threadId":"native-bg","turnId":"turn","item":{"id":"bg","type":"commandExecution","command":"sleep 1","status":"completed","exitCode":0}}})
def gate(name):
 end=time.monotonic()+20
 while not (root/name).exists():
  if time.monotonic()>end: raise RuntimeError('gate timeout: '+name)
  time.sleep(.01)
for line in sys.stdin:
 f=json.loads(line);m=f.get('method');i=f.get('id')
 if claude:
  if f['type']=='control_request':
   send({'type':'control_response','response':{'request_id':f['request_id'],'subtype':'success'}})
   if f['request'].get('subtype')=='interrupt':
    interrupted.set()
    for task in ('bg','child'): send({'type':'system','subtype':'task_notification','task_id':task,'status':'stopped','summary':'stopped'})
    send({'type':'result','subtype':'error_during_execution','is_error':True,'result':'interrupted','session_id':'native-bg'})
  elif f['type']=='user':
   gate('bind')
   send({'type':'system','subtype':'init','session_id':'native-bg'})
   send({'type':'assistant','message':{'content':[{'type':'tool_use','id':'bash','name':'Bash','input':{'command':'sleep 1','run_in_background':True}}]}})
   send({'type':'system','subtype':'task_started','task_id':'bg','task_type':'local_bash','tool_use_id':'bash'})
   send({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'bash','content':'Command running in background'}]}})
   send({'type':'assistant','message':{'content':[{'type':'tool_use','id':'agent','name':'Task','input':{'prompt':'fixture','run_in_background':True}}]}})
   send({'type':'system','subtype':'task_started','task_id':'child','task_type':'local_agent','tool_use_id':'agent'})
   send({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'agent','content':'Agent launched in background'}]}})
   send({'type':'result','subtype':'success','session_id':'native-bg'})
   (root/'result').touch();threading.Thread(target=complete_later,daemon=True).start()
 elif m in ('thread/start','thread/resume'):
  root=pathlib.Path(f['params']['cwd'])
  gate('bind');send({'id':i,'result':{'thread':{'id':'native-bg'}}})
 elif m=='turn/start':
  send({'id':i,'result':{'turn':{'id':'turn'}}})
  send({'method':'turn/started','params':{'threadId':'native-bg','turn':{'id':'turn'}}})
  send({'method':'item/started','params':{'threadId':'native-bg','turnId':'turn','item':{'id':'bg','type':'commandExecution','command':'sleep 1','status':'inProgress'}}})
  send({'method':'turn/completed','params':{'threadId':'native-bg','turn':{'id':'turn','status':'completed'}}})
  (root/'result').touch();threading.Thread(target=complete_later,daemon=True).start()
 elif m=='turn/interrupt':
  interrupted.set()
  send({'id':i,'result':{}})
  send({'method':'item/completed','params':{'threadId':'native-bg','turnId':'turn','item':{'id':'bg','type':'commandExecution','command':'sleep 1','status':'completed','exitCode':0}}})
  send({'method':'turn/completed','params':{'threadId':'native-bg','turn':{'id':'turn','status':'interrupted'}}})
 elif i is not None: send({'id':i,'result':{}})
