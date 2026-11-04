#!/usr/bin/env python3
"""Held native turns expose selection changes without real inference or credentials."""
import ctypes,json,os,signal,sys,uuid
ctypes.CDLL(None).prctl(1,signal.SIGTERM)
from pathlib import Path
claude='-p' in sys.argv
root=Path.cwd()
model=sys.argv[sys.argv.index('--model')+1] if '--model' in sys.argv else None
native=sys.argv[sys.argv.index('--resume')+1] if '--resume' in sys.argv else (sys.argv[sys.argv.index('--session-id')+1] if '--session-id' in sys.argv else str(uuid.uuid4()))
turns={}
def emit(v):print(json.dumps(v),flush=True)
def log(method,params):
 if not (root/'.selection-fixture').exists():return
 # Never record injected MCP authentication/configuration.
 with (root/'selection-trace.jsonl').open('a') as f:f.write(json.dumps({'pid':os.getpid(),'method':method,'params':{k:v for k,v in params.items() if k!='config'}})+'\n')
for line in sys.stdin:
 f=json.loads(line)
 if claude:
  if f['type']=='control_request':
   r=f['request'];log('claude/'+r['subtype'],r)
   if r['subtype']=='set_model':model=r['model']
   emit({'type':'control_response','response':{'request_id':f['request_id'],'subtype':'success','response':{}}});continue
  if f['type']!='user':continue
  text=f['message']['content'][0]['text'];log('claude/user',{'text':text,'model':model})
  emit({'type':'system','subtype':'init','session_id':native})
  emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'native:'+text}}})
  if not text.startswith('HOLD'):emit({'type':'result','subtype':'success','session_id':native})
  continue
 m,i,p=f.get('method'),f.get('id'),f.get('params',{})
 if i is None:continue
 if m in ('thread/start','thread/resume'):root=Path(p['cwd'])
 log(m,p)
 if m=='initialize':r={'capabilities':{}}
 elif m=='model/list':r={'data':[{'model':'one','displayName':'One','isDefault':True},{'model':'two','displayName':'Two'}]}
 elif m=='account/read':r={'account':{'type':'chatgpt'}}
 elif m=='account/rateLimits/read':r={'rateLimits':{}}
 elif m=='skills/list':r={'data':[]}
 elif m in ('thread/start','thread/resume'):
  native=p.get('threadId') or str(uuid.uuid4());r={'thread':{'id':native}}
 elif m=='thread/unsubscribe':
  emit({'id':i,'result':{'status':'unsubscribed'}});emit({'method':'thread/closed','params':{'threadId':p['threadId']}});continue
 elif m=='turn/start':
  native=p['threadId'];turn=str(uuid.uuid4());turns[native]=turn;r={'turn':{'id':turn}}
 else:r={}
 emit({'id':i,'result':r})
 if m=='turn/start':
  text=p['input'][0]['text'];emit({'method':'item/agentMessage/delta','params':{'threadId':native,'turnId':turn,'delta':'native:'+text}})
  if not text.startswith('HOLD'):emit({'method':'turn/completed','params':{'threadId':native,'turn':{'id':turn,'status':'completed'}}})
 elif m in ('turn/steer','turn/interrupt'):
  native=p['threadId'];turn=turns[native]
  emit({'method':'turn/completed','params':{'threadId':native,'turn':{'id':turn,'status':'completed' if m=='turn/steer' else 'interrupted'}}})
