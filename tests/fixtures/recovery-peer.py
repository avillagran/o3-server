#!/usr/bin/env python3
import json, sys, os, time, select
from pathlib import Path
claude = '-p' in sys.argv
def send(v): print(json.dumps(v), flush=True)
for line in sys.stdin:
 f=json.loads(line); m=f.get('method'); i=f.get('id')
 if claude and f.get('type') == 'user' or m == 'turn/start':
  if claude:
   assert sys.argv[sys.argv.index('--resume')+1] == 'native'
   send({'type':'system','subtype':'init','session_id':'native'})
  else:
   send({'id':i,'result':{'turn':{'id':'turn'}}})
   send({'method':'turn/started','params':{'threadId':'native','turn':{'id':'turn','status':'inProgress'}}})
  marker=Path(os.environ['RECOVERY_MARKER'])
  if not marker.exists():
   marker.write_text('active')
   if select.select([sys.stdin], [], [], 60)[0]: sys.exit(0)
  if claude: send({'type':'result','subtype':'success','session_id':'native','result':'recovered'})
  else: send({'method':'turn/completed','params':{'threadId':'native','turn':{'id':'turn','status':'completed'}}})
 elif m == 'thread/resume':
  assert f['params']['threadId']=='native'
  send({'id':i,'result':{'thread':{'id':'native'}}})
 elif i is not None: send({'id':i,'result':{}})
