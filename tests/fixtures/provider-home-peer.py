#!/usr/bin/env python3
"""Native peer persists conversations through the actual configured provider home."""
import fcntl,json,os,sys,time,uuid
from pathlib import Path
claude='-p' in sys.argv
home=Path(os.environ['CLAUDE_CONFIG_DIR' if claude else 'CODEX_HOME'])
assert home.is_absolute(), 'configured provider homes must be resolved before changing cwd'
(home/'log').mkdir(parents=True,exist_ok=True)
with (home/'log'/'peer-starts').open('a') as marker:marker.write('start\n')
expected=sys.argv[sys.argv.index('--expected-owner')+1] if '--expected-owner' in sys.argv else None
if expected: assert json.loads((home/'auth.json').read_text())['fixtureOwner']==expected
(home/'sessions').mkdir(parents=True,exist_ok=True)
native=sys.argv[sys.argv.index('--resume')+1] if claude and '--resume' in sys.argv else (sys.argv[sys.argv.index('--session-id')+1] if '--session-id' in sys.argv else str(uuid.uuid4()))
writers={}
retention={}
def acquire(n):
 if n in writers:return True
 locks=home/'thread-writer-locks';locks.mkdir(exist_ok=True)
 writer=(locks/(n+'.lock')).open('a')
 try:fcntl.flock(writer,fcntl.LOCK_EX|fcntl.LOCK_NB)
 except BlockingIOError:writer.close();return False
 writers[n]=writer;return True
def emit(v):print(json.dumps(v),flush=True)
def file(n):return home/'sessions'/(n+'.json')
def read(n):return json.loads(file(n).read_text()) if file(n).is_file() else []
def save(n,items):
 p=file(n).with_suffix('.partial');p.write_text(json.dumps(items));os.replace(p,file(n))
for line in sys.stdin:
 f=json.loads(line)
 if claude:
  if f['type']=='control_request':
   emit({'type':'control_response','response':{'request_id':f['request_id'],'subtype':'success','response':{'account':{'email':'fixture@example.invalid','tokenSource':'fixture'},'commands':[]}}});continue
  if f['type']!='user':continue
  history=read(native)+[f['message']['content'][0]['text']];save(native,history)
  emit({'type':'system','subtype':'init','session_id':native})
  emit({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'native-history:'+' | '.join(history)}}})
  emit({'type':'result','subtype':'success','session_id':native});continue
 m,i,p=f.get('method'),f.get('id'),f.get('params',{})
 if i is None:continue
 if m=='initialize':r={'capabilities':{}}
 elif m=='model/list':r={'data':[{'model':'test','displayName':'Test','isDefault':True}]}
 elif m=='account/read':r={'account':{'type':'chatgpt'}}
 elif m=='account/rateLimits/read':r={'rateLimits':{'primary':{'usedPercent':0,'windowDurationMins':300}}}
 elif m=='skills/list':r={'data':[]}
 elif m=='thread/start':native=str(uuid.uuid4());assert acquire(native);retention[native]=p.get('config',{}).get('thread_unload_delay_secs',60);save(native,[]);r={'thread':{'id':native}}
 elif m=='thread/resume':
  native=p['threadId']
  if not file(native).is_file():emit({'id':i,'error':{'code':-32602,'message':'Thread not found'}});continue
  if not acquire(native):emit({'id':i,'error':{'code':-32600,'message':f'thread {native} already has an active writer'}});continue
  retention[native]=p.get('config',{}).get('thread_unload_delay_secs',60)
  r={'thread':{'id':native}}
 elif m=='thread/unsubscribe':
  n=p['threadId']
  emit({'id':i,'result':{'status':'unsubscribed'}})
  if retention.get(n)==0 and n in writers:
   # Native cache eviction completes after the unsubscribe acknowledgement.
   time.sleep(.2);writers.pop(n).close()
   emit({'method':'thread/closed','params':{'threadId':n}})
  continue
 elif m=='thread/inject_items':
  history=read(p['threadId'])+[b.get('text','') for item in p['items'] for b in item['content']];save(p['threadId'],history);r={}
 elif m=='turn/start':
  with (home/'log'/'turn-processes').open('a') as trace:trace.write(json.dumps({'pid':os.getpid(),'threadId':p['threadId']})+'\n')
  native=p['threadId'];history=read(native)+[p['input'][0]['text']];save(native,history);r={'turn':{'id':str(uuid.uuid4())}}
 else:r={}
 emit({'id':i,'result':r})
 if m=='turn/start':
  emit({'method':'item/agentMessage/delta','params':{'threadId':native,'turnId':r['turn']['id'],'delta':'native-history:'+' | '.join(history)}})
  emit({'method':'turn/completed','params':{'threadId':native,'turn':{'id':r['turn']['id'],'status':'completed'}}})
