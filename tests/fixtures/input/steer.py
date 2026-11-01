#!/usr/bin/env python3
import json, sys, base64
from pathlib import Path
claude = '-p' in sys.argv
for line in sys.stdin:
 f=json.loads(line)
 if claude:
  if f['type']=='control_request':
   print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':f['request_id']}}),flush=True);continue
  content=f['message']['content']
 else:
  m=f.get('method');i=f.get('id')
  if i is None: continue
  result={}
  if m in ('thread/start','thread/resume'):result={'thread':{'id':'native'}}
  if m=='turn/start':result={'turn':{'id':'turn'}}
  print(json.dumps({'id':i,'result':result}),flush=True)
  if m not in ('turn/start','turn/steer'):continue
  if m=='turn/start':
   assert f['params']['model']=='override'
   assert f['params']['threadId']=='native'
  content=f['params']['input']
 if content[0]['text']=='hold':
  if claude:print(json.dumps({'type':'system','subtype':'init','session_id':'native'}),flush=True)
  continue
 assert 'context-proof' in content[0]['text']
 if claude:assert base64.b64decode(content[1]['source']['data']).startswith(b'\x89PNG')
 else:assert Path(content[1]['path']).read_bytes().startswith(b'\x89PNG')
 path=json.JSONDecoder().raw_decode(content[2]['text'].split('available at ',1)[1])[0]
 assert Path(path).read_text()=='file-proof'
 if claude:print(json.dumps({'type':'result','subtype':'success','session_id':'native'}),flush=True)
 else:print(json.dumps({'method':'turn/completed','params':{'threadId':'native','turn':{'id':'turn','status':'completed'}}}),flush=True)
