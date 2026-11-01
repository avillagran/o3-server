#!/usr/bin/env python3
"""Schema-based peer, deliberately synthetic; does not claim exhausted-account capture."""
import json, sys
for line in sys.stdin:
 f=json.loads(line); i=f.get('id'); m=f.get('method')
 if i is None:continue
 result={}
 if m in ('thread/start','thread/resume'):result={'thread':{'id':'native'}}
 if m=='turn/start':result={'turn':{'id':'turn'}}
 print(json.dumps({'id':i,'result':result}),flush=True)
 if m=='turn/start':
  fail=f['params']['input'][0]['text']=='fail'
  print(json.dumps({'method':'account/rateLimits/updated','params':{'rateLimits':{'limitId':'codex','primary':{'usedPercent':100 if fail else 5,'windowDurationMins':300,'resetsAt':4070908800},'secondary':None,'credits':None,'planType':'plus'}}}),flush=True)
  print(json.dumps({'method':'turn/completed','params':{'threadId':'native','turn':{'id':'turn','status':'failed' if fail else 'completed','items':[],'error':{'message':"You've hit your usage limit. Try again later.",'codexErrorInfo':'usageLimitExceeded','additionalDetails':None} if fail else None}}}),flush=True)
