#!/usr/bin/env python3
import json, sys
claude = '-p' in sys.argv
native = 'native-fork' if '--fork-session' in sys.argv else 'native'
if '--fork-session' in sys.argv:
 assert sys.argv[sys.argv.index('--resume')+1] == 'native'
def send(v): print(json.dumps(v), flush=True)
for line in sys.stdin:
 f=json.loads(line); m=f.get('method'); p=f.get('params',{}); i=f.get('id')
 if claude:
  if f.get('type') == 'control_request':
   send({'type':'control_response','response':{'request_id':f['request_id'],'subtype':'success'}})
  elif f.get('type') == 'user':
   send({'type':'system','subtype':'init','session_id':native})
   send({'type':'result','subtype':'success','session_id':native,'result':'ok'})
 elif m in ('thread/start','thread/resume','thread/fork'):
  if m == 'thread/fork':
   assert p['threadId'] == 'native'
   native='native-fork'
  send({'id':i,'result':{'thread':{'id':native}}})
 elif m == 'turn/start':
  assert p['model'] in ('haiku','sonnet')
  send({'id':i,'result':{'turn':{'id':'turn'}}})
  send({'method':'turn/completed','params':{'threadId':native,'turn':{'id':'turn','status':'completed'}}})
 elif i is not None: send({'id':i,'result':{}})
