#!/usr/bin/env python3
import json,sys,pathlib,time
claude='-p' in sys.argv
root=pathlib.Path.cwd()
def send(v): print(json.dumps(v),flush=True)
def gate(name):
 end=time.monotonic()+20
 while not (root/name).exists():
  if time.monotonic()>end: raise RuntimeError('gate timeout: '+name)
  time.sleep(.01)
for line in sys.stdin:
 f=json.loads(line);m=f.get('method');i=f.get('id')
 if claude:
  if f['type']=='control_request': send({'type':'control_response','response':{'request_id':f['request_id'],'subtype':'success'}})
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
   (root/'result').touch();gate('finish')
   send({'type':'system','subtype':'task_notification','task_id':'bg','status':'completed','summary':'done'})
   send({'type':'system','subtype':'task_notification','task_id':'child','status':'stopped','summary':'child done'})
 elif m in ('thread/start','thread/resume'):
  root=pathlib.Path(f['params']['cwd'])
  gate('bind');send({'id':i,'result':{'thread':{'id':'native-bg'}}})
 elif m=='turn/start':
  send({'id':i,'result':{'turn':{'id':'turn'}}})
  send({'method':'turn/started','params':{'threadId':'native-bg','turn':{'id':'turn'}}})
  send({'method':'item/started','params':{'threadId':'native-bg','turnId':'turn','item':{'id':'bg','type':'commandExecution','command':'sleep 1','status':'inProgress'}}})
  send({'method':'turn/completed','params':{'threadId':'native-bg','turn':{'id':'turn','status':'completed'}}})
  (root/'result').touch();gate('finish')
  send({'method':'item/completed','params':{'threadId':'native-bg','turnId':'turn','item':{'id':'bg','type':'commandExecution','command':'sleep 1','status':'completed','exitCode':0}}})
 elif i is not None: send({'id':i,'result':{}})
